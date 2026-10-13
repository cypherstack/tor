#!/usr/bin/env bash
# Run the Windows-only time, IPv6, and file release tests for $NATIVE_TARGET.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test --locked --manifest-path rust/Cargo.toml --target "$NATIVE_TARGET" --test saturating_time
cargo test --locked --manifest-path rust/Cargo.toml --target "$NATIVE_TARGET" --lib -- ipv6 closes_its_files
