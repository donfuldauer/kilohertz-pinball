#!/usr/bin/env bash
# Idempotent, non-interactive cloud environment setup for Kilohertz Pinball.
# Invoked automatically on fresh cloud VMs via .agents/environment.json.
#
# Installs and verifies every Linux tool needed across the repository:
#   - Pinned Rust toolchain from versions.toml [toolchain] + rustfmt, clippy,
#     llvm-tools-preview, and wasm32-unknown-unknown target
#   - cargo-llvm-cov (line coverage floor checks)
#   - SDL3 3.4.20 Linux video/audio/input dev headers + cmake/pkg-config
#   - Lavapipe software Vulkan driver (mesa-vulkan-drivers, libvulkan1,
#     vulkan-tools) and Xvfb/xauth for headless GPU and windowed tests
#   - Valgrind (callgrind tick-time benchmarks), jq, and Python 3
#   - .NET 8 SDK (tools/dof_bridge)
#   - Pre-fetched Cargo crate caches for all workspaces in the repo
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$REPO_ROOT"

CARGO_BIN_DIR="${CARGO_HOME:-$HOME/.cargo}/bin"
mkdir -p "$CARGO_BIN_DIR"
export PATH="$CARGO_BIN_DIR:$PATH"
export DOTNET_CLI_TELEMETRY_OPTOUT=1
export DOTNET_NOLOGO=1

# ---------------------------------------------------------------------------
# 1. Non-interactive ~/.bashrc persistence
# ---------------------------------------------------------------------------
# Non-interactive command shells source ~/.bashrc via BASH_ENV; unconditionally
# export Cargo's bin directory on line 1 before any interactive-shell guard or
# conditional without modifying or deleting existing lines or shell constructs.
touch "$HOME/.bashrc"

PATH_EXPORT="export PATH=\"${CARGO_BIN_DIR}:\$PATH\""
FIRST_LINE=$(head -n 1 "$HOME/.bashrc" || true)
if [ "$FIRST_LINE" != "$PATH_EXPORT" ]; then
  TMP_BASHRC=$(mktemp "$HOME/.bashrc.tmp.XXXXXX")
  trap 'rm -f "$TMP_BASHRC"' EXIT
  cp -p "$HOME/.bashrc" "$TMP_BASHRC"
  { printf '%s\n' "$PATH_EXPORT"; cat "$HOME/.bashrc"; } > "$TMP_BASHRC"
  mv "$TMP_BASHRC" "$HOME/.bashrc"
  trap - EXIT
fi

# ---------------------------------------------------------------------------
# 2. Linux system packages (SDL3 headers, lavapipe Vulkan, Xvfb, Valgrind,
#    Python 3, jq, .NET 8 SDK if absent)
# ---------------------------------------------------------------------------
if command -v apt-get >/dev/null 2>&1; then
  REQUIRED_APT_PKGS=(
    # C/C++ build toolchain for SDL3 (vendored) and Lua 5.4.9 (vendored)
    build-essential
    cmake
    pkg-config
    # Software Vulkan (lavapipe) and virtual X display for GPU / windowed tests
    mesa-vulkan-drivers
    libvulkan1
    vulkan-tools
    xvfb
    xauth
    # SDL3 3.4.20 Linux video, audio, and input backend dev headers
    # (exact list from versions.toml, docs/DEVELOPMENT.md, and .github/workflows/ci.yml)
    libx11-dev
    libxext-dev
    libxrandr-dev
    libxcursor-dev
    libxfixes-dev
    libxi-dev
    libxss-dev
    libxtst-dev
    libxkbcommon-dev
    libwayland-dev
    libegl-dev
    libdrm-dev
    libgbm-dev
    libudev-dev
    libdbus-1-dev
    libasound2-dev
    libpulse-dev
    # Tick-time benchmark (callgrind) and script utilities
    valgrind
    jq
    python3
    curl
    ca-certificates
  )

  # Ensure .NET 8 SDK is available for tools/dof_bridge
  DOTNET_SDKS=""
  if command -v dotnet >/dev/null 2>&1; then
    DOTNET_SDKS=$(dotnet --list-sdks 2>/dev/null || true)
  fi
  if ! grep -q '^8\.0\.' <<<"$DOTNET_SDKS"; then
    REQUIRED_APT_PKGS+=(dotnet-sdk-8.0)
  fi

  MISSING_PKGS=()
  for pkg in "${REQUIRED_APT_PKGS[@]}"; do
    PKG_STATUS=$(dpkg-query -W -f='${Status}' "$pkg" 2>/dev/null || true)
    if [ "$PKG_STATUS" != "install ok installed" ]; then
      MISSING_PKGS+=("$pkg")
    fi
  done

  if [ "${#MISSING_PKGS[@]}" -gt 0 ]; then
    SUDO=""
    if [ "$(id -u)" -ne 0 ]; then
      SUDO="sudo"
    fi
    echo "==> Installing missing apt packages: ${MISSING_PKGS[*]}"
    $SUDO apt-get update
    DEBIAN_FRONTEND=noninteractive $SUDO apt-get install -y --no-install-recommends "${MISSING_PKGS[@]}"
  else
    echo "==> All required apt packages are already installed."
  fi
fi

# ---------------------------------------------------------------------------
# 3. Pinned Rust toolchain from versions.toml [toolchain]
# ---------------------------------------------------------------------------
RUST_VERSION=$(sed -n '/^\[toolchain\]/,/^\[/{s/^rust *= *"\([^"]*\)".*/\1/p}' "$REPO_ROOT/versions.toml" | head -n 1)
if [ -z "$RUST_VERSION" ]; then
  echo "error: could not parse [toolchain] rust version from versions.toml" >&2
  exit 1
fi

if ! command -v rustup >/dev/null 2>&1; then
  echo "error: rustup not found on PATH ($PATH)" >&2
  exit 1
fi

CURRENT_RUSTC=$(rustc --version 2>/dev/null | awk '{print $2}' || true)
NEEDS_TOOLCHAIN=0
if [ "$CURRENT_RUSTC" != "$RUST_VERSION" ]; then
  NEEDS_TOOLCHAIN=1
else
  SYSROOT=$(rustc --print sysroot)
  if [ ! -d "$SYSROOT/lib/rustlib/wasm32-unknown-unknown" ]; then
    NEEDS_TOOLCHAIN=1
  fi
  INSTALLED_COMPONENTS=$(rustup component list --installed 2>/dev/null || true)
  for comp in rustfmt clippy llvm-tools; do
    if ! grep -q "^${comp}" <<<"$INSTALLED_COMPONENTS"; then
      NEEDS_TOOLCHAIN=1
    fi
  done
fi

if [ "$NEEDS_TOOLCHAIN" -eq 1 ]; then
  echo "==> Installing Rust $RUST_VERSION (rustfmt, clippy, llvm-tools-preview, wasm32-unknown-unknown)..."
  rustup toolchain install "$RUST_VERSION" --profile minimal \
    --component rustfmt,clippy,llvm-tools-preview \
    --target wasm32-unknown-unknown
  rustup default "$RUST_VERSION"
else
  echo "==> Rust $RUST_VERSION with required components and wasm32 target is already active."
fi

INSTALLED_RUSTC=$(rustc --version | awk '{print $2}')
if [ "$INSTALLED_RUSTC" != "$RUST_VERSION" ]; then
  echo "error: active rustc is $INSTALLED_RUSTC, expected $RUST_VERSION" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# 4. cargo-llvm-cov (for 85% / 60% crate coverage floor checks)
# ---------------------------------------------------------------------------
if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
  echo "==> Installing cargo-llvm-cov..."
  ARCH=$(uname -m)
  case "$ARCH" in
    x86_64)        LLVM_COV_TARGET="x86_64-unknown-linux-gnu" ;;
    aarch64|arm64) LLVM_COV_TARGET="aarch64-unknown-linux-gnu" ;;
    *)             LLVM_COV_TARGET="" ;;
  esac

  INSTALLED_PREBUILT=0
  if [ -n "$LLVM_COV_TARGET" ]; then
    TMP_DIR=$(mktemp -d)
    trap 'rm -rf "$TMP_DIR"' EXIT
    URL="https://github.com/taiki-e/cargo-llvm-cov/releases/latest/download/cargo-llvm-cov-${LLVM_COV_TARGET}.tar.gz"
    if curl -fsSL --retry 3 --retry-delay 2 "$URL" -o "$TMP_DIR/cargo-llvm-cov.tar.gz" \
      && tar -xzf "$TMP_DIR/cargo-llvm-cov.tar.gz" -C "$TMP_DIR" \
      && [ -f "$TMP_DIR/cargo-llvm-cov" ]; then
      install -m 0755 "$TMP_DIR/cargo-llvm-cov" "$CARGO_BIN_DIR/cargo-llvm-cov"
      INSTALLED_PREBUILT=1
    fi
    rm -rf "$TMP_DIR"
    trap - EXIT
  fi

  if [ "$INSTALLED_PREBUILT" -eq 0 ]; then
    cargo install --locked cargo-llvm-cov
  fi
else
  echo "==> cargo-llvm-cov is already installed ($(cargo llvm-cov --version))."
fi

# ---------------------------------------------------------------------------
# 5. Pre-fetch locked Cargo crate dependencies for all repo workspaces
# ---------------------------------------------------------------------------
echo "==> Fetching locked Cargo dependencies..."
cargo fetch --locked --manifest-path "$REPO_ROOT/Cargo.toml"
cargo fetch --locked --manifest-path "$REPO_ROOT/tests/spec_ci/Cargo.toml"
cargo fetch --locked --manifest-path "$REPO_ROOT/spikes/Cargo.toml"
cargo fetch --locked --manifest-path "$REPO_ROOT/spikes/spec_0_1/Cargo.toml"

# ---------------------------------------------------------------------------
# 6. Summary
# ---------------------------------------------------------------------------
echo "==> Environment ready:"
echo "    rustc:          $(rustc --version)"
echo "    cargo:          $(cargo --version)"
echo "    cargo-llvm-cov: $(cargo llvm-cov --version)"
echo "    wasm32 target:  $(rustc --print sysroot)/lib/rustlib/wasm32-unknown-unknown"
echo "    cmake:          $(cmake --version | head -n 1)"
echo "    xvfb-run:       $(command -v xvfb-run)"
echo "    valgrind:       $(valgrind --version)"
echo "    python3:        $(python3 --version)"
echo "    dotnet SDKs:    $(dotnet --list-sdks | tr '\n' ' ')"
