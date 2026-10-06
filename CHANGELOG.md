## 0.2.0

* Bump arti to 2.6.0; needs Rust 1.91.
* Include the vendored crates in `rust/patches/` in the prebuilt source
  fingerprint, so editing them invalidates prebuilts and the hook cache.
* Leave Cargo output from building a vendored crate on its own (`target/` and
  `Cargo.lock`) out of the native source fingerprint.
* Fix `tor_client_bootstrap` and `tor_client_set_dormant` freeing the client
  handle Dart still holds. Add `tor_client_free`; `Tor.stop` now releases the
  client handle.
* Let the OS allocate the SOCKS port while Rust binds the real IPv4 listener,
  removing the probe-then-bind race. The same port is used for optional IPv6
  loopback and returned to Dart while the listeners remain reserved. The SOCKS
  accept loop starts only after bootstrap succeeds.
* **Breaking change for C callers:** `tor_start` returns
  `struct Tor { client, proxy, port }`. The SOCKS listeners are bound but idle
  until `tor_proxy_start(proxy)` is called after a successful bootstrap.
  Dart's `Tor.start` does this for you.
* `Tor.stop` resets the wrapper's state, so Tor can be started again.
* **Behavior change:** give bootstrap a configurable two-minute default
  deadline. Before this release `Tor.start` waited indefinitely for
  bootstrap; pass a longer `bootstrapTimeout` to keep waiting on slow
  networks. A `stop` issued during bootstrap cancels it promptly before
  releasing the native handles.
* Add `tor_client_cancel_bootstrap`. Cancellation is permanent for the
  handle; free it and call `tor_start` again to bootstrap anew.
* `tor_client_bootstrap` returns a `TorBootstrapStatus` code (ok, cancelled,
  timed out, failed, null client) instead of a bool, and Dart maps it without
  matching error text. C enumerators are prefixed with `TorBootstrapStatus_`
  to avoid collisions with application enums; their numeric values and Dart
  names are unchanged. The header also supports C++ callers. A passed deadline throws the new `TorBootstrapTimeout`
  instead of a plain `Exception`.
* Report Arti's full cause chain on bootstrap failure in the error message
  Dart receives, without repeating a cause already included as the exact
  suffix of its parent's message.
* A null client handle no longer aborts the process: `tor_client_bootstrap`
  returns `NullClient` and `tor_client_set_dormant` does nothing; both set
  the last error message.
* Concurrent `Tor.start` calls with the same raw data directory and timeout
  share one attempt and complete when Tor is ready, or fail together with the
  same error. Other starts queue behind it and either reuse the running
  instance or fail with a `StateError` for a different data directory.
* Release the native client and proxy when bootstrap fails during
  `Tor.start`, instead of leaving the proxy running. Preserve the bootstrap
  error and stack if cleanup also fails; print the secondary cleanup failure
  and expose it through `lastCleanupError` and `lastCleanupStackTrace`.
* `Tor.disable` now stops the proxy like `Tor.stop`; before, it only changed
  the reported status while traffic kept flowing. **API change:** it returns
  a `Future<void>` that can throw on release failure. Await it or attach an
  error handler when calling from a synchronous dispose method.
* Free the state and cache directory strings passed to `tor_start`.
* Add `tor_string_free` and use it to release Rust error messages after
  Dart copies them.
* Strip NUL bytes from Rust error messages instead of panicking across FFI.
* `Tor.stop` now shuts down the client's runtime, releasing its threads and
  its state and cache files, and completes once they are released. Shutdown
  waits up to five seconds for blocking storage work; a write still running
  after that may keep its file open briefly after the future completes. Before,
  every start leaked a client that kept the cache open, so the data directory could
  not be deleted on Windows. If the release itself fails, `stop` throws and
  the next `start` or `stop` retries it before opening a new client. If proxy
  release fails, retain its client too and retry the proxy before freeing it.
* `Tor.start` after an unawaited `Tor.stop` restarts Tor instead of returning
  at once and letting the queued stop leave it off. Starts always queue behind
  earlier calls; a start with a different data directory than the running
  instance fails with a `StateError` and leaves it running.
* `Tor.status` stays `on` while releasing a running instance, including when
  release fails and handles remain for retry. Failed startup cleanup stays
  `starting`; the next `start` or `stop` retries teardown before creating a new
  client. Await a successful `stop` before deleting the data directory.
  Status tracks lifecycle transitions and does not detect an unexpected SOCKS
  accept-loop exit after startup. `Tor.port` throws as soon as the proxy has stopped,
  with the message "Tor proxy is not running" instead of "Tor proxy port is
  unexpectedly null"; the type is still `Exception`.
* `tor_proxy_stop` waits for the SOCKS accept loop to exit before returning,
  so the listeners and its reference to the client are released, and Dart
  runs it on a worker isolate.
* Use the data directories literally; names containing `$` or `~` were
  expanded as variables and rejected.
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
* Reject bootstrap timeouts under one millisecond before starting the native
  client.
* `Tor.start` asynchronously creates the data directory and resolves its
  physical path inside the queued start before comparing it with the running
  instance. Paths through symbolic links or trailing separators reuse that
  instance, and on macOS `/var/...` and `/private/var/...` no longer conflict.
  When starting from off, status reports `starting` as soon as `start` is
  called, before the queued start runs.
  A path that cannot be created fails with the `FileSystemException` and restores
  status to `off` if no native handles remain.
* Route `Tor.hello` through the injectable native layer so test instances
  can substitute the greeting without calling the real library.
* Skip hidden files such as `.DS_Store` in the native source fingerprint and
  hash sources as bytes, so stray files no longer fail or invalidate builds.
* Avoid hashing all Rust inputs during source builds; full fingerprints remain
  required when selecting prebuilts.

## 0.1.0

* Build the Rust core with Flutter native assets instead of cargokit.
  Requires Flutter 3.47.2 or later, `rustup` on the build machine, and
  NDK 27 or later for Android.
* Remove `NotSupportedPlatform`; `Tor.throwRustException` no longer takes
  a bindings argument.
* Fix `Tor.start` never detecting a failed `tor_start`.

## 0.0.1

* TODO: Describe initial release.
