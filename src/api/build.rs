fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=../../frontend/dist");
    if !std::path::Path::new("../../frontend/dist/index.html").is_file() {
        return Err("Frontend assets are missing. Run `npm --prefix frontend ci` and `npm --prefix frontend run build` before building Rust.".into());
    }
    Ok(())
}
