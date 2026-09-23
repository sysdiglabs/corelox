//! Build-time support for `#[btf]` structs, meant to be used from build.rs.
//!
//! [`Builder`] generates a C header with CO-RE accessors for the structs,
//! [`Bindgen`] turns it into Rust bindings plus the BPF code behind them.

mod bindings;
mod error;
mod header;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use corelox_parser::BtfItem;
use corelox_parser::syn;

pub use bindgen;
pub use bindings::{Bindgen, OutputPaths, Prepared};
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
        generate_header(self.read_sources()?)
    }

    /// Generates the C header and writes it to `path`.
    pub fn write_header(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let path = path.as_ref();
        std::fs::write(path, self.generate_header()?).map_err(|source| Error::Io {
            path: path.to_owned(),
            source,
        })
    }

    /// Returns the paths and contents of all input files.
    fn read_sources(&self) -> Result<Vec<(PathBuf, String)>, Error> {
        let mut sources = Vec::new();
        for path in self.paths()? {
            let text = std::fs::read_to_string(&path).map_err(|source| Error::Io {
                path: path.clone(),
                source,
            })?;
            sources.push((path, text));
        }
        Ok(sources)
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

    /// The part of the header before the accessors
    fn definitions(header: &str) -> &str {
        header
            .split_once("\ntypedef ")
            .map_or(header, |(defs, _)| defs)
    }

    const PREAMBLE: &str = "\
/* Generated by corelox. Do not edit. */
#pragma once

#include <stdbool.h>
#include <linux/types.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_core_read.h>

/*
 * Calling these makes the verifier reject the program. The accessors only call
 * them when a field's Rust type doesn't match the running kernel's, which the
 * verifier knows after the CO-RE relocations (see the accessors).
 */
static void (*const corelox_size_mismatch)(void) = (void *)195900001;
static void (*const corelox_signedness_mismatch)(void) = (void *)195900002;
";

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
            PREAMBLE
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
            definitions(&header),
            PREAMBLE.to_owned()
                + "
struct Types;

struct Types {
\tunsigned char a;
\tlong long b;
\tunsigned long c;
\tbool d;
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
                    #[btf]
                    pub struct Inner {
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
            definitions(&header),
            PREAMBLE.to_owned()
                + "
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
    fn accessors() {
        let header = generate(&[(
            "a.rs",
            "#[btf] struct task { pid: i32, parent: *const task }",
        )])
        .unwrap();

        assert_eq!(
            header.strip_prefix(definitions(&header)).unwrap(),
            "
typedef __typeof__(((struct task *)0)->pid) co_re_task_pid_t;
static inline long co_re_task_pid(const void *p, co_re_task_pid_t *out)
{
\tif (__builtin_preserve_field_info(((typeof(struct task) *)0)->pid, BPF_FIELD_BYTE_SIZE) != sizeof(*out)) corelox_size_mismatch(); /* task::pid */
\tif (__builtin_preserve_field_info(((typeof(struct task) *)0)->pid, BPF_FIELD_SIGNED) != true) corelox_signedness_mismatch(); /* task::pid */
\treturn bpf_probe_read_kernel(out, sizeof(*out),
\t\t(const void *)__builtin_preserve_access_index(&((const struct task *)p)->pid));
}
static inline bool co_re_task_has_pid(void)
{
\treturn __builtin_preserve_field_info((((typeof(struct task) *)0)->pid), BPF_FIELD_EXISTS);
}

typedef const void *co_re_task_parent_t;
static inline long co_re_task_parent(const void *p, co_re_task_parent_t *out)
{
\tif (__builtin_preserve_field_info(((typeof(struct task) *)0)->parent, BPF_FIELD_BYTE_SIZE) != sizeof(*out)) corelox_size_mismatch(); /* task::parent */
\treturn bpf_probe_read_kernel(out, sizeof(*out),
\t\t(const void *)__builtin_preserve_access_index(&((const struct task *)p)->parent));
}
static inline bool co_re_task_has_parent(void)
{
\treturn __builtin_preserve_field_info((((typeof(struct task) *)0)->parent), BPF_FIELD_EXISTS);
}
"
        );
    }

    #[test]
    fn member_paths() {
        let header = generate(&[(
            "a.rs",
            r#"
            #[btf]
            struct task_struct {
                pid: i32,
                #[btf(path = "se.vruntime")]
                vruntime: u64,
                #[btf(path = "se.avg.load_avg")]
                load: c_ulong,
                #[btf(path = "se.on_rq")]
                on_rq: u32,
                #[btf(path = "comm")]
                name: [c_char; 16],
            }
            "#,
        )])
        .unwrap();

        assert_eq!(
            definitions(&header),
            PREAMBLE.to_owned()
                + "
struct task_struct;
struct task_struct__se__avg;
struct task_struct__se;

struct task_struct__se__avg {
\tunsigned long load_avg;
};

struct task_struct__se {
\tunsigned long long vruntime;
\tstruct task_struct__se__avg avg;
\tunsigned int on_rq;
};

struct task_struct {
\tint pid;
\tstruct task_struct__se se;
\tchar comm[16];
};
"
        );

        // accessors are named after the Rust fields, but access the C members
        for expected in [
            "typedef __typeof__(((struct task_struct *)0)->se.avg.load_avg) co_re_task_struct_load_t;",
            "static inline long co_re_task_struct_name(const void *p, co_re_task_struct_name_t *out)",
            "__builtin_preserve_access_index(&((const struct task_struct *)p)->comm)",
            "static inline bool co_re_task_struct_has_vruntime(void)",
            "(((typeof(struct task_struct) *)0)->se.vruntime), BPF_FIELD_EXISTS)",
        ] {
            assert!(header.contains(expected), "{expected}");
        }
        // generated nested structs don't get accessors
        assert!(!header.contains("co_re_task_struct__se"));

        // signedness is checked on the C member, and on the elements of arrays,
        // but not for `char`
        assert!(header.contains(
            "->se.avg.load_avg, BPF_FIELD_SIGNED) != false) corelox_signedness_mismatch(); /* task_struct::load */"
        ));
        assert!(!header.contains("->comm[0], BPF_FIELD_SIGNED"));
    }

    #[test]
    fn type_checks() {
        let header = generate(&[(
            "a.rs",
            "#[btf] struct s { a: [[u32; 2]; 3], b: *const u8, c: f64, d: core::ffi::c_long }",
        )])
        .unwrap();
        let checks: Vec<_> = header
            .lines()
            .filter(|line| line.contains("_mismatch();"))
            .map(|line| line.split_once("0)->").unwrap().1)
            .collect();
        assert_eq!(
            checks,
            [
                "a, BPF_FIELD_BYTE_SIZE) != sizeof(*out)) corelox_size_mismatch(); /* s::a */",
                "a[0][0], BPF_FIELD_SIGNED) != false) corelox_signedness_mismatch(); /* s::a */",
                "b, BPF_FIELD_BYTE_SIZE) != sizeof(*out)) corelox_size_mismatch(); /* s::b */",
                "c, BPF_FIELD_BYTE_SIZE) != sizeof(*out)) corelox_size_mismatch(); /* s::c */",
                "d, BPF_FIELD_BYTE_SIZE) != sizeof(*out)) corelox_size_mismatch(); /* s::d */",
                "d, BPF_FIELD_SIGNED) != true) corelox_signedness_mismatch(); /* s::d */",
            ]
        );
    }

    #[test]
    fn conflicting_paths() {
        let src = r#"#[btf] struct S {
    se: u32,
    #[btf(path = "se.x")]
    x: u32,
    #[btf(path = "se")]
    y: u32,
}
#[btf] struct T {
    #[btf(path = "a.b")]
    b: u32,
    a: u32,
}"#;
        assert_eq!(
            errors(&[("a.rs", src)]),
            [
                "a.rs:4:5: error: C member `se.x` conflicts with another field",
                "a.rs:6:5: error: C member `se` conflicts with another field",
                "a.rs:11:5: error: C member `a` conflicts with another field",
            ]
        );
    }

    #[test]
    fn invalid_path() {
        let src = "#[btf] struct S {\n    #[btf(path = \"a->b\")]\n    x: u32,\n}";
        assert_eq!(
            errors(&[("a.rs", src)]),
            ["a.rs:2:5: error: invalid path `a->b`: expected C identifiers separated by dots"]
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
