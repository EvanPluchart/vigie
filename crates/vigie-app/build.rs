fn main() {
    // The target OS, not the host's: `cfg!(target_os)` would describe the build script itself.
    let style = match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("windows") => "fluent",
        Ok("macos") => "cupertino",
        _ => "cosmic",
    };
    let config = slint_build::CompilerConfiguration::new().with_style(style.into());
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI compiles");
}
