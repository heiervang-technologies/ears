#!/usr/bin/env bash
# Cross-build ears for 32-bit ARM Alpine (armv7-unknown-linux-musleabihf),
# e.g. the Lenovo TB-7305F tablet ("pup"), from an x86_64 host.
#
#   scripts/build-armv7-musl.sh [OUT_DIR]
#
# ort has no prebuilt ONNX Runtime for this target, so the binary is built
# with `--features ort-dynamic`: it dlopens Alpine's community/onnxruntime
# (/usr/lib/libonnxruntime.so.1) at run time. That rules out a static musl
# binary (musl's static dlopen is a stub), so ears links dynamically against
# Alpine's musl and libgcc_s, from a sysroot taken out of the arm/v7 alpine
# image.
#
# Needs: rustup target armv7-unknown-linux-musleabihf, clang, lld, llvm-ar,
# docker with qemu-arm binfmt (only to fetch the sysroot, once).
# Target needs: apk add onnxruntime libgcc
# Check on the target, no microphone or sound involved:
#   ears --version && ears vad-file some-16k-speech.wav
set -euo pipefail

ALPINE="${ALPINE:-alpine:3.24}"
TARGET=armv7-unknown-linux-musleabihf
TRIPLE=armv7-alpine-linux-musleabihf
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-${CARGO_TARGET_DIR:-$ROOT/target}/armv7-musl}"
SYSROOT="$OUT/sysroot"
mkdir -p "$OUT"

if [[ ! -e "$SYSROOT/usr/lib/libonnxruntime.so.1" ]]; then
  echo "fetching the $ALPINE armv7 sysroot (musl, libgcc, onnxruntime)..."
  rm -rf "$SYSROOT"
  docker run --rm --platform linux/arm/v7 -v "$OUT:/out" "$ALPINE" sh -c \
    'apk add -q musl-dev gcc libgcc onnxruntime && mkdir -p /out/sysroot && cp -a /lib /usr /out/sysroot/'
fi
GCC_DIR="$(find "$SYSROOT/usr/lib/gcc/$TRIPLE" -mindepth 1 -maxdepth 1 -type d | head -1)"

# rustc picks the linker flavour from the file name: not "ld*"
cat >"$OUT/armv7-clang-cc" <<EOF
#!/bin/sh
exec clang --target=$TRIPLE --sysroot=$SYSROOT -march=armv7-a -mfpu=vfpv3-d16 -mfloat-abi=hard "\$@"
EOF
cat >"$OUT/armv7-clang-link" <<EOF
#!/bin/sh
exec clang --target=$TRIPLE --sysroot=$SYSROOT -fuse-ld=lld -B$GCC_DIR -L$GCC_DIR -rtlib=libgcc --unwindlib=libgcc "\$@"
EOF
chmod +x "$OUT/armv7-clang-cc" "$OUT/armv7-clang-link"

cd "$ROOT"
# ORT_LIB_LOCATION: with load-dynamic, ort-sys would otherwise still try to
# download a runtime (voice_activity_detector keeps ort's default features)
CARGO_TARGET_DIR="$OUT/target" \
ORT_LIB_LOCATION="$SYSROOT/usr/lib" \
CC_armv7_unknown_linux_musleabihf="$OUT/armv7-clang-cc" \
AR_armv7_unknown_linux_musleabihf=llvm-ar \
CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER="$OUT/armv7-clang-link" \
CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_RUSTFLAGS="-C target-feature=-crt-static -C link-self-contained=no" \
  cargo build --release --locked --target "$TARGET" --features ort-dynamic

BIN="$OUT/target/$TARGET/release/ears"
file "$BIN" 2>/dev/null || true
echo "$BIN"
