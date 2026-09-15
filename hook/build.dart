import 'package:code_assets/code_assets.dart';
import 'package:hooks/hooks.dart';
import 'package:native_toolchain_rust/native_toolchain_rust.dart';

import 'prebuilt.dart';

Future<void> main(List<String> args) async {
  await build(args, (input, output) async {
    if (!input.config.buildCodeAssets) return;
    if (await usePrebuilt(input, output)) return;

    output.dependencies.addAll([
      input.packageRoot.resolve('rust/Cargo.toml'),
      input.packageRoot.resolve('rust/Cargo.lock'),
      input.packageRoot.resolve('rust/rust-toolchain.toml'),
    ]);

    await const RustBuilder(
      assetName: 'tor_ffi_plugin_bindings_generated.dart',
      extraCargoBuildArgs: ['--locked'],
    ).run(input: input, output: output);
  });
}
