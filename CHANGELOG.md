## 0.2.0

* Bump arti to 2.6.0; needs Rust 1.91.
* Fix `tor_client_bootstrap` and `tor_client_set_dormant` freeing the client
  handle Dart still holds. Add `tor_client_free`; `Tor.stop` now releases the
  client handle.
* Bind the SOCKS listeners before `tor_start` returns, so a port collision
  fails `Tor.start` instead of leaving a dead proxy behind a published port.
* `Tor.stop` resets the wrapper's state, so Tor can be started again.
* `Tor.start` and `Tor.stop` run one at a time. A `stop` issued while a
  `start` is in progress waits for it and then tears it down.
* Concurrent `Tor.start` calls share one attempt and complete only when Tor is
  ready, or fail together with the same error.

## 0.1.0

* Build the Rust core with Flutter native assets instead of cargokit.
  Requires Flutter 3.47.2 or later, `rustup` on the build machine, and
  NDK 27 or later for Android.
* Remove `NotSupportedPlatform`; `Tor.throwRustException` no longer takes
  a bindings argument.
* Fix `Tor.start` never detecting a failed `tor_start`.

## 0.0.1

* TODO: Describe initial release.
