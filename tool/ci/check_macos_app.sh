#!/usr/bin/env bash
# Check that the packaged macOS example's library exports and loads.
set -euo pipefail
cd "$(dirname "$0")/../.."
library=$(find -L example/build/macos/Build/Products/Release \
  -type f \
  -path '*/Contents/Frameworks/tor_ffi_plugin.framework/tor_ffi_plugin' \
  -print -quit)
test -n "$library"
nm -gj "$library" | grep -Fx _tor_proxy_stop
python3 tool/ci/smoke_native.py "$library"
