import 'dart:convert';
import 'dart:io';

import '../hook/prebuilt.dart';

// Run with dart --packages=.dart_tool/package_config.json to avoid build hooks.
Future<void> main(List<String> args) async {
  if (args.length != 1) {
    throw ArgumentError('Usage: prebuilt_manifest.dart <artifacts-directory>');
  }
  final directory = Directory(args.single).absolute;
  final artifacts = <String, Object>{};
  for (final target in releaseTargets.keys) {
    final file = File.fromUri(directory.uri.resolve(releaseFileName(target)));
    final size = await file.length();
    if (size <= 0 || size > 1024 * 1024 * 1024) {
      throw StateError('Invalid prebuilt size: ${file.path}');
    }
    artifacts[target] = {'sha256': await fileHash(file), 'size': size};
  }
  final source = await sourceFingerprint(Platform.script.resolve('../'));
  final manifest = File.fromUri(directory.uri.resolve('manifest.json'));
  final contents = {
    'schema_version': 1,
    'package': 'tor_ffi_plugin',
    'source_sha256': source.hash,
    'artifacts': artifacts,
  };
  await manifest.writeAsString(
    '${const JsonEncoder.withIndent('  ').convert(contents)}\n',
  );
  await File.fromUri(directory.uri.resolve('manifest.sha256'))
      .writeAsString('${await fileHash(manifest)}  manifest.json\n');
}
