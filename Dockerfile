# @license MIT
# @copyright 2026 Mickaël Canouil
# @author Mickaël Canouil

# A statically linked build of the default feature set.
#
# The point of the image is that you can read it in a minute and see that
# there is nothing in it. The result holds one binary: no shell, no package
# manager, no interpreter. The default build does not depend on ureq, which
# sits behind the `ner` feature, so the image carries no HTTP client and no
# TLS stack either.
#
# OCR and the recognition model need system libraries and a 348 MB download,
# so they stay a build-from-source choice rather than bloating this.

# Pinned by digest so a registry-side tag repoint cannot change what the binary
# is built from. Dependabot bumps the digest when the tag moves.
FROM rust:1-alpine@sha256:3c38f3f82c2f3d73da3b38e18d279393a04cb43ddded0e35088a8c3324d40900 AS build

# musl-dev and the C toolchain are for rusqlite, which vendors SQLite's C
# sources through its `bundled` feature. Nothing else in the default build
# needs a system library.
RUN apk add --no-cache musl-dev

WORKDIR /src

# Dependencies are built from the manifests alone, so editing the source does
# not rebuild the whole tree on every image build.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
RUN mkdir -p src \
  && echo 'fn main() {}' > src/main.rs \
  && echo '' > src/lib.rs \
  && cargo build --release --locked \
  && rm -rf src

COPY src ./src
# The stale fingerprint from the stub above would otherwise be reused.
RUN touch src/main.rs src/lib.rs \
  && cargo build --release --locked \
  && strip target/release/oboro

# Docker seeds a fresh named volume from whatever the image holds at that
# path, ownership included. Shipping an empty directory owned by the runtime
# user is what makes `-v oboro-vault:/vault` writable; without it the volume
# arrives owned by root and the container cannot write its own key.
RUN mkdir -p /seed-vault

# Pinned by digest for the same reason as the build stage.
FROM gcr.io/distroless/static-debian12:nonroot@sha256:f5b485ea962d9bd1186b2f6b3a061191539b905b82ec395de78cbfae51f20e35

COPY --from=build /src/target/release/oboro /usr/local/bin/oboro
COPY --from=build --chown=nonroot:nonroot /seed-vault /vault

# A container's filesystem goes when the container does. For this tool that
# is not an inconvenience but a silent, permanent failure: clean succeeds,
# the mapping vanishes with the container, and restore can never recover the
# document. Pointing the defaults at a declared volume means the documented
# commands all mount one place.
# Named KEY_FILE rather than KEY because it holds a path, not key material;
# the shorter name reads like a secret and gets flagged as one.
ENV OBORO_VAULT=/vault/vault.db \
    OBORO_KEY_FILE=/vault/key
VOLUME ["/vault"]

WORKDIR /work

ENTRYPOINT ["/usr/local/bin/oboro"]
CMD ["--help"]

LABEL org.opencontainers.image.title="oboro" \
      org.opencontainers.image.description="Replace sensitive values in documents with reversible placeholders before sharing them with a language model." \
      org.opencontainers.image.source="https://github.com/mcanouil/oboro" \
      org.opencontainers.image.documentation="https://m.canouil.dev/oboro/" \
      org.opencontainers.image.licenses="MIT"
