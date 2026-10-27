{
  description = "Reproducible reference builds of tor_ffi_plugin's Rust library";

  inputs.dnix.url = "github:cypherstack/dnix";

  outputs = { self, dnix, ... }: dnix.lib.package {
    src = self;
    config = ./dnix.json;
  };
}
