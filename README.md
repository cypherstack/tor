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

## Example app

`flutter run` in `example` to run the example app

See `example/lib/main.dart` for usage.
