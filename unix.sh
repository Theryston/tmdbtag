#!/usr/bin/env bash
set -eo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
RESET='\033[0m'

REPO="Theryston/tmdbtag"
BINARY_NAME="tmdbtag"
INSTALL_DIR="$HOME/.local/bin"

if [ -n "$TMDBTAG_INSTALL_DIR" ]; then
    INSTALL_DIR="$TMDBTAG_INSTALL_DIR"
elif [ -n "$XDG_BIN_HOME" ]; then
    INSTALL_DIR="$XDG_BIN_HOME"
fi

printf '%b\n' "$BLUE  tmdbtag installer$RESET"
printf '%s\n\n' "Installing the latest release..."

detect_os() {
    case "$(uname -s)" in
        Linux*) printf '%s\n' "linux" ;;
        Darwin*) printf '%s\n' "macos" ;;
        *) printf '%s\n' "unknown" ;;
    esac
}

detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64) printf '%s\n' "x86_64" ;;
        aarch64|arm64) printf '%s\n' "aarch64" ;;
        *) printf '%s\n' "unknown" ;;
    esac
}

OS="$(detect_os)"
ARCH="$(detect_arch)"

if [ "$OS" = "unknown" ]; then
    printf '%b\n' "$RED Error: unsupported operating system$RESET"
    printf '%s\n' "This installer supports Linux and macOS."
    exit 1
fi

if [ "$ARCH" = "unknown" ]; then
    printf '%b\n' "$RED Error: unsupported architecture$RESET"
    printf '%s\n' "This installer supports x86_64 and ARM64."
    exit 1
fi

if [ "$OS" = "linux" ]; then
    TARGET="$ARCH-unknown-linux-gnu"
else
    TARGET="$ARCH-apple-darwin"
fi

if command -v curl >/dev/null 2>&1; then
    DOWNLOADER="curl"
elif command -v wget >/dev/null 2>&1; then
    DOWNLOADER="wget"
else
    printf '%b\n' "$RED Error: curl or wget is required.$RESET"
    exit 1
fi

download() {
    URL="$1"
    DESTINATION="$2"

    if [ "$DOWNLOADER" = "curl" ]; then
        curl --proto '=https' --tlsv1.2 --fail --silent --show-error \
            --location --retry 3 --retry-delay 1 "$URL" --output "$DESTINATION"
    else
        wget --https-only --tries=3 --quiet "$URL" --output-document="$DESTINATION"
    fi
}

printf '%b\n' "$YELLOW Detected target:$RESET $TARGET"
printf '%b\n' "$BLUE Fetching the latest release...$RESET"

API_URL="https://api.github.com/repos/$REPO/releases/latest"
if [ "$DOWNLOADER" = "curl" ]; then
    RELEASE_JSON="$(curl --proto '=https' --tlsv1.2 --fail --silent --show-error \
        --location --retry 3 --retry-delay 1 "$API_URL")"
else
    RELEASE_JSON="$(wget --https-only --tries=3 --quiet "$API_URL" --output-document=-)"
fi

VERSION="$(printf '%s\n' "$RELEASE_JSON" | sed -nE \
    's/^[[:space:]]*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/p' | head -n 1)"

if [ -z "$VERSION" ] || [[ ! "$VERSION" =~ ^v[0-9]+[A-Za-z0-9.+-]*$ ]]; then
    printf '%b\n' "$RED Error: could not determine a valid latest release.$RESET"
    exit 1
fi

ARCHIVE_NAME="$BINARY_NAME-$TARGET.tar.gz"
ARCHIVE_URL="https://github.com/$REPO/releases/download/$VERSION/$ARCHIVE_NAME"
CHECKSUM_URL="https://github.com/$REPO/releases/download/$VERSION/checksums-sha256.txt"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

ARCHIVE_PATH="$TMP_DIR/$ARCHIVE_NAME"
CHECKSUM_PATH="$TMP_DIR/checksums-sha256.txt"

printf '%b\n' "$GREEN Latest version:$RESET $VERSION"
printf '%b\n' "$BLUE Downloading $ARCHIVE_NAME...$RESET"
download "$ARCHIVE_URL" "$ARCHIVE_PATH"
download "$CHECKSUM_URL" "$CHECKSUM_PATH"

EXPECTED_CHECKSUM="$(awk -v file="$ARCHIVE_NAME" '$2 == file { print $1; exit }' "$CHECKSUM_PATH")"
if [ -z "$EXPECTED_CHECKSUM" ]; then
    printf '%b\n' "$RED Error: the release checksum is missing.$RESET"
    exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
    ACTUAL_CHECKSUM="$(sha256sum "$ARCHIVE_PATH" | awk '{ print $1 }')"
elif command -v shasum >/dev/null 2>&1; then
    ACTUAL_CHECKSUM="$(shasum -a 256 "$ARCHIVE_PATH" | awk '{ print $1 }')"
else
    printf '%b\n' "$RED Error: sha256sum or shasum is required to verify the release.$RESET"
    exit 1
fi

if [ "$EXPECTED_CHECKSUM" != "$ACTUAL_CHECKSUM" ]; then
    printf '%b\n' "$RED Error: checksum verification failed.$RESET"
    exit 1
fi

if ! tar -tzf "$ARCHIVE_PATH" | grep -Fxq "$BINARY_NAME"; then
    printf '%b\n' "$RED Error: the release archive has an unexpected layout.$RESET"
    exit 1
fi

tar -xzf "$ARCHIVE_PATH" -C "$TMP_DIR"
EXTRACTED_BINARY="$TMP_DIR/$BINARY_NAME"
if [ ! -f "$EXTRACTED_BINARY" ]; then
    printf '%b\n' "$RED Error: the binary was not found after extraction.$RESET"
    exit 1
fi

mkdir -p "$INSTALL_DIR"
printf '%b\n' "$BLUE Installing to:$RESET $INSTALL_DIR"
install -m 0755 "$EXTRACTED_BINARY" "$INSTALL_DIR/$BINARY_NAME"

if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    export PATH="$PATH:$INSTALL_DIR"
    SHELL_PATH="$SHELL"
    if [ -z "$SHELL_PATH" ]; then
        SHELL_PATH="sh"
    fi
    SHELL_NAME="$(basename "$SHELL_PATH")"

    case "$SHELL_NAME" in
        zsh)
            PROFILE="$HOME/.zshrc"
            ;;
        bash)
            if [ -f "$HOME/.bashrc" ]; then
                PROFILE="$HOME/.bashrc"
            else
                PROFILE="$HOME/.bash_profile"
            fi
            ;;
        fish)
            PROFILE="$HOME/.config/fish/config.fish"
            ;;
        *)
            PROFILE="$HOME/.profile"
            ;;
    esac

    if ! grep -Fq "$INSTALL_DIR" "$PROFILE" 2>/dev/null; then
        mkdir -p "$(dirname "$PROFILE")"
        {
            printf '\n'
            printf '%s\n' "# Added by tmdbtag installer"
            if [ "$SHELL_NAME" = "fish" ]; then
                printf '%s\n' "set -gx PATH \"$INSTALL_DIR\" \$PATH"
            else
                printf '%s\n' "export PATH=\"\$PATH:$INSTALL_DIR\""
            fi
        } >> "$PROFILE"
    fi
fi

printf '\n%b\n' "$GREEN tmdbtag installed successfully.$RESET"
printf '%s\n' "Run 'tmdbtag --help' to get started."
