"""Load an exact artifact and check its null-safe proxy-stop entry point."""

import ctypes
import sys
from pathlib import Path

library = ctypes.CDLL(str(Path(sys.argv[1]).resolve(strict=True)))
library.tor_proxy_stop.argtypes = [ctypes.c_void_p]
library.tor_proxy_stop.restype = None
library.tor_proxy_stop(None)
print(f"Loaded native library: {sys.argv[1]}")
