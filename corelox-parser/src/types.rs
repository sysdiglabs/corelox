//! Field types supported in `#[btf]` structs.
//!
//! Besides the primitives below, fields can be raw pointers, arrays
//! and other `#[btf]` structs.

use zyn::syn::{Path, PathArguments};

/// A primitive field type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Primitive {
    /// The C equivalent
    pub c_type: &'static str,
    pub kind: PrimitiveKind,
    /// Size in bytes on BPF targets
    pub size: usize,
    pub signedness: Signedness,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimitiveKind {
    Integer,
    Bool,
    Float,
    Void,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signedness {
    Signed,
    Unsigned,
    /// Not an integer, or `char`, whose signedness depends on how the kernel was built
    Unchecked,
}

/// Returns the C equivalent of a primitive type: a Rust primitive like `u32`
/// (without a path) or a `core::ffi` type like `c_int` (through any path).
pub fn primitive_c_type(path: &Path) -> Option<&'static str> {
    primitive(path).map(|p| p.c_type)
}

/// Returns the details of a primitive type, see [`primitive_c_type`].
pub fn primitive(path: &Path) -> Option<Primitive> {
    let last = path.segments.last()?;
    if !matches!(last.arguments, PathArguments::None) {
        return None;
    }
    let name = last.ident.to_string();
    let single = path.leading_colon.is_none() && path.segments.len() == 1;
    let (c_type, kind, size, signedness) = match single {
        true => rust_primitive(&name).or_else(|| c_type(&name)),
        false => c_type(&name),
    }?;
    Some(Primitive {
        c_type,
        kind,
        size,
        signedness,
    })
}

type PrimitiveInfo = (&'static str, PrimitiveKind, usize, Signedness);

fn rust_primitive(name: &str) -> Option<PrimitiveInfo> {
    use PrimitiveKind::*;
    use Signedness::*;
    Some(match name {
        "u8" => ("unsigned char", Integer, 1, Unsigned),
        "u16" => ("unsigned short", Integer, 2, Unsigned),
        "u32" => ("unsigned int", Integer, 4, Unsigned),
        "u64" => ("unsigned long long", Integer, 8, Unsigned),
        "u128" => ("unsigned __int128", Integer, 16, Unsigned),
        "usize" => ("unsigned long", Integer, 8, Unsigned),
        "i8" => ("signed char", Integer, 1, Signed),
        "i16" => ("short", Integer, 2, Signed),
        "i32" => ("int", Integer, 4, Signed),
        "i64" => ("long long", Integer, 8, Signed),
        "i128" => ("__int128", Integer, 16, Signed),
        "isize" => ("long", Integer, 8, Signed),
        "bool" => ("bool", Bool, 1, Unsigned),
        "f32" => ("float", Float, 4, Unchecked),
        "f64" => ("double", Float, 8, Unchecked),
        _ => return None,
    })
}

fn c_type(name: &str) -> Option<PrimitiveInfo> {
    use PrimitiveKind::*;
    use Signedness::*;
    Some(match name {
        "c_char" => ("char", Integer, 1, Unchecked),
        "c_schar" => ("signed char", Integer, 1, Signed),
        "c_uchar" => ("unsigned char", Integer, 1, Unsigned),
        "c_short" => ("short", Integer, 2, Signed),
        "c_ushort" => ("unsigned short", Integer, 2, Unsigned),
        "c_int" => ("int", Integer, 4, Signed),
        "c_uint" => ("unsigned int", Integer, 4, Unsigned),
        "c_long" => ("long", Integer, 8, Signed),
        "c_ulong" => ("unsigned long", Integer, 8, Unsigned),
        "c_longlong" => ("long long", Integer, 8, Signed),
        "c_ulonglong" => ("unsigned long long", Integer, 8, Unsigned),
        "c_float" => ("float", Float, 4, Unchecked),
        "c_double" => ("double", Float, 8, Unchecked),
        "c_void" => ("void", Void, 0, Unchecked),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(path: &str) -> Option<&'static str> {
        primitive_c_type(&zyn::syn::parse_str(path).unwrap())
    }

    #[test]
    fn primitives() {
        assert_eq!(c("u32"), Some("unsigned int"));
        assert_eq!(c("c_int"), Some("int"));
        assert_eq!(c("core::ffi::c_void"), Some("void"));
        // Rust primitives only without a path, so a `foo::u32` is something else
        assert_eq!(c("foo::u32"), None);
        assert_eq!(c("task_struct"), None);
        assert_eq!(c("c_int<T>"), None);
    }

    #[test]
    fn signedness() {
        let signedness = |path| {
            primitive(&zyn::syn::parse_str(path).unwrap())
                .unwrap()
                .signedness
        };
        assert_eq!(signedness("i32"), Signedness::Signed);
        assert_eq!(signedness("core::ffi::c_ulong"), Signedness::Unsigned);
        assert_eq!(signedness("c_char"), Signedness::Unchecked);
        assert_eq!(signedness("f64"), Signedness::Unchecked);
    }
}
