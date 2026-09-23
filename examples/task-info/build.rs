use std::path::PathBuf;

fn main() -> Result<(), corelox::Error> {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let header = corelox::Builder::new().glob("src/**/*.rs");
    corelox::Bindgen::new(header, out_dir.join("corelox")).generate()?;
    Ok(())
}
