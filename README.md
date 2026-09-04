<!--
SPDX-FileCopyrightText: 2022-2023 Foundation Devices Inc.

SPDX-License-Identifier: GPL-3.0-or-later
-->

# tor

[foundation-Devices/tor](https://github.com/Foundation-Devices/tor) is a multi-platform Flutter plugin for managing a Tor proxy.  Based on [arti](https://gitlab.torproject.org/tpo/core/arti).

## Getting started

### [Install rust](https://www.rust-lang.org/tools/install)

Use `rustup`, not `homebrew`.  The toolchain and cross-compilation targets pinned in `rust/rust-toolchain.toml` are installed automatically on first build.

### Building

The Rust library is built and bundled by Flutter's native assets support (see `hook/build.dart`), so just `flutter run` or build as usual.  Requires Flutter 3.47.2 or later; Android builds need NDK 27 or later.

## Development

To generate `tor_ffi_plugin.h` C bindings for Rust, `cargo build` in `rust` to produce headers according to `build.rs`.
To generate `tor_ffi_plugin_bindings_generated.dart` Dart bindings for C, `dart run ffigen --config ffigen.yaml`.

## Example app

`flutter run` in `example` to run the example app

See `example/lib/main.dart` for usage.
