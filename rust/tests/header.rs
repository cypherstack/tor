//! Check that the public C ABI can coexist with application enum names.

#[cfg(unix)]
#[test]
fn header_enum_names_do_not_collide() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let header = std::path::Path::new(env!("TOR_FFI_PLUGIN_HEADER"));
    let source = format!(
        "enum ApplicationResult {{ Ok, Cancelled, TimedOut, Failed, NullClient }};\n\
         #include \"{}\"\n\
         static_assert(sizeof(TorBootstrapStatus) == sizeof(int32_t), \"ABI size\");\n\
         static_assert(TorBootstrapStatus_Ok == 0, \"ABI value\");\n\
         static_assert(TorBootstrapStatus_Cancelled == 1, \"ABI value\");\n\
         static_assert(TorBootstrapStatus_TimedOut == 2, \"ABI value\");\n\
         static_assert(TorBootstrapStatus_Failed == 3, \"ABI value\");\n\
         static_assert(TorBootstrapStatus_NullClient == 4, \"ABI value\");\n",
        header.display()
    );
    for (compiler, language, standard) in [("cc", "c", "c11"), ("c++", "c++", "c++11")] {
        let source = if language == "c" {
            source.replace("static_assert", "_Static_assert")
        } else {
            source.clone()
        };
        let mut child = Command::new(compiler)
            .args([
                "-x",
                language,
                &format!("-std={standard}"),
                "-fsyntax-only",
                "-",
            ])
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("install a C and C++ compiler to check the public header");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(source.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{language} header compilation failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
