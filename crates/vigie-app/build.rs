const ICON: &str = "../../assets/brand/icons/vigie.ico";

fn main() {
    // The target OS, not the host's: `cfg!(target_os)` would describe the build script itself.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let style = match target_os.as_str() {
        "windows" => "fluent",
        "macos" => "cupertino",
        _ => "cosmic",
    };
    // French is bundled (decision 0008); source strings are English, without contexts.
    let config = slint_build::CompilerConfiguration::new()
        .with_style(style.into())
        .with_bundled_translations("translations")
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI compiles");

    // Explorer, the taskbar and Alt+Tab read the icon from the executable's resources.
    if target_os == "windows" {
        println!("cargo:rerun-if-changed={ICON}");
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon(ICON);
        resource.set("FileDescription", "Vigie");
        resource.set("ProductName", "Vigie");
        resource.compile().expect("Windows resources compile");
    }
}
