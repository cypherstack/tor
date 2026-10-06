## 0.2.0

* Bump arti to 2.6.0; needs Rust 1.91.
* Fix `tor_client_bootstrap` and `tor_client_set_dormant` freeing the client
  handle Dart still holds. Add `tor_client_free`; `Tor.stop` now releases the
  client handle.
* Let the OS allocate the SOCKS port while Rust binds the real IPv4 listener,
  removing the probe-then-bind race. The same port is used for optional IPv6
  loopback and returned to Dart while the listeners remain reserved.
* `Tor.stop` resets the wrapper's state, so Tor can be started again.
* `Tor.start` and `Tor.stop` run one at a time. A `stop` issued while a
  `start` is in progress waits for it and then tears it down.
* Concurrent `Tor.start` calls share one attempt and complete only when Tor is
  ready, or fail together with the same error.
* Release the native client and proxy when bootstrap fails during
  `Tor.start`, instead of leaving the proxy running.
* `Tor.disable` now stops the proxy like `Tor.stop`; before, it only changed
  the reported status while traffic kept flowing. **API change:** it returns
  a `Future<void>` that can throw on release failure. Await it or attach an
  error handler when calling from a synchronous dispose method.
* Free the state and cache directory strings passed to `tor_start`.
* When the OS assigns the SOCKS port and its IPv6 loopback twin is held by
  another process, try up to three more ports, then fail to start with an
  error saying the twin stayed occupied. Serving IPv4 alone there would let
  clients that resolve `localhost` to `::1` reach the other process. IPv4-only
  loopback is used only when IPv6 is unavailable on the host. A caller-requested
  non-zero port is not retried, and start fails if its IPv6 twin is held by
  another process. Dart always passes zero, so only direct C callers see this.
* Start on Windows hosts with IPv6 disabled. Winsock reports the missing
  address family as `WSAEAFNOSUPPORT`, which was not recognized, so
  `Tor.start` failed instead of serving IPv4 loopback.
* Route `Tor.hello` through the injectable native layer so test instances
  can substitute the greeting without calling the real library.

## 0.1.0

* Build the Rust core with Flutter native assets instead of cargokit.
  Requires Flutter 3.47.2 or later, `rustup` on the build machine, and
  NDK 27 or later for Android.
* Remove `NotSupportedPlatform`; `Tor.throwRustException` no longer takes
  a bindings argument.
* Fix `Tor.start` never detecting a failed `tor_start`.

## 0.0.1

* TODO: Describe initial release.
