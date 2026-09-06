fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=../../frontend/dist");
    println!("cargo:rerun-if-changed=../../proto/vmm.proto");
    if !std::path::Path::new("../../frontend/dist/index.html").is_file() {
        return Err("Frontend assets are missing. Run `npm --prefix frontend ci` and `npm --prefix frontend run build` before building Rust.".into());
    }
    tonic_prost_build::compile_protos("../../proto/vmm.proto")?;
    Ok(())
}
