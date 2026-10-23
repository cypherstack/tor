<!--
SPDX-FileCopyrightText: 2022-2023 Foundation Devices Inc.

SPDX-License-Identifier: GPL-3.0-or-later
-->

# tor

[foundation-Devices/tor](https://github.com/Foundation-Devices/tor) is a multi-platform Flutter plugin for managing a Tor proxy.  Based on [arti](https://gitlab.torproject.org/tpo/core/arti).

## Getting started

Requires Flutter 3.47.2 or later. Nothing else is needed to use the package:
the build hook downloads the Rust library built for this version and checks
its hash before Flutter bundles it, so `flutter run` and `flutter build` work
as usual. See [Native library](#native-library) for building it yourself.

### Starting and stopping Tor

`Tor.start` completes after Tor has bootstrapped and the local SOCKS proxy is
ready. Bootstrap has a two-minute deadline by default; when it passes,
`Tor.start` throws `TorBootstrapTimeout`. Before 0.2.0 it waited
indefinitely. Callers can provide a different positive duration when needed:

```dart
final directory = await getApplicationSupportDirectory();
await Tor.instance.start(
  torDataDirPath: directory.path,
  bootstrapTimeout: const Duration(minutes: 3),
);
```

Starts and stops run one at a time in the order they were called, so
`stop()` followed by `start()` restarts Tor even when neither is awaited.
Concurrent starts with the same raw data directory and timeout share one
attempt. Other starts queue behind it and either reuse the running instance
or fail with a `StateError` for a different data directory, leaving it running.
The queued start creates and resolves the directory asynchronously before
comparing its physical path with the running instance. `Tor.stop()` and
`Tor.disable()` cancel an in-progress bootstrap, release the native client, and
complete after shutdown finishes. `Tor.status` stays `on` while releasing a
running instance, including when shutdown throws and resources remain for retry.
A failed start whose cleanup fails stays `starting`. The next `start()` or
`stop()` retries teardown before opening another client. Await a successful
`stop()` before deleting the data directory. Status tracks lifecycle
transitions; it does not detect an unexpected SOCKS accept-loop exit after
startup.
Native shutdown waits up to five seconds for blocking storage work; a longer
write may hold its file open briefly after the future completes.

`disable()` returns the same throwing `Future<void>` as `stop()`. Await it
where possible. A synchronous Flutter `dispose()` must handle its error
explicitly; `unawaited` alone does not catch failures:

```dart
unawaited(Tor.instance.disable().catchError((Object error, StackTrace stack) {
  debugPrint('Tor shutdown failed; retry stop before deleting its data: $error');
}));
```

If startup and its cleanup both fail, the original startup exception and stack
are preserved. The cleanup failure is printed with its stack and remains
available as `lastCleanupError` and `lastCleanupStackTrace` until the next native start attempt.

### Native library

The hook obtains the Rust library in one of three modes, chosen with the
`build` user define in the consuming app's root `pubspec.yaml` (the workspace
root for pub workspaces), or for this checkout with a `.dnix-build` file
holding the mode:

```yaml
hooks:
  user_defines:
    tor_ffi_plugin:
      build: prebuilt   # the default; or source, or dev
```

- `prebuilt` downloads the library of the release `dnix.lock` names and
  verifies it against the hash recorded there. A mismatch is an error, never
  a fallback. Downloads are cached by hash and need public HTTPS.
- `source` builds the library with Nix, the same build CI uses, and verifies
  it against the same hash. Needs Nix on Linux or an Apple Silicon Mac; the
  Apple targets need Xcode 27.0.
- `dev` builds with Nix but skips the Xcode pin and the hash check, for
  working on the Rust code. The hook notices a `.dnix-build` file from its
  next run on, so touch `dnix.json` after creating it.

Only dynamic libraries are provided, without sanitizers:

| Platform | Architectures | Minimum |
| --- | --- | --- |
| Android | armv7, arm64, x64 | API 24; supports 16 KiB pages |
| iOS | arm64 device; arm64/x64 simulator | iOS 13; arm64 simulator 14 |
| macOS | arm64, x64 | macOS 11 |
| Linux | x64, arm64 | glibc 2.28 |
| Windows | x64 | MSVC runtime |

The floors, the Rust and Xcode versions, and the location of the Rust
workspace live in `dnix.json`.

#### Releases

`dnix.lock` records the hash of every library the Rust source builds to and
names the GitHub release, `native-<release>`, that holds those bytes. After
changing the Rust code, run `dart tool/update_lock.dart` on a Mac with Nix
and Xcode 27.0 and commit the lock with the change; without a Mac, push and
copy the lock CI prints once every host is green. CI
(`.github/workflows/native.yml`) builds on four hosts, fails if any host or
the lock disagrees, and publishes the release when the lock reaches the
default branch. Nothing is committed by CI, and old releases stay.

Anyone can rebuild one platform with Nix and compare:
`dart tool/verify_release.dart linux` (or android, windows, macos, ios, or a
target triple), on Windows inside WSL2 for the non-Apple platforms. The build
comes from the shared [dnix](https://github.com/cypherstack/dnix) package,
whose documentation explains how the bytes stay identical across hosts.

## Development

To generate `tor_ffi_plugin.h` C bindings for Rust, `cargo build` in `rust` to produce headers according to `build.rs`.
To generate `tor_ffi_plugin_bindings_generated.dart` Dart bindings for C, `dart run ffigen --config ffigen.yaml`.

### Tests

Run the Rust tests with the pinned toolchain, then the offline Dart tests:

```sh
(cd rust && cargo test --locked)
flutter test
```

Before the lock's release is published, or to test local Rust changes, the
tests need the library built from source: `echo dev > .dnix-build` with Nix
installed (`source` to also check the hash).

The lifecycle regression also verifies stop/restart against the real Tor
network. Enable it explicitly (it requires network access and can take several
minutes):

```sh
flutter test --dart-define=TOR_NETWORK_TESTS=true
```

## Example app

`flutter run` in `example` to run the example app

See `example/lib/main.dart` for usage.
