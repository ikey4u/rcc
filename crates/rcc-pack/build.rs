fn main() {
    buildinfo::emit_git_version(
        "RCC_VERSION",
        &[
            ".",
            "../rcc-core",
            "../buildinfo",
            "../../Cargo.toml",
            "../../Cargo.lock",
        ],
    );
}
