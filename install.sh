#!/usr/bin/env bash
#
# ears installation script
#

set -euo pipefail

INSTALL_DIR="${HOME}/.local/bin"
CONFIG_DIR="${HOME}/.config/ears"
SOUNDS_DIR="${HOME}/.local/share/ears-sounds"

echo "Installing ears..."

# Create necessary directories
mkdir -p "$INSTALL_DIR"
mkdir -p "$CONFIG_DIR"
mkdir -p "$SOUNDS_DIR"

# Build Rust binary with all features
echo "Building Rust binary..."
cargo build --release --all-features

# Install the Rust binary
cp target/release/ears "$INSTALL_DIR/ears"
chmod +x "$INSTALL_DIR/ears"

echo "✓ Installed ears to $INSTALL_DIR/ears"

# Copy sound files if they exist
if [[ -d "sounds" ]] && [[ -n "$(ls -A sounds 2>/dev/null)" ]]; then
    cp sounds/* "$SOUNDS_DIR/"
    echo "✓ Installed sound files to $SOUNDS_DIR"
fi

# Check if ~/.local/bin is in PATH
if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    echo ""
    echo "WARNING: $INSTALL_DIR is not in your PATH"
    echo "Add this line to your ~/.bashrc or ~/.zshrc:"
    echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
fi

echo ""
echo "Installation complete!"
echo ""
echo "Next steps:"
echo "  1. Ensure you have a whisper.cpp server running"
echo "  2. Configure the server URL: ears server http://your-server:port"
echo "  3. Select your microphone: ears select"
echo "  4. Bind a keyboard shortcut to run: ears toggle"
echo ""
echo "For more information, see README.md"
