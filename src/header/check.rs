//! Checking `#[btf]` field types against a kernel's BTF.
//!
//! This is the build-time counterpart of the checks in the accessors, which
//! reject the program at load time on the kernel it's loaded on. It only
//! knows about one kernel, but reports mismatches with source locations.

use btf_rs::{Btf, Member, Struct, Type};
use corelox_parser::syn::ext::IdentExt;
use corelox_parser::types::{PrimitiveKind, Signedness};

use super::ctype::{CType, checks_signedness};
use super::{Accessor, SourceStruct};
use crate::error::location;

/// Checks the fields of `s` against the kernel struct of the same name.
/// Mismatches are errors, and missing structs or members only warnings:
/// they may exist in other kernels.
pub(super) fn check_struct(
    btf: &Btf,
    s: &SourceStruct,
    accessors: &[Accessor],
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> Result<(), btf_rs::Error> {
    let name = s.item.ident.unraw().to_string();
    let Some(kernel) = find_struct(btf, &name)? else {
        warnings.push(format!(
            "{}: the kernel has no `struct {name}`, so its fields can't be checked",
            location(&s.path, s.item.ident.span()),
        ));
        return Ok(());
    };

    // accessors are in the same order as the fields
    for (field, accessor) in s.item.fields.iter().zip(accessors) {
        let path: Vec<_> = accessor.member.split('.').collect();
        let loc = location(&s.path, field.ident.span());
        let field = format!("{name}::{}", field.ident.unraw());
        match check_member(btf, &kernel, &path, &accessor.ty)? {
            Ok(()) => {}
            Err(Finding::Missing(member)) => warnings.push(format!(
                "{loc}: `{field}`: the kernel's `struct {name}` has no member `{member}`"
            )),
            Err(Finding::Mismatch(reason)) => {
                errors.push(format!("{loc}: error: `{field}`: {reason}"))
            }
        }
    }
    Ok(())
}

enum Finding {
    /// The member, as far as it could be found
    Missing(String),
    Mismatch(String),
}

fn find_struct(btf: &Btf, name: &str) -> Result<Option<Struct>, btf_rs::Error> {
    Ok(btf
        .resolve_types_by_name(name)?
        .into_iter()
        .find_map(|t| match t {
            Type::Struct(s) => Some(s),
            _ => None,
        }))
}

fn check_member(
    btf: &Btf,
    kernel: &Struct,
    path: &[&str],
    ty: &CType,
) -> Result<Result<(), Finding>, btf_rs::Error> {
    let mut current = kernel.clone();
    let mut found = None;
    for (i, name) in path.iter().enumerate() {
        let member_path = path[..=i].join(".");
        let Some(member) = find_member(btf, &current, name)? else {
            return Ok(Err(Finding::Missing(member_path)));
        };
        if i + 1 < path.len() {
            let (typedef, t) = skip_modifiers(btf, btf.resolve_chained_type(&member)?)?;
            match t {
                Type::Struct(s) | Type::Union(s) => current = s,
                t => {
                    return Ok(Err(Finding::Mismatch(format!(
                        "the kernel's `{member_path}` is {}, not a struct or union",
                        describe_kernel(btf, typedef.as_deref(), &t)?
                    ))));
                }
            }
        }
        found = Some(member);
    }
    let member = found.expect("non-empty path");
    let member_path = path.join(".");

    if member.bitfield_size().is_some_and(|size| size > 0) {
        return Ok(Err(Finding::Mismatch(format!(
            "the kernel's `{member_path}` is a bitfield, which isn't supported"
        ))));
    }
    let subject = Subject {
        member: member_path,
        elements: false,
    };
    Ok(compare(btf, btf.resolve_chained_type(&member)?, ty, &subject)?.map_err(Finding::Mismatch))
}

/// Finds a member by name, also in anonymous structs and unions, like libbpf does.
fn find_member(btf: &Btf, s: &Struct, name: &str) -> Result<Option<Member>, btf_rs::Error> {
    for member in &s.members {
        let member_name = btf.resolve_name(member)?;
        if member_name == name {
            return Ok(Some(member.clone()));
        }
        if member_name.is_empty()
            && let (_, Type::Struct(inner) | Type::Union(inner)) =
                skip_modifiers(btf, btf.resolve_chained_type(member)?)?
            && let Some(found) = find_member(btf, &inner, name)?
        {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

/// Resolves typedefs and qualifiers, returning the outermost typedef's name, if any.
fn skip_modifiers(btf: &Btf, mut t: Type) -> Result<(Option<String>, Type), btf_rs::Error> {
    let mut typedef = None;
    loop {
        t = match &t {
            Type::Typedef(x) => {
                if typedef.is_none() {
                    typedef = Some(btf.resolve_name(x)?);
                }
                btf.resolve_chained_type(x)?
            }
            Type::Const(x) | Type::Volatile(x) | Type::Restrict(x) => {
                btf.resolve_chained_type(x)?
            }
            Type::TypeTag(x) => btf.resolve_chained_type(x)?,
            _ => return Ok((typedef, t)),
        };
    }
}

/// What's being compared, for messages.
struct Subject {
    member: String,
    /// The elements of the (possibly nested) array `member`
    elements: bool,
}

impl Subject {
    fn kernel(&self) -> String {
        match self.elements {
            false => format!("the kernel's `{}` is", self.member),
            true => format!("the elements of the kernel's `{}` are", self.member),
        }
    }

    fn rust(&self) -> &'static str {
        match self.elements {
            false => "the Rust type is",
            true => "the Rust ones are",
        }
    }
}

/// Compares a kernel type with a Rust one, returning the mismatch, if any.
fn compare(
    btf: &Btf,
    kernel: Type,
    rust: &CType,
    subject: &Subject,
) -> Result<Result<(), String>, btf_rs::Error> {
    let (typedef, kernel) = skip_modifiers(btf, kernel)?;
    let mismatch = |rust: String| -> Result<Result<(), String>, btf_rs::Error> {
        Ok(Err(format!(
            "{} {}, but {} {rust}",
            subject.kernel(),
            describe_kernel(btf, typedef.as_deref(), &kernel)?,
            subject.rust(),
        )))
    };

    // integers, including enums
    let int = match &kernel {
        Type::Int(i) => Some((i.size(), i.is_signed())),
        Type::Enum(e) => Some((e.size(), e.is_signed())),
        Type::Enum64(e) => Some((e.size(), e.is_signed())),
        _ => None,
    };
    match (&kernel, int, rust) {
        (_, Some((size, signed)), CType::Primitive(p))
            if matches!(p.kind, PrimitiveKind::Integer | PrimitiveKind::Bool) =>
        {
            if size != p.size {
                return mismatch(bytes(p.size));
            }
            if checks_signedness(p) && signed != (p.signedness == Signedness::Signed) {
                return mismatch(signedness(p.signedness == Signedness::Signed).to_owned());
            }
            Ok(Ok(()))
        }
        (Type::Float(f), _, CType::Primitive(p)) if p.kind == PrimitiveKind::Float => {
            if f.size() != p.size {
                return mismatch(bytes(p.size));
            }
            Ok(Ok(()))
        }
        (Type::Ptr(ptr), _, CType::Pointer(pointee)) => {
            // pointers to anything but #[btf] structs are only passed around
            let CType::Struct(name) = &**pointee else {
                return Ok(Ok(()));
            };
            let (_, target) = skip_modifiers(btf, btf.resolve_chained_type(ptr)?)?;
            if struct_name(btf, &target)?.as_deref() != Some(name.as_str()) {
                return mismatch(format!("a pointer to `struct {name}`"));
            }
            Ok(Ok(()))
        }
        (Type::Array(a), _, CType::Array(elem, len)) => {
            if a.len() as u64 != *len {
                return mismatch(format!("an array of {len}"));
            }
            let subject = Subject {
                member: subject.member.clone(),
                elements: true,
            };
            compare(btf, btf.resolve_chained_type(a)?, elem, &subject)
        }
        (Type::Struct(_), _, CType::Struct(name)) => {
            if struct_name(btf, &kernel)?.as_deref() != Some(name.as_str()) {
                return mismatch(format!("`struct {name}`"));
            }
            Ok(Ok(()))
        }
        _ => mismatch(describe_rust(rust)),
    }
}

/// The name of a struct (or its forward declaration), if it is one.
fn struct_name(btf: &Btf, t: &Type) -> Result<Option<String>, btf_rs::Error> {
    Ok(match t {
        Type::Struct(s) => Some(btf.resolve_name(s)?),
        Type::Fwd(f) if f.is_struct() => Some(btf.resolve_name(f)?),
        _ => None,
    })
}

fn describe_kernel(btf: &Btf, typedef: Option<&str>, t: &Type) -> Result<String, btf_rs::Error> {
    let name = |t: &Type| -> Result<String, btf_rs::Error> {
        Ok(match (typedef, t.as_btf_type()) {
            (Some(typedef), _) => typedef.to_owned(),
            (None, Some(t)) => btf.resolve_name(t)?,
            (None, None) => String::new(),
        })
    };
    Ok(match t {
        Type::Int(i) if i.is_bool() => format!("`{}` (a bool)", name(t)?),
        Type::Int(i) => format!(
            "`{}` ({}, {})",
            name(t)?,
            bytes(i.size()),
            signedness(i.is_signed())
        ),
        Type::Enum(e) => format!(
            "`enum {}` ({}, {})",
            name(t)?,
            bytes(e.size()),
            signedness(e.is_signed())
        ),
        Type::Enum64(e) => format!(
            "`enum {}` ({}, {})",
            name(t)?,
            bytes(e.size()),
            signedness(e.is_signed())
        ),
        Type::Float(f) => format!("`{}` ({})", name(t)?, bytes(f.size())),
        Type::Ptr(ptr) => {
            let (_, target) = skip_modifiers(btf, btf.resolve_chained_type(ptr)?)?;
            match struct_name(btf, &target)? {
                Some(name) => format!("a pointer to `struct {name}`"),
                None => "a pointer".to_owned(),
            }
        }
        Type::Array(a) => format!("an array of {}", a.len()),
        Type::Struct(_) => format!("`struct {}`", name(t)?),
        Type::Union(_) => format!("`union {}`", name(t)?),
        t => format!("a {}", t.name()),
    })
}

fn describe_rust(t: &CType) -> String {
    match t {
        CType::Primitive(p) => match p.kind {
            PrimitiveKind::Integer => "an integer".to_owned(),
            PrimitiveKind::Bool => "a bool".to_owned(),
            PrimitiveKind::Float => "a float".to_owned(),
            PrimitiveKind::Void => "void".to_owned(),
        },
        CType::Struct(name) => format!("`struct {name}`"),
        CType::Pointer(_) => "a pointer".to_owned(),
        CType::Array(_, len) => format!("an array of {len}"),
    }
}

fn bytes(n: usize) -> String {
    match n {
        1 => "1 byte".to_owned(),
        n => format!("{n} bytes"),
    }
}

fn signedness(signed: bool) -> &'static str {
    if signed { "signed" } else { "unsigned" }
}
