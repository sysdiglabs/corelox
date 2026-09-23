use std::path::PathBuf;

fn main() -> Result<(), btf_rs::Error> {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let header = btf_rs::Builder::new().glob("src/**/*.rs");
    btf_rs::Bindgen::new(header, out_dir.join("btf-rs")).generate()?;
    Ok(())
}
