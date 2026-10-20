import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

import '../hook/prebuilt.dart';

/// A minimal package tree containing every tracked native input.
Future<Directory> sourceTree() async {
  final root = await Directory.systemTemp.createTemp('tor-sources-');
  addTearDown(() => root.delete(recursive: true));
  for (final path in [
    'rust/Cargo.toml',
    'rust/Cargo.lock',
    'rust/rust-toolchain.toml',
    'rust/build.rs',
    'hook/build.dart',
    'lib/tor_ffi_plugin_bindings_generated.dart',
    'rust/src/lib.rs',
    'rust/patches/saturating-time/Cargo.toml',
    'rust/patches/saturating-time/src/lib.rs',
  ]) {
    final file = File.fromUri(root.uri.resolve(path));
    await file.parent.create(recursive: true);
    await file.writeAsString(path);
  }
  return root;
}

void main() {
  test('source dependencies are reusable without calculating hashes', () async {
    final root = await sourceTree();

    final dependencies = await sourceDependencies(root.uri);
    final fingerprint = await sourceFingerprint(root.uri);

    expect(dependencies, fingerprint.dependencies);
    expect(dependencies, contains(root.uri.resolve('rust/src/lib.rs')));
    expect(fingerprint.hash, hasLength(64));
  });

  test('a change to a vendored patch changes the fingerprint', () async {
    final root = await sourceTree();
    const patched = 'rust/patches/saturating-time/src/lib.rs';
    final before = await sourceFingerprint(root.uri);

    expect(before.dependencies, contains(root.uri.resolve(patched)));
    await File.fromUri(root.uri.resolve(patched)).writeAsString('changed');
    final after = await sourceFingerprint(root.uri);

    expect(after.hash, isNot(before.hash));
  });

  test('CRLF line endings do not change the fingerprint', () async {
    final root = await sourceTree();
    final file = File.fromUri(root.uri.resolve('rust/src/lib.rs'));
    await file.writeAsBytes('a\nb\r\nc\r'.codeUnits);
    final lf = await sourceFingerprint(root.uri);
    await file.writeAsBytes('a\r\nb\r\nc\r'.codeUnits);
    final crlf = await sourceFingerprint(root.uri);

    expect(crlf.hash, lf.hash);
  });

  test('cargo output beside a vendored patch is not an input', () async {
    final root = await sourceTree();
    final before = await sourceFingerprint(root.uri);

    // What `cargo test` leaves behind when run inside the vendored crate.
    const crate = 'rust/patches/saturating-time/';
    final target = Directory.fromUri(root.uri.resolve('${crate}target/debug/'));
    await target.create(recursive: true);
    await File.fromUri(target.uri.resolve('libsaturating_time.rlib'))
        .writeAsString('built');
    await File.fromUri(root.uri.resolve('${crate}Cargo.lock'))
        .writeAsString('# lock');
    final after = await sourceFingerprint(root.uri);

    expect(after.hash, before.hash);
    expect(after.dependencies, isNot(contains(target.uri)));
    // The package's own lockfile is still an input.
    expect(after.dependencies, contains(root.uri.resolve('rust/Cargo.lock')));
  });
}
