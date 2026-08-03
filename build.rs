fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=app.dev.manifest");
    println!("cargo:rerun-if-changed=assets/icons");
    let development_manifest = std::env::var_os("CARGO_FEATURE_DEV_MANIFEST").is_some()
        || std::env::var("PROFILE").is_ok_and(|profile| profile != "release");
    let parameters = if development_manifest {
        vec!["DEVELOPMENT_BUILD"]
    } else {
        Vec::new()
    };
    let compilation = embed_resource::compile_for("app.rc", ["AltTabio"], parameters);
    if let Err(error) = compilation.manifest_required() {
        panic!("AltTabio Windows resources are required: {error}");
    }
}
