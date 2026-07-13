fn main() {
    let style = std::env::var("SLINT_STYLE").unwrap_or_else(|_| "fluent".into());
    let configuration = slint_build::CompilerConfiguration::new().with_style(style);
    slint_build::compile_with_config("ui/app.slint", configuration)
        .expect("failed to compile ui/app.slint");
    println!("cargo:rerun-if-changed=ui/app.slint");
    println!("cargo:rerun-if-env-changed=SLINT_STYLE");
}
