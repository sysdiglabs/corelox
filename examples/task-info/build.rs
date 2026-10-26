use std::path::PathBuf;

fn main() -> Result<(), corelox_build::Error> {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let header = corelox_build::Builder::new().glob("src/**/*.rs");
    corelox_build::Bindgen::new(header, out_dir.join("corelox")).generate()?;
    Ok(())
}
