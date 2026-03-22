#!/usr/bin/env bash
set -euo pipefail

# Build and package per-platform VSIXs.
# For local dev: builds for current platform only.
# For CI: pass --all to build all platforms.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
EXT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
SPAR_ROOT="$(cd "$EXT_DIR/.." && pwd)"

TARGETS=("darwin-arm64")
if [[ "${1:-}" == "--all" ]]; then
  TARGETS=("darwin-arm64" "darwin-x64" "linux-x64" "linux-arm64" "win32-x64")
fi

# Map VS Code target to Rust target
declare -A RUST_TARGETS=(
  ["darwin-arm64"]="aarch64-apple-darwin"
  ["darwin-x64"]="x86_64-apple-darwin"
  ["linux-x64"]="x86_64-unknown-linux-gnu"
  ["linux-arm64"]="aarch64-unknown-linux-gnu"
  ["win32-x64"]="x86_64-pc-windows-msvc"
)

for target in "${TARGETS[@]}"; do
  rust_target="${RUST_TARGETS[$target]}"
  echo "==> Building spar for $target ($rust_target)..."

  binary_name="spar"
  if [[ "$target" == win32-* ]]; then
    binary_name="spar.exe"
  fi

  # Build (use cross for non-native targets)
  if [[ "$rust_target" == "$(rustc -vV | grep host | cut -d' ' -f2)" ]]; then
    cargo build --release --target "$rust_target" -p spar
  else
    echo "  Skipping non-native target $rust_target (use CI for cross builds)"
    continue
  fi

  # Copy binary to extension bin/
  mkdir -p "$EXT_DIR/bin"
  cp "$SPAR_ROOT/target/$rust_target/release/$binary_name" "$EXT_DIR/bin/$binary_name"
  chmod +x "$EXT_DIR/bin/$binary_name" 2>/dev/null || true

  echo "==> Packaging VSIX for $target..."
  cd "$EXT_DIR"
  npx @vscode/vsce package --target "$target" --no-dependencies
  cd "$SPAR_ROOT"

  echo "  Created: $EXT_DIR/spar-aadl-$(grep version "$EXT_DIR/package.json" | head -1 | grep -o '[0-9.]*')-$target.vsix"
done

echo "Done!"
