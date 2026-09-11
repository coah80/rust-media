#[cfg(feature = "native")]
fn main() {
    slint_build::compile_with_config(
        "ui/player.slint",
        slint_build::CompilerConfiguration::new()
            .with_style("fluent-dark".into())
            .with_library_paths(std::collections::HashMap::from([(
                "lucide".into(),
                std::path::PathBuf::from(lucide_slint::lib()),
            )])),
    )
    .expect("compile native player");
}

#[cfg(not(feature = "native"))]
fn main() {}
