//! Mapping Rust field types to C.

use std::collections::HashMap;

use corelox_parser::syn::ext::IdentExt;
use corelox_parser::syn::{self, Expr, Lit, PathArguments, Type};
use corelox_parser::types::{Primitive, PrimitiveKind, Signedness, primitive};

#[derive(Clone)]
pub(crate) enum CType {
    Primitive(Primitive),
    /// A struct, by name
    Struct(String),
    Pointer(Box<CType>),
    Array(Box<CType>, u64),
}

impl CType {
    /// Renders a C declaration of `name` with this type, e.g. `unsigned char *name[4]`.
    pub fn declaration(&self, name: &str) -> String {
        self.declare(name.to_owned())
    }

    pub fn is_pointer(&self) -> bool {
        matches!(self, CType::Pointer(_))
    }

    /// Returns which integer's signedness to check, relative to a member of
    /// this type (e.g. `[0]` for the elements of an array), and whether it
    /// should be signed. `None` if there's nothing to check.
    pub fn signedness_check(&self) -> Option<(String, bool)> {
        match self {
            CType::Primitive(p) if checks_signedness(p) => {
                Some((String::new(), p.signedness == Signedness::Signed))
            }
            CType::Primitive(_) | CType::Struct(_) | CType::Pointer(_) => None,
            CType::Array(inner, _) => inner
                .signedness_check()
                .map(|(index, signed)| (format!("[0]{index}"), signed)),
        }
    }

    fn declare(&self, name: String) -> String {
        match self {
            CType::Primitive(p) => format!("{} {name}", p.c_type),
            CType::Struct(s) => format!("struct {s} {name}"),
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

/// Whether to check the signedness of an integer.
///
/// Not for single bytes: the kernel's `char` looks just like an `unsigned char`
/// or a `signed char` in BTF, depending on the kernel (it's built with
/// `-funsigned-char` since 6.2), so e.g. `comm: [u8; 16]` couldn't work on
/// all kernels.
pub(crate) fn checks_signedness(p: &Primitive) -> bool {
    p.kind == PrimitiveKind::Integer && p.size > 1 && p.signedness != Signedness::Unchecked
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

                if let Some(primitive) = primitive(&p.path) {
                    if complete && primitive.kind == PrimitiveKind::Void {
                        return Err(syn::Error::new_spanned(
                            ty,
                            "`c_void` is only supported behind a pointer",
                        ));
                    }
                    Ok(CType::Primitive(primitive))
                } else if let Some(&idx) = self.known.get(&name) {
                    if complete {
                        self.deps.push(idx);
                    }
                    Ok(CType::Struct(name))
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
