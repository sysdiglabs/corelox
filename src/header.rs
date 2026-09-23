use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt::Write;
use std::path::PathBuf;

use btf_rs_parser::BtfItem;
use btf_rs_parser::syn::ext::IdentExt;
use btf_rs_parser::syn::{self, Expr, Lit, PathArguments, Type};

use crate::error::{self, Error, location};

/// A `#[btf]` struct along with the file it was found in.
pub(crate) struct SourceStruct {
    pub path: PathBuf,
    pub item: BtfItem,
}

enum CType {
    Base(String),
    Pointer(Box<CType>),
    Array(Box<CType>, u64),
}

impl CType {
    /// Renders a C declaration of `name` with this type.
    fn declare(&self, name: String) -> String {
        match self {
            CType::Base(base) => format!("{base} {name}"),
            CType::Pointer(inner) => inner.declare(format!("*{name}")),
            CType::Array(inner, len) => {
                // `*p[N]` is an array of pointers, a pointer to an array needs `(*p)[N]`
                let name = if name.starts_with('*') {
                    format!("({name})")
                } else {
                    name
                };
                inner.declare(format!("{name}[{len}]"))
            }
        }
    }
}

struct CStruct {
    name: String,
    fields: Vec<(String, CType)>,
    /// Indices of structs that must be defined before this one
    deps: Vec<usize>,
}

fn rust_primitive(name: &str) -> Option<&'static str> {
    Some(match name {
        "u8" => "unsigned char",
        "u16" => "unsigned short",
        "u32" => "unsigned int",
        "u64" => "unsigned long long",
        "u128" => "unsigned __int128",
        "usize" => "unsigned long",
        "i8" => "signed char",
        "i16" => "short",
        "i32" => "int",
        "i64" => "long long",
        "i128" => "__int128",
        "isize" => "long",
        "bool" => "_Bool",
        "f32" => "float",
        "f64" => "double",
        _ => return None,
    })
}

/// `core::ffi` types, matched by name regardless of the path they're used through
fn c_type(name: &str) -> Option<&'static str> {
    Some(match name {
        "c_char" => "char",
        "c_schar" => "signed char",
        "c_uchar" => "unsigned char",
        "c_short" => "short",
        "c_ushort" => "unsigned short",
        "c_int" => "int",
        "c_uint" => "unsigned int",
        "c_long" => "long",
        "c_ulong" => "unsigned long",
        "c_longlong" => "long long",
        "c_ulonglong" => "unsigned long long",
        "c_float" => "float",
        "c_double" => "double",
        "c_void" => "void",
        _ => return None,
    })
}

/// Maps a Rust field type to C. `complete` is set when the C type must be
/// complete, i.e. for by-value fields and array elements.
fn map_type(
    ty: &Type,
    known: &HashMap<String, usize>,
    deps: &mut Vec<usize>,
    complete: bool,
) -> syn::Result<CType> {
    match ty {
        Type::Paren(p) => map_type(&p.elem, known, deps, complete),
        Type::Group(g) => map_type(&g.elem, known, deps, complete),
        Type::Ptr(p) => Ok(CType::Pointer(Box::new(map_type(
            &p.elem, known, deps, false,
        )?))),
        Type::Array(a) => {
            let len = match &a.len {
                Expr::Lit(lit) => match &lit.lit {
                    Lit::Int(int) => int.base10_parse()?,
                    _ => return Err(syn::Error::new_spanned(&a.len, "expected array length")),
                },
                _ => {
                    return Err(syn::Error::new_spanned(
                        &a.len,
                        "array length must be an integer literal",
                    ));
                }
            };
            Ok(CType::Array(
                Box::new(map_type(&a.elem, known, deps, true)?),
                len,
            ))
        }
        Type::Path(p) if p.qself.is_none() => {
            let last = p.path.segments.last().expect("non-empty path");
            if !matches!(last.arguments, PathArguments::None) {
                return Err(syn::Error::new_spanned(
                    ty,
                    "generic types are not supported",
                ));
            }
            let name = last.ident.unraw().to_string();
            let single = p.path.leading_colon.is_none() && p.path.segments.len() == 1;

            if single && let Some(c) = rust_primitive(&name) {
                Ok(CType::Base(c.to_owned()))
            } else if let Some(c) = c_type(&name) {
                if complete && c == "void" {
                    return Err(syn::Error::new_spanned(
                        ty,
                        "`c_void` is only supported behind a pointer",
                    ));
                }
                Ok(CType::Base(c.to_owned()))
            } else if let Some(&idx) = known.get(&name) {
                if complete {
                    deps.push(idx);
                }
                Ok(CType::Base(format!("struct {name}")))
            } else {
                Err(syn::Error::new_spanned(
                    ty,
                    format!("unsupported type `{name}` (not a primitive or a #[btf] struct)"),
                ))
            }
        }
        _ => Err(syn::Error::new_spanned(ty, "unsupported type")),
    }
}

/// Returns struct indices ordered so that each struct comes after the ones
/// it contains by value, or the index of a struct that contains itself.
fn definition_order(structs: &[CStruct]) -> Result<Vec<usize>, usize> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        New,
        Visiting,
        Done,
    }

    fn visit(
        idx: usize,
        structs: &[CStruct],
        state: &mut [State],
        order: &mut Vec<usize>,
    ) -> Result<(), usize> {
        match state[idx] {
            State::Done => return Ok(()),
            State::Visiting => return Err(idx),
            State::New => {}
        }
        state[idx] = State::Visiting;
        for &dep in &structs[idx].deps {
            visit(dep, structs, state, order)?;
        }
        state[idx] = State::Done;
        order.push(idx);
        Ok(())
    }

    let mut state = vec![State::New; structs.len()];
    let mut order = Vec::with_capacity(structs.len());
    for idx in 0..structs.len() {
        visit(idx, structs, &mut state, &mut order)?;
    }
    Ok(order)
}

pub(crate) fn generate(structs: &[SourceStruct]) -> Result<String, Error> {
    let mut errors = Vec::new();

    let mut known: HashMap<String, usize> = HashMap::new();
    for (idx, s) in structs.iter().enumerate() {
        match known.entry(s.item.ident.unraw().to_string()) {
            Entry::Occupied(e) => {
                let first = &structs[*e.get()];
                errors.push(format!(
                    "{}: error: duplicate #[btf] struct `{}`, first defined at {}",
                    location(&s.path, s.item.ident.span()),
                    e.key(),
                    location(&first.path, first.item.ident.span()),
                ));
            }
            Entry::Vacant(e) => {
                e.insert(idx);
            }
        }
    }

    let mut cstructs = Vec::with_capacity(structs.len());
    for s in structs {
        let mut deps = Vec::new();
        let mut fields = Vec::new();
        for field in &s.item.fields {
            match map_type(&field.ty, &known, &mut deps, true) {
                Ok(ty) => fields.push((field.ident.unraw().to_string(), ty)),
                Err(e) => error::syn_messages(&s.path, e, &mut errors),
            }
        }
        cstructs.push(CStruct {
            name: s.item.ident.unraw().to_string(),
            fields,
            deps,
        });
    }

    if !errors.is_empty() {
        return Err(Error::Source(errors));
    }

    let order = definition_order(&cstructs).map_err(|idx| {
        let s = &structs[idx];
        Error::Source(vec![format!(
            "{}: error: #[btf] struct `{}` recursively contains itself",
            location(&s.path, s.item.ident.span()),
            cstructs[idx].name,
        )])
    })?;

    let mut out = String::from("/* Generated by btf-rs. Do not edit. */\n#pragma once\n");
    if !cstructs.is_empty() {
        out.push('\n');
    }
    for s in &cstructs {
        writeln!(out, "struct {};", s.name).unwrap();
    }
    for idx in order {
        let s = &cstructs[idx];
        writeln!(out, "\nstruct {} {{", s.name).unwrap();
        for (name, ty) in &s.fields {
            writeln!(out, "\t{};", ty.declare(name.clone())).unwrap();
        }
        out.push_str("};\n");
    }
    Ok(out)
}
