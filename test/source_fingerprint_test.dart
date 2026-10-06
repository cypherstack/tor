import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

import '../hook/prebuilt.dart';

void main() {
  test('source dependencies are reusable without calculating hashes', () async {
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
    ]) {
      final file = File.fromUri(root.uri.resolve(path));
      await file.parent.create(recursive: true);
      await file.writeAsString(path);
    }

    final dependencies = await sourceDependencies(root.uri);
    final fingerprint = await sourceFingerprint(root.uri);

    expect(dependencies, fingerprint.dependencies);
    expect(dependencies, contains(root.uri.resolve('rust/src/lib.rs')));
    expect(fingerprint.hash, hasLength(64));
  });

  test('CRLF line endings do not change the fingerprint', () async {
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
    ]) {
      final file = File.fromUri(root.uri.resolve(path));
      await file.parent.create(recursive: true);
      await file.writeAsString(path);
    }
    final file = File.fromUri(root.uri.resolve('rust/src/lib.rs'));
    await file.writeAsBytes('a\nb\r\nc\r'.codeUnits);
    final lf = await sourceFingerprint(root.uri);
    await file.writeAsBytes('a\r\nb\r\nc\r'.codeUnits);
    final crlf = await sourceFingerprint(root.uri);

    expect(crlf.hash, lf.hash);
  });
}
