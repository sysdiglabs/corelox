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

    corelox::Builder::new()
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
    let err = corelox::Builder::new()
        .files([dir.join("nope.rs")])
        .generate_header()
        .unwrap_err();
    assert!(matches!(err, corelox::Error::Io { .. }), "{err:?}");
}

#[test]
fn invalid_glob() {
    let err = corelox::Builder::new()
        .glob("[")
        .generate_header()
        .unwrap_err();
    assert!(matches!(err, corelox::Error::Glob { .. }), "{err:?}");
}

#[test]
fn bindgen() {
    let dir = tmp_dir("bindgen");
    fs::write(
        dir.join("src/a.rs"),
        r#"#[btf] struct task { pid: i32, parent: *const task, #[btf(path = "c")] comm: [u8; 16] }"#,
    )
    .unwrap();

    let paths = corelox::Bindgen::new(
        corelox::Builder::new().file(dir.join("src/a.rs")),
        dir.join("out"),
    )
    .generate()
    .unwrap();

    let bindings = fs::read_to_string(&paths.bindings).unwrap();
    for expected in [
        "pub type co_re_task_pid_t = core::ffi::c_int;",
        "pub type co_re_task_comm_t = [core::ffi::c_uchar; 16usize];",
        "#[link_name = \"co_re_task_parent__extern\"]",
        "pub fn co_re_task_parent(\n        p: *const core::ffi::c_void,\n        out: *mut co_re_task_parent_t,\n    ) -> core::ffi::c_long;",
        "pub fn co_re_task_has_comm() -> bool;",
    ] {
        assert!(bindings.contains(expected), "{expected}\n\n{bindings}");
    }
    // only the accessors, not the structs or anything from the included headers
    assert!(!bindings.contains("pub struct"), "{bindings}");
    assert!(!bindings.contains("bpf_probe_read_kernel"), "{bindings}");

    let wrappers = fs::read_to_string(&paths.wrappers).unwrap();
    assert!(wrappers.contains("co_re_task_parent__extern"), "{wrappers}");
    let bitcode = fs::read(&paths.wrappers_bitcode).unwrap();
    assert!(bitcode.starts_with(b"BC\xc0\xde"), "not LLVM bitcode");
}
