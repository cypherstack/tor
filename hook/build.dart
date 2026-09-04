import 'package:hooks/hooks.dart';
import 'package:native_toolchain_rust/native_toolchain_rust.dart';

void main(List<String> args) async {
  await build(args, (input, output) async {
    output.dependencies.addAll([
      input.packageRoot.resolve('rust/Cargo.toml'),
      input.packageRoot.resolve('rust/Cargo.lock'),
      input.packageRoot.resolve('rust/rust-toolchain.toml'),
    ]);

    await const RustBuilder(
      assetName: 'tor_ffi_plugin_bindings_generated.dart',
    ).run(input: input, output: output);
  });
}
