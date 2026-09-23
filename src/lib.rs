//! Build-time support for `#[btf]` structs.
//!
//! Meant to be used from build.rs:
//!
//! ```no_run
//! let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
//! btf_rs::Builder::new()
//!     .glob("src/**/*.rs")
//!     .write_header(out_dir.join("btf.h"))
//!     .unwrap();
//! ```

mod error;
mod header;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use btf_rs_parser::BtfItem;
use btf_rs_parser::syn;

pub use error::Error;

use header::SourceStruct;

/// Collects `#[btf]` structs from Rust source files and generates a C header
/// with the corresponding struct definitions.
///
/// Files are scanned in the order they were added. Each file is only scanned
/// once, even if it's added several times (e.g. directly and via a glob).
/// Inline modules are scanned too, but `mod foo;` declarations aren't followed:
/// add those files separately.
#[derive(Debug, Clone, Default)]
pub struct Builder {
    inputs: Vec<Input>,
}

#[derive(Debug, Clone)]
enum Input {
    File(PathBuf),
    Glob(String),
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a Rust source file to scan.
    pub fn file(mut self, path: impl AsRef<Path>) -> Self {
        self.inputs.push(Input::File(path.as_ref().to_owned()));
        self
    }

    /// Adds Rust source files to scan.
    pub fn files<P: AsRef<Path>>(self, paths: impl IntoIterator<Item = P>) -> Self {
        paths.into_iter().fold(self, Self::file)
    }

    /// Adds all files matching a glob pattern, e.g. `"src/**/*.rs"`, to scan.
    pub fn glob(mut self, pattern: impl Into<String>) -> Self {
        self.inputs.push(Input::Glob(pattern.into()));
        self
    }

    /// Generates the C header.
    pub fn generate_header(&self) -> Result<String, Error> {
        let mut sources = Vec::new();
        for path in self.paths()? {
            let text = std::fs::read_to_string(&path).map_err(|source| Error::Io {
                path: path.clone(),
                source,
            })?;
            sources.push((path, text));
        }
        generate_header(sources)
    }

    /// Generates the C header and writes it to `path`.
    pub fn write_header(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let path = path.as_ref();
        std::fs::write(path, self.generate_header()?).map_err(|source| Error::Io {
            path: path.to_owned(),
            source,
        })
    }

    fn paths(&self) -> Result<Vec<PathBuf>, Error> {
        let mut seen = HashSet::new();
        let mut paths = Vec::new();
        let mut add = |path: PathBuf| {
            if seen.insert(path.clone()) {
                paths.push(path);
            }
        };

        for input in &self.inputs {
            match input {
                Input::File(path) => add(path.clone()),
                Input::Glob(pattern) => {
                    let entries = glob::glob(pattern).map_err(|e| Error::Glob {
                        pattern: pattern.clone(),
                        message: e.to_string(),
                    })?;
                    for entry in entries {
                        let path = entry.map_err(|e| Error::Io {
                            path: e.path().to_owned(),
                            source: e.into(),
                        })?;
                        if path.is_file() {
                            add(path);
                        }
                    }
                }
            }
        }
        Ok(paths)
    }
}

fn generate_header(sources: Vec<(PathBuf, String)>) -> Result<String, Error> {
    let mut structs = Vec::new();
    let mut errors = Vec::new();
    for (path, text) in sources {
        match syn::parse_file(&text) {
            Ok(file) => collect_structs(&path, &file.items, &mut structs, &mut errors),
            Err(e) => error::syn_messages(&path, e, &mut errors),
        }
    }
    if !errors.is_empty() {
        return Err(Error::Source(errors));
    }
    header::generate(&structs)
}

fn collect_structs(
    path: &Path,
    items: &[syn::Item],
    structs: &mut Vec<SourceStruct>,
    errors: &mut Vec<String>,
) {
    for item in items {
        match item {
            syn::Item::Struct(s) => match BtfItem::from_item(s) {
                Ok(Some(item)) => structs.push(SourceStruct {
                    path: path.to_owned(),
                    item,
                }),
                Ok(None) => {}
                Err(d) => error::diagnostic_messages(path, &d, errors),
            },
            syn::Item::Mod(syn::ItemMod {
                content: Some((_, items)),
                ..
            }) => collect_structs(path, items, structs, errors),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generate(sources: &[(&str, &str)]) -> Result<String, Error> {
        generate_header(
            sources
                .iter()
                .map(|(path, text)| (PathBuf::from(path), text.to_string()))
                .collect(),
        )
    }

    fn errors(sources: &[(&str, &str)]) -> Vec<String> {
        match generate(sources) {
            Err(Error::Source(errors)) => errors,
            other => panic!("expected source errors, got {other:?}"),
        }
    }

    #[test]
    fn empty() {
        assert_eq!(
            generate(&[("a.rs", "struct NotBtf { x: String }")]).unwrap(),
            "/* Generated by btf-rs. Do not edit. */\n#pragma once\n"
        );
    }

    #[test]
    fn types() {
        let header = generate(&[(
            "a.rs",
            r#"
            use core::ffi::{c_int, c_void};

            #[btf]
            struct Types {
                a: u8,
                b: i64,
                c: usize,
                d: bool,
                e: c_int,
                f: core::ffi::c_ulong,
                g: *const c_void,
                h: *mut *const u8,
                i: [u16; 4],
                j: [[u8; 2]; 3],
                k: [*const u8; 4],
                l: *const [u8; 4],
                r#type: u32,
            }
            "#,
        )])
        .unwrap();

        assert_eq!(
            header,
            "\
/* Generated by btf-rs. Do not edit. */
#pragma once

struct Types;

struct Types {
\tunsigned char a;
\tlong long b;
\tunsigned long c;
\t_Bool d;
\tint e;
\tunsigned long f;
\tvoid *g;
\tunsigned char **h;
\tunsigned short i[4];
\tunsigned char j[3][2];
\tunsigned char *k[4];
\tunsigned char (*l)[4];
\tunsigned int type;
};
"
        );
    }

    #[test]
    fn struct_refs_across_files() {
        let header = generate(&[
            (
                "a.rs",
                r#"
                #[btf]
                struct Outer {
                    inner: Inner,
                    inners: [crate::b::Inner; 2],
                    next: *const Outer,
                    prev: *const Later,
                }
                "#,
            ),
            (
                "b.rs",
                r#"
                mod inline {
                    #[btf(direct_access)]
                    pub struct Inner {
                        #[btf(path = "x.y")]
                        x: u32,
                    }
                }

                #[btf]
                struct Later {
                    outer: Outer,
                }
                "#,
            ),
        ])
        .unwrap();

        assert_eq!(
            header,
            "\
/* Generated by btf-rs. Do not edit. */
#pragma once

struct Outer;
struct Inner;
struct Later;

struct Inner {
\tunsigned int x;
};

struct Outer {
\tstruct Inner inner;
\tstruct Inner inners[2];
\tstruct Outer *next;
\tstruct Later *prev;
};

struct Later {
\tstruct Outer outer;
};
"
        );
    }

    #[test]
    fn unsupported_types() {
        let src = "#[btf]
struct Foo {
    a: String,
    b: Vec<u8>,
    c: &'static u8,
    d: c_void,
    e: [u8; N],
}";
        assert_eq!(
            errors(&[("a.rs", src)]),
            [
                "a.rs:3:8: error: unsupported type `String` (not a primitive or a #[btf] struct)",
                "a.rs:4:8: error: generic types are not supported",
                "a.rs:5:8: error: unsupported type",
                "a.rs:6:8: error: `c_void` is only supported behind a pointer",
                "a.rs:7:13: error: array length must be an integer literal",
            ]
        );
    }

    #[test]
    fn parser_errors_are_located() {
        let src = "#[btf]
struct Foo<T> {
    #[btf(pat = \"x\")]
    x: T,
}";
        assert_eq!(
            errors(&[("a.rs", src)]),
            [
                "a.rs:2:8: error: #[btf] structs cannot be generic",
                "a.rs:3:11: error: unknown argument `pat`",
                "a.rs:3:11: help: did you mean `path`?",
            ]
        );
    }

    #[test]
    fn errors_from_all_files() {
        assert_eq!(
            errors(&[("a.rs", "#[btf] struct Foo(u32);"), ("b.rs", "struct 1 {}"),]),
            [
                "a.rs:1:15: error: #[btf] structs must have named fields",
                "b.rs:1:8: error: expected identifier",
            ]
        );
    }

    #[test]
    fn duplicate_struct() {
        assert_eq!(
            errors(&[
                ("a.rs", "#[btf] struct Foo {}"),
                ("b.rs", "mod m { #[btf] struct Foo {} }"),
            ]),
            ["b.rs:1:23: error: duplicate #[btf] struct `Foo`, first defined at a.rs:1:15"]
        );
    }

    #[test]
    fn recursive_struct() {
        assert_eq!(
            errors(&[(
                "a.rs",
                "#[btf] struct A { b: B }\n#[btf] struct B { a: [A; 1] }"
            )]),
            ["a.rs:1:15: error: #[btf] struct `A` recursively contains itself"]
        );
    }
}
