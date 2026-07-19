FROM mcr.microsoft.com/devcontainers/rust:1-1-bookworm

# System libraries the later phases need: Tesseract for OCR of images and
# scanned PDFs, Pdfium's build dependencies, and pkg-config for the -sys
# crates. Installed here so no contributor has to set up a host toolchain.
RUN apt-get update \
  && apt-get install -y --no-install-recommends \
    clang \
    libclang-dev \
    libleptonica-dev \
    libssl-dev \
    libtesseract-dev \
    pkg-config \
    tesseract-ocr \
    tesseract-ocr-eng \
    tesseract-ocr-fra \
  && apt-get clean \
  && rm -rf /var/lib/apt/lists/*

RUN rustup component add clippy rustfmt \
  && cargo install cargo-nextest --locked
