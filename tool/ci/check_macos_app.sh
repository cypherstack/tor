#!/usr/bin/env bash
# Check the packaged macOS example's library; with a directory, also record exports, size, and load output there.
set -euo pipefail
cd "$(dirname "$0")/../.."
out=${1:-}
library=$(find -L example/build/macos/Build/Products/Release \
  -type f \
  -path '*/Contents/Frameworks/tor_ffi_plugin.framework/tor_ffi_plugin' \
  -print -quit)
test -n "$library"
if [ -n "$out" ]; then
  mkdir -p "$out"
  nm -gj "$library" > "$out/exports.txt"
  grep -Fx _tor_proxy_stop "$out/exports.txt"
  stat -f '%z' "$library" > "$out/size.txt"
  python3 tool/ci/smoke_native.py "$library" | tee "$out/load.txt"
else
  nm -gj "$library" | grep -Fx _tor_proxy_stop
  python3 tool/ci/smoke_native.py "$library"
fi
