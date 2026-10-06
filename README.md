<!--
SPDX-FileCopyrightText: 2022-2023 Foundation Devices Inc.

SPDX-License-Identifier: GPL-3.0-or-later
-->

# tor

[foundation-Devices/tor](https://github.com/Foundation-Devices/tor) is a multi-platform Flutter plugin for managing a Tor proxy.  Based on [arti](https://gitlab.torproject.org/tpo/core/arti).

## Getting started

Source builds are the default. To use prebuilt libraries without installing Rust,
see [Prebuilt libraries](#prebuilt-libraries).

### [Install rust](https://www.rust-lang.org/tools/install)

Use `rustup`, not `homebrew`.  The toolchain and cross-compilation targets pinned in `rust/rust-toolchain.toml` are installed automatically on first build.

### Building

The Rust library is built and bundled by Flutter's native assets support (see `hook/build.dart`), so just `flutter run` or build as usual.  Requires Flutter 3.47.2 or later; Android builds need NDK 27 or later.

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

### Prebuilt libraries

Copy the manifest URL and SHA-256 from a native release into your application's
root `pubspec.yaml` (the workspace root for pub workspaces):

```yaml
hooks:
  user_defines:
    tor_ffi_plugin:
      native_build: prebuilt
      prebuilt_manifest_url: https://github.com/cypherstack/tor/releases/download/native-YOUR-RELEASE/manifest.json
      prebuilt_manifest_sha256: "REPLACE_WITH_RELEASE_MANIFEST_SHA256"
```

Use the package revision named in the release. The hook verifies the manifest,
native source fingerprint, and library size and checksum before bundling it.
Missing or invalid downloads fail the build. Downloads require public HTTPS.
Flutter can reuse the hook output until `flutter clean` removes it. The normal
Flutter application build tools are still required. Omit these settings or use
`native_build: source` to compile Rust locally.

Prebuilts use dynamic linking without sanitizers and cover:

| Platform | Architectures | Minimum |
| --- | --- | --- |
| Android | armv7, arm64, x64 | API 24; supports 16 KiB pages |
| iOS | arm64 device; arm64/x64 simulator | iOS 13; arm64 simulator 14 |
| macOS | arm64, x64 | macOS 11 |
| Linux | x64; arm64 | glibc 2.35 (x64); 2.39 (arm64) |
| Windows | x64 | MSVC runtime |

Push a new `native-*` tag (for example `native-0.1.0-1`) to build and publish
all targets. Release notes contain the hook settings above with the actual pin.
Existing releases are not overwritten. Manual workflow runs on branches only
upload artifacts. To generate the manifest locally, put the target-named
libraries in `artifacts/`, run `flutter pub get`, then:

```sh
dart --packages=.dart_tool/package_config.json tool/prebuilt_manifest.dart artifacts
```

## Development

To generate `tor_ffi_plugin.h` C bindings for Rust, `cargo build` in `rust` to produce headers according to `build.rs`.
To generate `tor_ffi_plugin_bindings_generated.dart` Dart bindings for C, `dart run ffigen --config ffigen.yaml`.

### Tests

Run the Rust tests with the pinned toolchain, then the offline Dart tests:

```sh
(cd rust && cargo test --locked)
flutter test
```

The lifecycle regression also verifies stop/restart against the real Tor
network. Enable it explicitly (it requires network access and can take several
minutes):

```sh
flutter test --dart-define=TOR_NETWORK_TESTS=true
```

## Example app

`flutter run` in `example` to run the example app

See `example/lib/main.dart` for usage.
