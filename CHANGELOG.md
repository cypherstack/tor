## 0.1.0

* Build the Rust core with Flutter native assets instead of cargokit.
  Requires Flutter 3.47.2 or later, `rustup` on the build machine, and
  NDK 27 or later for Android.
* Remove `NotSupportedPlatform`; `Tor.throwRustException` no longer takes
  a bindings argument.
* Fix `Tor.start` never detecting a failed `tor_start`.

## 0.0.1

* TODO: Describe initial release.
