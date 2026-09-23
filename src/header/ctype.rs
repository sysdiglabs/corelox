//! Mapping Rust field types to C.

use std::collections::HashMap;

use btf_rs_parser::syn::ext::IdentExt;
use btf_rs_parser::syn::{self, Expr, Lit, PathArguments, Type};

pub(crate) enum CType {
    Base(String),
    Pointer(Box<CType>),
    Array(Box<CType>, u64),
}

impl CType {
    /// Renders a C declaration of `name` with this type, e.g. `unsigned char *name[4]`.
    pub fn declaration(&self, name: &str) -> String {
        self.declare(name.to_owned())
    }

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

/// Maps Rust field types to C, recording which `#[btf]` structs are needed
/// as complete types.
pub(crate) struct TypeMapper<'a> {
    /// Indices of `#[btf]` structs by name
    pub known: &'a HashMap<String, usize>,
    /// Indices of structs that must be defined before the mapped types are used
    pub deps: Vec<usize>,
}

impl TypeMapper<'_> {
    pub fn map(&mut self, ty: &Type) -> syn::Result<CType> {
        self.map_inner(ty, true)
    }

    /// `complete` is set when the C type must be complete,
    /// i.e. for by-value fields and array elements.
    fn map_inner(&mut self, ty: &Type, complete: bool) -> syn::Result<CType> {
        match ty {
            Type::Paren(p) => self.map_inner(&p.elem, complete),
            Type::Group(g) => self.map_inner(&g.elem, complete),
            Type::Ptr(p) => Ok(CType::Pointer(Box::new(self.map_inner(&p.elem, false)?))),
            Type::Array(a) => {
                let Expr::Lit(lit) = &a.len else {
                    return Err(syn::Error::new_spanned(
                        &a.len,
                        "array length must be an integer literal",
                    ));
                };
                let Lit::Int(len) = &lit.lit else {
                    return Err(syn::Error::new_spanned(&a.len, "expected array length"));
                };
                let elem = self.map_inner(&a.elem, true)?;
                Ok(CType::Array(Box::new(elem), len.base10_parse()?))
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
                } else if let Some(&idx) = self.known.get(&name) {
                    if complete {
                        self.deps.push(idx);
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
}
