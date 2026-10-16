#!/usr/bin/env bash
# Install the repository toolchain, with caller-selected components/targets.
set -euo pipefail
cd "$(dirname "$0")/../.."
toolchain=$(sed -n 's/^channel = "\([^"]*\)".*/\1/p' rust/rust-toolchain.toml)
test -n "$toolchain"
rustup toolchain install "$toolchain" --profile minimal "$@"
echo "RUSTUP_TOOLCHAIN=$toolchain" >> "$GITHUB_ENV"
