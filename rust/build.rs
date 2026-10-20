// SPDX-FileCopyrightText: 2023 Foundation Devices Inc.
//
// SPDX-License-Identifier: GPL-3.0-or-later

use cbindgen::{Config, EnumConfig, Language};
use glob::glob;
use std::env;
use std::path::PathBuf;

fn main() {
    android_on_linux_check();

    // Create C header files for Dart
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let package_name = env::var("CARGO_PKG_NAME").unwrap();

    let output_file = target_dir()
        .join(format!("{}.h", package_name))
        .display()
        .to_string();
    // List files rather than src itself: Cargo copies the directory into the
    // dep-info, where the Flutter hook runner takes it for a missing file and
    // reruns the hook on every build. Adding a module edits a listed file.
    for source in glob("src/**/*.rs").unwrap() {
        println!("cargo:rerun-if-changed={}", source.unwrap().display());
    }
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-env-changed=CARGO_TARGET_DIR");
    // android_on_linux_check reads these to find the NDK's clang runtime.
    println!("cargo:rerun-if-env-changed=ANDROID_NDK_HOME");
    println!("cargo:rerun-if-env-changed=CC_x86_64_linux_android");
    println!("cargo:rustc-env=TOR_FFI_PLUGIN_HEADER={}", output_file);

    let config = Config {
        language: Language::C,
        cpp_compat: true,
        enumeration: EnumConfig {
            prefix_with_name: true,
            ..Default::default()
        },
        ..Default::default()
    };

    cbindgen::generate_with_config(crate_dir, config)
        .unwrap()
        .write_to_file(output_file);
}

fn target_dir() -> PathBuf {
    if let Ok(target) = env::var("CARGO_TARGET_DIR") {
        PathBuf::from(target)
    } else {
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("target")
    }
}

fn android_on_linux_check() {
    let target = env::var("TARGET").unwrap();
    if target == "x86_64-linux-android" {
        let os = match env::consts::OS {
            "macos" => "darwin",
            "windows" => "windows",
            _ => "linux",
        };

        let ndk_home_result = env::var("ANDROID_NDK_HOME");
        let ndk_home = if let Ok(value) = ndk_home_result {
            value
        } else {
            println!("ANDROID_NDK_HOME not set. Trying CC_x86_64_linux_android");
            let path_to_parse_with_hack = env::var("CC_x86_64_linux_android")
                .expect("CC_x86_64_linux_android not set")
                .replace('\\', "/");

            path_to_parse_with_hack
                .split("/toolchains/")
                .next()
                .expect("Failed to parse path to get NDK home")
                .to_string()
        };

        let link_search_glob = format!(
            "{}/toolchains/llvm/prebuilt/{}-x86_64/lib/clang/**/lib/linux",
            ndk_home, os
        );

        let link_search_path = glob(&link_search_glob)
            .expect("failed to read link_search_glob")
            .next()
            .expect("failed to find link_search_path")
            .expect("link_search_path glob result failed");
        println!("cargo:rustc-link-lib=static=clang_rt.builtins-x86_64-android");
        println!("cargo:rustc-link-search={}", link_search_path.display());
    }
}
