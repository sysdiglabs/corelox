use std::fs;
use std::path::PathBuf;

fn tmp_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src/sub")).unwrap();
    dir
}

#[test]
fn glob_and_files() {
    let dir = tmp_dir("glob_and_files");
    fs::write(dir.join("src/a.rs"), "#[btf] struct A { x: u32 }").unwrap();
    fs::write(dir.join("src/sub/b.rs"), "#[btf] struct B { a: A }").unwrap();
    fs::write(dir.join("src/sub/not_rust.txt"), "struct {").unwrap();
    let out = dir.join("btf.h");

    btf_rs::Builder::new()
        .file(dir.join("src/sub/b.rs"))
        // matches b.rs again, which must not produce a duplicate
        .glob(format!("{}/src/**/*.rs", dir.display()))
        .write_header(&out)
        .unwrap();

    let header = fs::read_to_string(out).unwrap();
    let (definitions, _) = header.split_once("\ntypedef ").unwrap();
    let (_, structs) = definitions.split_once("\n\nstruct ").unwrap();
    assert_eq!(
        structs,
        "\
B;
struct A;

struct A {
\tunsigned int x;
};

struct B {
\tstruct A a;
};
"
    );
}

#[test]
fn missing_file() {
    let dir = tmp_dir("missing_file");
    let err = btf_rs::Builder::new()
        .files([dir.join("nope.rs")])
        .generate_header()
        .unwrap_err();
    assert!(matches!(err, btf_rs::Error::Io { .. }), "{err:?}");
}

#[test]
fn invalid_glob() {
    let err = btf_rs::Builder::new()
        .glob("[")
        .generate_header()
        .unwrap_err();
    assert!(matches!(err, btf_rs::Error::Glob { .. }), "{err:?}");
}
