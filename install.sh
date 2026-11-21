#!/bin/sh
# ears installer.
#
#   curl -fsSL https://raw.githubusercontent.com/heiervang-technologies/ears/main/install.sh | sh
#
# Checks for the programs ears runs (PipeWire's recorder, a typing tool,
# notifications, sound cues) and offers to install the missing ones with your
# package manager. Then installs a prebuilt ears for your CPU from the latest
# GitHub release, or builds it with cargo when there is none for this system.
# Run from a checkout of the repo, it builds that checkout instead.
#
# Termux builds from source (experimental: ears needs PipeWire to listen, so
# on a phone it mostly serves as the client end of an ssh session).
#
# Options (after `| sh -s --` when piped):
#   --yes            install missing packages without asking
#   --no-deps        do not check or install packages
#   --from-source    build with cargo even when a prebuilt binary exists
#   --version vX.Y.Z install that release instead of the latest
#   --service        also install and enable the ears-vad systemd user unit
#   --prefix DIR     where the binary goes (default ~/.local/bin)
#
# Environment: EARS_REPO (owner/name, default heiervang-technologies/ears).

set -eu

REPO=${EARS_REPO:-heiervang-technologies/ears}
BIN_DIR=${HOME}/.local/bin
YES=0 DEPS=1 FROM_SOURCE=0 VERSION="" SERVICE=0

while [ $# -gt 0 ]; do
    case $1 in
        --yes | -y) YES=1 ;;
        --no-deps) DEPS=0 ;;
        --from-source) FROM_SOURCE=1 ;;
        --version) VERSION=${2:?--version needs a tag}; shift ;;
        --service) SERVICE=1 ;;
        --prefix) BIN_DIR=${2:?--prefix needs a directory}; shift ;;
        -h | --help) sed -n '2,25p' "$0" 2>/dev/null || echo "see https://github.com/$REPO"; exit 0 ;;
        *) echo "ears install: unknown option $1" >&2; exit 2 ;;
    esac
    shift
done

say() { printf '%s\n' "$*"; }
warn() { printf 'ears install: %s\n' "$*" >&2; }
die() { warn "$*"; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# A yes/no question. Piped into sh, stdin is the script, so ask the terminal.
# No terminal (CI, ssh without -t): the answer is no unless --yes was given.
ask() {
    [ "$YES" = 1 ] && return 0
    [ -r /dev/tty ] && [ -w /dev/tty ] || return 1
    printf '%s [Y/n] ' "$1" >/dev/tty
    read -r reply </dev/tty || return 1
    case $reply in '' | y | Y | yes | Yes) return 0 ;; *) return 1 ;; esac
}

# --- where we are ------------------------------------------------------------

[ "$(uname -s)" = Linux ] || die "ears runs on Linux only (found $(uname -s))."

if [ -n "${TERMUX_VERSION:-}" ] || [ -d /data/data/com.termux ]; then
    PLATFORM=termux
else
    PLATFORM=linux
fi

case $(uname -m) in
    x86_64 | amd64) ARCH=x86_64 ;;
    aarch64 | arm64) ARCH=aarch64 ;;
    *) ARCH=$(uname -m); FROM_SOURCE=1 ;;
esac

PM=""
for pm in pacman apt-get dnf zypper apk pkg; do
    if have "$pm"; then PM=$pm; break; fi
done
[ "$PLATFORM" = termux ] && PM=pkg

SUDO=""
if [ "$(id -u)" != 0 ] && [ "$PLATFORM" != termux ]; then
    if have sudo; then SUDO=sudo; elif have doas; then SUDO=doas; fi
fi

# Where the script runs from: a checkout builds itself.
SRC=""
case $0 in
    */install.sh | install.sh)
        dir=$(cd "$(dirname "$0")" && pwd)
        [ -f "$dir/Cargo.toml" ] && grep -q '^name = "ears"' "$dir/Cargo.toml" && SRC=$dir ;;
esac

say "ears install: $PLATFORM $ARCH${PM:+, $PM}${SRC:+, from checkout $SRC}"

# --- runtime dependencies ----------------------------------------------------

# pkg_for COMMAND: the package that provides it under $PM ("" = none known).
pkg_for() {
    case $PM:$1 in
        pacman:pw-record) echo pipewire ;;
        apt-get:pw-record) echo pipewire-bin ;;
        dnf:pw-record) echo pipewire-utils ;;
        zypper:pw-record | apk:pw-record) echo pipewire-tools ;;
        pacman:notify-send | dnf:notify-send | apk:notify-send) echo libnotify ;;
        apt-get:notify-send) echo libnotify-bin ;;
        zypper:notify-send) echo libnotify-tools ;;
        pacman:paplay) echo libpulse ;;
        apt-get:paplay | dnf:paplay | zypper:paplay | apk:paplay) echo pulseaudio-utils ;;
        *:wtype) echo wtype ;;
        *:ydotool) echo ydotool ;;
        *:wl-copy) echo wl-clipboard ;;
        pacman:pkill | dnf:pkill | zypper:pkill) echo procps-ng ;;
        apt-get:pkill | apk:pkill) echo procps ;;
        pkg:curl) echo curl ;;
        *) echo "" ;;
    esac
}

# What ears runs, by what it is for. Typing: wtype on Wayland, ydotool
# (with the clipboard tools) elsewhere.
needed() {
    echo pw-record notify-send paplay pkill
    if [ -n "${WAYLAND_DISPLAY:-}" ]; then echo wtype; else echo ydotool; fi
}

install_packages() {
    case $PM in
        pacman) $SUDO pacman -S --needed --noconfirm "$@" ;;
        apt-get) $SUDO apt-get update -qq && $SUDO apt-get install -y -qq "$@" ;;
        dnf) $SUDO dnf install -y "$@" ;;
        zypper) $SUDO zypper --non-interactive install "$@" ;;
        apk) $SUDO apk add "$@" ;;
        pkg) pkg install -y "$@" ;;
        *) return 1 ;;
    esac
}

if [ "$DEPS" = 1 ] && [ "$PLATFORM" = linux ]; then
    missing="" pkgs="" unknown=""
    for cmd in $(needed); do
        have "$cmd" && continue
        missing="$missing $cmd"
        p=$(pkg_for "$cmd")
        if [ -n "$p" ]; then
            case " $pkgs " in *" $p "*) ;; *) pkgs="$pkgs $p" ;; esac
        else
            unknown="$unknown $cmd"
        fi
    done
    if [ -z "$missing" ]; then
        say "dependencies: all present"
    else
        say "missing:$missing"
        # shellcheck disable=SC2086
        if [ -n "$pkgs" ] && ask "Install$pkgs with $PM?"; then
            install_packages $pkgs || warn "package install failed; ears runs, but without$missing"
        elif [ -n "$pkgs" ]; then
            say "  install them with: $SUDO $PM install$pkgs"
        fi
        [ -n "$unknown" ] && say "  no package known for$unknown under ${PM:-this system}; install it by hand"
    fi
fi

# --- the binary --------------------------------------------------------------

mkdir -p "$BIN_DIR"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT INT TERM

# glibc_ok: the prebuilt binaries link glibc 2.35 (Ubuntu 22.04) or newer.
glibc_ok() {
    v=$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{print $2}')
    [ -n "$v" ] || return 1
    major=${v%%.*} minor=${v#*.}; minor=${minor%%.*}
    [ "$major" -gt 2 ] || { [ "$major" -eq 2 ] && [ "$minor" -ge 35 ]; }
}

fetch() { # URL FILE
    if have curl; then curl -fsSL --retry 3 -o "$2" "$1"
    elif have wget; then wget -qO "$2" "$1"
    else die "need curl or wget"; fi
}

prebuilt() {
    if [ -n "$VERSION" ]; then api="https://api.github.com/repos/$REPO/releases/tags/$VERSION"
    else api="https://api.github.com/repos/$REPO/releases/latest"; fi
    fetch "$api" "$TMP/release.json" || return 1
    tag=$(sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' "$TMP/release.json" | head -n1)
    # Newest naming first (ears-linux-x86_64.tar.gz), then the versioned one.
    url=$(grep -o '"browser_download_url": *"[^"]*linux-'"$ARCH"'\.tar\.gz"' "$TMP/release.json" \
        | sed 's/.*"\(https[^"]*\)"/\1/' | head -n1)
    [ -n "$url" ] || { warn "release ${tag:-?} has no build for linux-$ARCH"; return 1; }
    say "downloading ears $tag ($ARCH)"
    fetch "$url" "$TMP/ears.tar.gz" || return 1
    if fetch "$url.sha256" "$TMP/ears.tar.gz.sha256" 2>/dev/null; then
        want=$(awk '{print $1}' "$TMP/ears.tar.gz.sha256")
        got=$(sha256sum "$TMP/ears.tar.gz" | awk '{print $1}')
        [ "$want" = "$got" ] || die "checksum mismatch for $url"
    else
        warn "no checksum published for this release; not verified"
    fi
    tar -xzf "$TMP/ears.tar.gz" -C "$TMP"
    bin=$(find "$TMP" -type f -name ears | head -n1)
    [ -n "$bin" ] || die "no ears binary in $url"
    chmod +x "$bin"
    # An older release may link a newer glibc than this system has.
    "$bin" --version >/dev/null 2>&1 || { warn "the $tag build does not run here: $("$bin" --version 2>&1 | head -n1)"; return 1; }
    install -m755 "$bin" "$BIN_DIR/ears"
}

from_source() {
    if ! have cargo; then
        if [ "$PLATFORM" = termux ]; then
            ask "Building needs Rust. Install rust with pkg?" && pkg install -y rust || die "need cargo"
        else
            die "building ears from source needs Rust (cargo): https://rustup.rs"
        fi
    fi
    features=""
    # ort ships no ONNX Runtime for Android; Termux has its own.
    if [ "$PLATFORM" = termux ]; then
        [ -e "${PREFIX:-/data/data/com.termux/files/usr}/lib/libonnxruntime.so" ] || { ask "Install onnxruntime with pkg (voice detection needs it)?" && pkg install -y onnxruntime; } || true
        features="--features ort-dynamic"
    fi
    if [ -n "$SRC" ]; then
        say "building $SRC"
        # shellcheck disable=SC2086
        cargo install --locked --path "$SRC" --root "$TMP/root" $features
    else
        ref=${VERSION:+--tag $VERSION}
        say "building ears from github.com/$REPO ${VERSION:-main}"
        # shellcheck disable=SC2086
        cargo install --locked --git "https://github.com/$REPO" ${ref:---branch main} --root "$TMP/root" $features
    fi
    install -m755 "$TMP/root/bin/ears" "$BIN_DIR/ears"
}

if [ -n "$SRC" ] || [ "$FROM_SOURCE" = 1 ] || [ "$PLATFORM" = termux ] || ! glibc_ok; then
    from_source
elif ! prebuilt; then
    warn "falling back to building from source"
    from_source
fi

# Custom sounds from a checkout (the defaults are built into the binary).
if [ -n "$SRC" ] && [ -d "$SRC/sounds" ]; then
    mkdir -p "$HOME/.local/share/ears-sounds"
    cp "$SRC"/sounds/*.wav "$HOME/.local/share/ears-sounds/" 2>/dev/null || true
fi

say "installed $("$BIN_DIR/ears" --version 2>/dev/null || echo ears) to $BIN_DIR/ears"

# An older copy earlier in PATH (cargo's) would shadow this one.
found=$(command -v ears 2>/dev/null || true)
if [ -n "$found" ] && [ "$found" != "$BIN_DIR/ears" ]; then
    warn "$found comes first in PATH and is $("$found" --version 2>/dev/null || echo another build); remove it or reorder PATH"
fi
case ":$PATH:" in *":$BIN_DIR:"*) ;; *) say "add $BIN_DIR to PATH:  export PATH=\"$BIN_DIR:\$PATH\"" ;; esac

# --- optional: the systemd user unit -----------------------------------------

if [ "$SERVICE" = 1 ]; then
    unit="$HOME/.config/systemd/user/ears-vad.service"
    mkdir -p "$(dirname "$unit")"
    if [ -n "$SRC" ]; then cp "$SRC/contrib/systemd/ears-vad.service" "$TMP/unit"
    else fetch "https://raw.githubusercontent.com/$REPO/${VERSION:-main}/contrib/systemd/ears-vad.service" "$TMP/unit"; fi
    sed "s#^ExecStart=.*#ExecStart=$BIN_DIR/ears vad start#" "$TMP/unit" >"$unit"
    systemctl --user daemon-reload && systemctl --user enable --now ears-vad.service \
        && say "ears-vad.service enabled" || warn "could not enable ears-vad.service"
fi

cat <<EOF

Next:
  ears server http://your-asr-server:port   # whisper.cpp, vLLM Qwen3-ASR, or any OpenAI-compatible endpoint
  ears select                               # pick the microphone
  ears vad                                  # always-on voice typing (or bind \`ears toggle\` to a key)
More: https://github.com/$REPO#readme
EOF
