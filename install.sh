#!/usr/bin/env bash
set -euo pipefail

REPO="maceip/tsh"
INSTALL_DIR="${TSH_INSTALL_DIR:-$HOME/.tsh}"
BIN_DIR="$INSTALL_DIR/bin"
BASE_URL="https://github.com/$REPO/releases"

# --- Helpers ----------------------------------------------------------------

info()  { printf '\033[1;32m%s\033[0m\n' "$*"; }
warn()  { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
error() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

need_cmd() {
    if ! command -v "$1" > /dev/null 2>&1; then
        error "need '$1' (command not found)"
    fi
}

fetch() {
    local url="$1" dest="$2"
    if command -v curl > /dev/null 2>&1; then
        curl -fsSL "$url" -o "$dest"
    elif command -v wget > /dev/null 2>&1; then
        wget -qO "$dest" "$url"
    else
        error "need curl or wget"
    fi
}

fetch_url() {
    local url="$1"
    if command -v curl > /dev/null 2>&1; then
        curl -fsSL "$url"
    elif command -v wget > /dev/null 2>&1; then
        wget -qO- "$url"
    else
        error "need curl or wget"
    fi
}

# --- Detect platform --------------------------------------------------------

detect_os() {
    case "$(uname -s)" in
        Linux*)  echo "linux" ;;
        Darwin*) echo "darwin" ;;
        *)       error "unsupported OS: $(uname -s)" ;;
    esac
}

detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64)   echo "x86_64" ;;
        aarch64|arm64)  echo "aarch64" ;;
        *)              error "unsupported architecture: $(uname -m)" ;;
    esac
}

# --- Version helpers --------------------------------------------------------

version_gt() {
    local IFS=.
    local i ver1=($1) ver2=($2)
    for ((i = 0; i < 3; i++)); do
        local v1=${ver1[i]:-0}
        local v2=${ver2[i]:-0}
        if ((v1 > v2)); then return 0; fi
        if ((v1 < v2)); then return 1; fi
    done
    return 1
}

get_latest_version() {
    local response
    response="$(fetch_url "https://api.github.com/repos/$REPO/releases/latest")"
    echo "$response" | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/'
}

# --- Main -------------------------------------------------------------------

main() {
    local OS ARCH VERSION ARCHIVE_NAME DOWNLOAD_URL TMPDIR

    OS="$(detect_os)"
    ARCH="$(detect_arch)"

    info "Detected platform: $OS-$ARCH"

    # Determine version
    if [ -n "${TSH_VERSION:-}" ]; then
        VERSION="$TSH_VERSION"
    else
        info "Fetching latest release..."
        VERSION="$(get_latest_version)"
    fi

    if [ -z "$VERSION" ]; then
        error "could not determine latest version"
    fi

    info "Installing tsh $VERSION"

    ARCHIVE_NAME="tsh-${VERSION}-${OS}-${ARCH}.tar.gz"
    DOWNLOAD_URL="${BASE_URL}/download/${VERSION}/${ARCHIVE_NAME}"

    # Download
    TMPDIR="$(mktemp -d)"
    trap 'rm -rf "$TMPDIR"' EXIT

    info "Downloading $DOWNLOAD_URL"
    fetch "$DOWNLOAD_URL" "$TMPDIR/$ARCHIVE_NAME"

    # Extract
    tar xzf "$TMPDIR/$ARCHIVE_NAME" -C "$TMPDIR"

    # Install
    mkdir -p "$BIN_DIR"
    local STAGING_DIR="$TMPDIR/tsh-${VERSION}-${OS}-${ARCH}"
    cp "$STAGING_DIR/tsh" "$BIN_DIR/tsh"
    chmod +x "$BIN_DIR/tsh"
    cp "$STAGING_DIR/safety_filter.py" "$BIN_DIR/safety_filter.py"

    # Store version
    echo "$VERSION" > "$INSTALL_DIR/.version"

    # --- Generate tsh-update script -----------------------------------------
    cat > "$BIN_DIR/tsh-update" << 'UPDATER'
#!/usr/bin/env bash
set -euo pipefail

REPO="maceip/tsh"
INSTALL_DIR="${TSH_INSTALL_DIR:-$HOME/.tsh}"
BIN_DIR="$INSTALL_DIR/bin"
BASE_URL="https://github.com/$REPO/releases"

info()  { printf '\033[1;32m%s\033[0m\n' "$*"; }
warn()  { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
error() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

fetch() {
    local url="$1" dest="$2"
    if command -v curl > /dev/null 2>&1; then
        curl -fsSL "$url" -o "$dest"
    elif command -v wget > /dev/null 2>&1; then
        wget -qO "$dest" "$url"
    else
        error "need curl or wget"
    fi
}

fetch_url() {
    local url="$1"
    if command -v curl > /dev/null 2>&1; then
        curl -fsSL "$url"
    elif command -v wget > /dev/null 2>&1; then
        wget -qO- "$url"
    else
        error "need curl or wget"
    fi
}

detect_os() {
    case "$(uname -s)" in
        Linux*)  echo "linux" ;;
        Darwin*) echo "darwin" ;;
        *)       error "unsupported OS: $(uname -s)" ;;
    esac
}

detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64)   echo "x86_64" ;;
        aarch64|arm64)  echo "aarch64" ;;
        *)              error "unsupported architecture: $(uname -m)" ;;
    esac
}

version_gt() {
    local IFS=.
    local i ver1=($1) ver2=($2)
    for ((i = 0; i < 3; i++)); do
        local v1=${ver1[i]:-0}
        local v2=${ver2[i]:-0}
        if ((v1 > v2)); then return 0; fi
        if ((v1 < v2)); then return 1; fi
    done
    return 1
}

CURRENT="unknown"
if [ -f "$INSTALL_DIR/.version" ]; then
    CURRENT="$(cat "$INSTALL_DIR/.version")"
fi

info "Current version: $CURRENT"
info "Checking for updates..."

LATEST_TAG="$(fetch_url "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')"

if [ -z "$LATEST_TAG" ]; then
    error "could not determine latest version"
fi

CURRENT_NUM="${CURRENT#v}"
LATEST_NUM="${LATEST_TAG#v}"

if [ "$CURRENT" = "$LATEST_TAG" ] || ! version_gt "$LATEST_NUM" "$CURRENT_NUM"; then
    info "Already up to date ($CURRENT)"
    exit 0
fi

info "Updating from $CURRENT to $LATEST_TAG"

OS="$(detect_os)"
ARCH="$(detect_arch)"
ARCHIVE_NAME="tsh-${LATEST_TAG}-${OS}-${ARCH}.tar.gz"
DOWNLOAD_URL="${BASE_URL}/download/${LATEST_TAG}/${ARCHIVE_NAME}"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

fetch "$DOWNLOAD_URL" "$TMPDIR/$ARCHIVE_NAME"
tar xzf "$TMPDIR/$ARCHIVE_NAME" -C "$TMPDIR"

STAGING_DIR="$TMPDIR/tsh-${LATEST_TAG}-${OS}-${ARCH}"
cp "$STAGING_DIR/tsh" "$BIN_DIR/tsh"
chmod +x "$BIN_DIR/tsh"
cp "$STAGING_DIR/safety_filter.py" "$BIN_DIR/safety_filter.py"
echo "$LATEST_TAG" > "$INSTALL_DIR/.version"

info "Updated to $LATEST_TAG"
UPDATER
    chmod +x "$BIN_DIR/tsh-update"

    # --- Generate tsh-uninstall script --------------------------------------
    cat > "$BIN_DIR/tsh-uninstall" << 'UNINSTALLER'
#!/usr/bin/env bash
set -euo pipefail

INSTALL_DIR="${TSH_INSTALL_DIR:-$HOME/.tsh}"

info()  { printf '\033[1;32m%s\033[0m\n' "$*"; }

if [ "${1:-}" != "--yes" ]; then
    printf "This will remove tsh from %s and clean PATH entries. Continue? [y/N] " "$INSTALL_DIR"
    read -r answer
    case "$answer" in
        [yY]*) ;;
        *) echo "Aborted."; exit 0 ;;
    esac
fi

# Remove install directory
rm -rf "$INSTALL_DIR"

# Clean PATH from shell rc files
for rc_file in "$HOME/.bashrc" "$HOME/.bash_profile" "$HOME/.zshrc" "$HOME/.profile"; do
    if [ -f "$rc_file" ]; then
        # Use a temp file for portability (macOS sed -i differs from GNU)
        grep -v '\.tsh/bin' "$rc_file" > "$rc_file.tmp" && mv "$rc_file.tmp" "$rc_file"
    fi
done

info "tsh has been uninstalled."
echo "Open a new terminal or source your shell profile for PATH changes to take effect."
UNINSTALLER
    chmod +x "$BIN_DIR/tsh-uninstall"

    # --- Configure PATH -----------------------------------------------------
    configure_path

    # --- Python check -------------------------------------------------------
    if ! command -v python3 > /dev/null 2>&1; then
        warn "python3 not found — tsh requires Python 3 for the safety filter"
    fi

    # --- Done ---------------------------------------------------------------
    echo ""
    info "tsh $VERSION installed to $BIN_DIR"
    echo ""
    echo "  To get started, open a new terminal or run:"
    echo ""
    local shell_name
    shell_name="$(basename "${SHELL:-/bin/bash}")"
    case "$shell_name" in
        zsh)  echo "    source ~/.zshrc" ;;
        bash) echo "    source ~/.bashrc" ;;
        *)    echo "    source ~/.profile" ;;
    esac
    echo ""
    echo "  Then run:  tsh"
    echo ""
    echo "  Other commands:"
    echo "    tsh-update      Update to the latest version"
    echo "    tsh-uninstall   Remove tsh from your system"
    echo ""
}

configure_path() {
    local path_entry="export PATH=\"$BIN_DIR:\$PATH\""
    local added=false

    local shell_name
    shell_name="$(basename "${SHELL:-/bin/bash}")"

    case "$shell_name" in
        zsh)
            add_to_rc "$HOME/.zshrc" "$path_entry" && added=true
            ;;
        bash)
            add_to_rc "$HOME/.bashrc" "$path_entry" && added=true
            if [ -f "$HOME/.bash_profile" ]; then
                add_to_rc "$HOME/.bash_profile" "$path_entry"
            fi
            ;;
        *)
            add_to_rc "$HOME/.profile" "$path_entry" && added=true
            ;;
    esac

    # Also ensure .profile is covered as a fallback
    if [ "$shell_name" != "zsh" ] && [ ! -f "$HOME/.bash_profile" ]; then
        add_to_rc "$HOME/.profile" "$path_entry"
    fi

    export PATH="$BIN_DIR:$PATH"
}

add_to_rc() {
    local rc_file="$1" line="$2"
    if [ -f "$rc_file" ] && grep -q '\.tsh/bin' "$rc_file" 2>/dev/null; then
        return 1
    fi
    {
        echo ''
        echo '# tsh (Token Shell)'
        echo "$line"
    } >> "$rc_file"
    info "Added tsh to PATH in $rc_file"
    return 0
}

main "$@"
