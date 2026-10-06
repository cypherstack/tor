import 'package:code_assets/code_assets.dart';
import 'package:hooks/hooks.dart';
import 'package:native_toolchain_rust/native_toolchain_rust.dart';

import 'prebuilt.dart';

Future<void> main(List<String> args) async {
  await build(args, (input, output) async {
    if (!input.config.buildCodeAssets) return;
    if (await usePrebuilt(input, output)) return;

    // Cargo's dep-info, which RustBuilder tracks, omits the vendored crates in
    // rust/patches, so declare the inputs the prebuilt fingerprint covers.
    output.dependencies.addAll(
      (await sourceFingerprint(input.packageRoot)).dependencies,
    );

    await const RustBuilder(
      assetName: 'tor_ffi_plugin_bindings_generated.dart',
      extraCargoBuildArgs: ['--locked'],
    ).run(input: input, output: output);
  });
}
