#!/usr/bin/env bash
# @license MIT
# @copyright 2026 Mickaël Canouil
# @author Mickaël Canouil
#
# oboro installer.
#
#   curl -fsSL https://m.canouil.dev/oboro/install.sh | bash
#
# Downloads the prebuilt release binary for this machine, verifies it against
# the release SHA256SUMS, and installs it. The default binary reads txt, md,
# docx, xlsx and pdf, and touches no network. Passing `--features ner` picks
# the ner build instead, which also finds untold names once the model is
# fetched with `oboro models pull`; on Linux it needs glibc 2.39 or newer.
# Reading images (ocr) needs the Tesseract system libraries, so that stays a
# `cargo build --features ocr` choice.
#
# Environment variables:
#   OBORO_VERSION             Install this version instead of the latest.
#   OBORO_INSTALL_DIR         Install here instead of the resolved default.
#   OBORO_FEATURES            Pick a feature build; `ner` is the only value.
#   OBORO_SKIP_CHECKSUM=1     Skip SHA256 verification (not recommended).
#   OBORO_VERIFY_PROVENANCE=1 Also verify build provenance with the gh CLI.
#
# This installer needs bash. On a minimal distribution such as Alpine, which
# ships only busybox, install it first: `apk add bash curl`.

# POSIX-syntax guard so `sh install.sh` fails clearly rather than mis-parsing
# the bash below. It runs before `set -o pipefail`, which dash rejects.
if [ -z "${BASH_VERSION:-}" ]; then
	echo "This installer needs bash. Run: bash install.sh (or: curl -fsSL https://m.canouil.dev/oboro/install.sh | bash)" >&2
	exit 1
fi

set -euo pipefail

REPO="mcanouil/oboro"
BINARY_NAME="oboro"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info() { echo -e "${GREEN}$1${NC}"; }
warn() { echo -e "${YELLOW}$1${NC}"; }
error() {
	echo -e "${RED}$1${NC}" >&2
	exit 1
}

# mktemp output lives in a global so the EXIT trap, which runs after main()
# returns and its locals are gone, can still see and remove it. Guarded so an
# exit before mktemp (empty tmpdir) does not trip `set -u`.
tmpdir=""
cleanup() {
	[ -n "${tmpdir}" ] && rm -rf "${tmpdir}"
	return 0
}
trap cleanup EXIT

usage() {
	cat <<EOF
oboro installer

Usage:
  curl -fsSL https://m.canouil.dev/oboro/install.sh | bash
  ./install.sh [--version <version>] [--dir <path>] [--features ner] [--help]

Options:
  --version <version>  Install this version instead of the latest.
  --dir <path>         Install into this directory.
  --features ner       Install the ner build, which also finds untold names.
                       Linux: needs glibc 2.39+. Fetch the model afterwards
                       with 'oboro models pull' (about 348 MB, once).
  --help               Show this help and exit.

Environment variables:
  OBORO_VERSION, OBORO_INSTALL_DIR, OBORO_FEATURES, OBORO_SKIP_CHECKSUM,
  OBORO_VERIFY_PROVENANCE. See the script header for details.
EOF
}

# oboro publishes one archive per Rust target triple. Map the running machine
# onto the triple the release job built, so the filename lines up exactly.
# The ner build links ONNX Runtime, which has no musl build, so on Linux it
# is a glibc archive where the default is static musl.
detect_target() {
	local features="$1"
	local os arch
	case "$(uname -s)" in
	Darwin) os="apple-darwin" ;;
	Linux)
		if [ "${features}" = "ner" ]; then
			os="unknown-linux-gnu"
		else
			os="unknown-linux-musl"
		fi
		;;
	*) error "Unsupported OS: $(uname -s). oboro ships binaries for macOS and Linux." ;;
	esac
	case "$(uname -m)" in
	x86_64 | amd64) arch="x86_64" ;;
	aarch64 | arm64) arch="aarch64" ;;
	*) error "Unsupported architecture: $(uname -m)." ;;
	esac
	if [ "${os}" = "apple-darwin" ] && [ "${arch}" = "x86_64" ]; then
		error "oboro no longer ships an Intel macOS binary. Build from source with 'cargo install'."
	fi
	echo "${arch}-${os}"
}

find_install_dir() {
	if [ -n "${OBORO_INSTALL_DIR:-}" ]; then
		# Creating it is left to the install step, which can fall back to sudo
		# for a root-owned path such as /opt/oboro; an eager mkdir here would
		# abort under `set -e` before that fallback is reached.
		echo "${OBORO_INSTALL_DIR}"
	elif [ -w "/usr/local/bin" ]; then
		echo "/usr/local/bin"
	else
		mkdir -p "${HOME}/.local/bin"
		echo "${HOME}/.local/bin"
	fi
}

download() {
	local url="$1" output="$2"
	if command -v curl &>/dev/null; then
		curl -fsSL "${url}" -o "${output}"
	elif command -v wget &>/dev/null; then
		wget -q "${url}" -O "${output}"
	else
		error "Neither curl nor wget is available."
	fi
}

get_latest_version() {
	# Follow the redirect from the HTML /releases/latest to /releases/tag/<tag>.
	# Unlike api.github.com this is not rate-limited to 60 requests per hour per
	# IP, so users behind a shared address are not turned away with a 403.
	local url="https://github.com/${REPO}/releases/latest"
	local final_url=""
	if command -v curl &>/dev/null; then
		final_url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "${url}") || return 1
	elif command -v wget &>/dev/null; then
		final_url=$(wget --spider -S "${url}" 2>&1 |
			awk 'tolower($1)=="location:" {print $2}' |
			tail -1 |
			tr -d '\r\n') || return 1
	else
		return 1
	fi
	case "${final_url}" in
	*/releases/tag/*) echo "${final_url##*/releases/tag/}" ;;
	*) return 1 ;;
	esac
}

verify_checksum() {
	local file="$1" checksums_file="$2" filename="$3"

	if [ ! -f "${checksums_file}" ]; then
		error "SHA256SUMS is not available. Set OBORO_SKIP_CHECKSUM=1 to bypass."
	fi

	local expected
	expected=$(awk -v f="${filename}" '{gsub(/^\*/, "", $2); if ($2==f) {print $1; exit}}' "${checksums_file}")
	if [ -z "${expected}" ]; then
		error "No checksum for ${filename} in SHA256SUMS."
	fi

	local actual
	if command -v sha256sum &>/dev/null; then
		actual=$(sha256sum "${file}" | cut -d' ' -f1)
	elif command -v shasum &>/dev/null; then
		actual=$(shasum -a 256 "${file}" | cut -d' ' -f1)
	else
		error "No sha256 tool found. Install coreutils or set OBORO_SKIP_CHECKSUM=1 to bypass."
	fi

	if [ "${expected}" != "${actual}" ]; then
		error "Checksum verification failed.\n  Expected: ${expected}\n  Actual:   ${actual}"
	fi
	info "Checksum verified."
}

verify_provenance() {
	local file="$1"
	if ! command -v gh &>/dev/null; then
		error "OBORO_VERIFY_PROVENANCE=1 needs the gh CLI, which is not installed."
	fi
	info "Verifying build provenance..."
	if ! gh attestation verify "${file}" --repo "${REPO}"; then
		error "Build provenance verification failed."
	fi
}

main() {
	local version="${OBORO_VERSION:-}"
	local install_dir_override=""
	local features="${OBORO_FEATURES:-}"

	while [ "$#" -gt 0 ]; do
		case "$1" in
		--version)
			[ "$#" -ge 2 ] || error "--version needs a value."
			version="$2"
			shift 2
			;;
		--dir)
			[ "$#" -ge 2 ] || error "--dir needs a value."
			install_dir_override="$2"
			shift 2
			;;
		--features)
			[ "$#" -ge 2 ] || error "--features needs a value."
			features="$2"
			shift 2
			;;
		--help | -h)
			usage
			exit 0
			;;
		*) error "Unknown argument: $1 (try --help)." ;;
		esac
	done

	case "${features}" in
	"" | ner) ;;
	*) error "Unsupported feature build: ${features}. The only prebuilt feature build is 'ner'; ocr needs a source build." ;;
	esac

	info "Installing ${BINARY_NAME}..."
	echo

	local target
	target=$(detect_target "${features}")

	if [ -z "${version}" ]; then
		info "Resolving the latest release..."
		version=$(get_latest_version) ||
			error "Could not resolve the latest version. Pass --version or see https://github.com/${REPO}/releases."
	fi
	# The tags carry no leading v; accept one anyway so a pasted v0.2.0 works.
	version="${version#v}"

	local install_dir
	install_dir=$(OBORO_INSTALL_DIR="${install_dir_override:-${OBORO_INSTALL_DIR:-}}" find_install_dir)

	info "Version:           ${version}"
	info "Target:            ${target}"
	if [ "${features}" = "ner" ]; then
		info "Features:          ner"
	fi
	info "Install directory: ${install_dir}"
	echo

	# The ner archives carry the feature in the filename, after the target.
	local variant_suffix=""
	if [ "${features}" = "ner" ]; then
		variant_suffix="-ner"
	fi
	local filename="${BINARY_NAME}-${version}-${target}${variant_suffix}.tar.gz"
	local base_url="https://github.com/${REPO}/releases/download/${version}"

	tmpdir=$(mktemp -d)

	info "Downloading ${filename}..."
	if ! download "${base_url}/${filename}" "${tmpdir}/${filename}"; then
		if [ "${features}" = "ner" ]; then
			error "Download failed. Releases up to 0.2.0 carry no ner archives; see https://github.com/${REPO}/releases for available builds."
		fi
		error "Download failed. See https://github.com/${REPO}/releases for available builds."
	fi

	if [ "${OBORO_SKIP_CHECKSUM:-0}" = "1" ]; then
		warn "Checksum verification skipped (OBORO_SKIP_CHECKSUM=1)."
	else
		download "${base_url}/SHA256SUMS" "${tmpdir}/SHA256SUMS" ||
			error "Could not download SHA256SUMS. Set OBORO_SKIP_CHECKSUM=1 to bypass."
		verify_checksum "${tmpdir}/${filename}" "${tmpdir}/SHA256SUMS" "${filename}"
	fi

	if [ "${OBORO_VERIFY_PROVENANCE:-0}" = "1" ]; then
		verify_provenance "${tmpdir}/${filename}"
	fi

	info "Extracting..."
	tar -xzf "${tmpdir}/${filename}" -C "${tmpdir}"
	[ -f "${tmpdir}/${BINARY_NAME}" ] || error "The archive did not contain a ${BINARY_NAME} binary."

	# A root-owned directory such as /usr/local/bin needs sudo for every write,
	# the signing included, so resolve the prefix once. Left empty when the
	# directory is writable, so nothing runs under sudo needlessly.
	local sudo_cmd=""
	if [ ! -w "${install_dir}" ]; then
		warn "${install_dir} is not writable; using sudo."
		sudo_cmd="sudo"
	fi
	# Create the directory now, with sudo when it or its parent is root-owned,
	# so a custom OBORO_INSTALL_DIR that does not yet exist is handled here.
	# shellcheck disable=SC2086
	${sudo_cmd} mkdir -p "${install_dir}"
	# shellcheck disable=SC2086
	${sudo_cmd} mv "${tmpdir}/${BINARY_NAME}" "${install_dir}/"
	# shellcheck disable=SC2086
	${sudo_cmd} chmod +x "${install_dir}/${BINARY_NAME}"

	# An unsigned Mach-O binary is killed by Gatekeeper on first run; an ad-hoc
	# signature is enough to let it start. Non-fatal, since the binary still runs
	# once the user clears it manually.
	if [ "$(uname -s)" = "Darwin" ]; then
		# shellcheck disable=SC2086
		${sudo_cmd} codesign -s - "${install_dir}/${BINARY_NAME}" 2>/dev/null || true
	fi

	echo
	info "Installed ${BINARY_NAME} ${version} to ${install_dir}/${BINARY_NAME}."
	echo

	case ":${PATH}:" in
	*":${install_dir}:"*) ;;
	*)
		warn "${install_dir} is not on your PATH. Add this to your shell profile:"
		echo "  export PATH=\"${install_dir}:\$PATH\""
		echo
		;;
	esac

	echo "Next steps:"
	if [ "${features}" = "ner" ]; then
		echo "  ${BINARY_NAME} models pull   # Fetch the recognition model (about 348 MB, once)"
	fi
	echo "  ${BINARY_NAME} doctor   # Report what this build can do"
	echo "  ${BINARY_NAME} --help   # List the commands"
	echo
	if [ "${features}" = "ner" ]; then
		if [ "$(uname -s)" = "Linux" ]; then
			echo "The ner binary links glibc: it needs glibc 2.39 or newer (Ubuntu 24.04+,"
			echo "Debian 13+). On older distributions, build from source instead."
			echo
		fi
	else
		echo "This is the default feature set. Finding untold names (ner) is also"
		echo "prebuilt: rerun with --features ner. Reading images (ocr) needs a"
		echo "source build; see https://m.canouil.dev/oboro/quickstart.html"
	fi
}

# Guard: run main only when executed directly, not when sourced. A curl | bash
# pipe leaves BASH_SOURCE[0] empty, which we treat as direct execution.
if [[ "${BASH_SOURCE[0]-}" == "${0}" || -z "${BASH_SOURCE[0]-}" ]]; then
	main "$@"
fi
