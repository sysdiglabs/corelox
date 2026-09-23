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
    pub signedness: Signedness,
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
    let (c_type, signedness) = match single {
        true => rust_primitive(&name).or_else(|| c_type(&name)),
        false => c_type(&name),
    }?;
    Some(Primitive { c_type, signedness })
}

fn rust_primitive(name: &str) -> Option<(&'static str, Signedness)> {
    use Signedness::*;
    Some(match name {
        "u8" => ("unsigned char", Unsigned),
        "u16" => ("unsigned short", Unsigned),
        "u32" => ("unsigned int", Unsigned),
        "u64" => ("unsigned long long", Unsigned),
        "u128" => ("unsigned __int128", Unsigned),
        "usize" => ("unsigned long", Unsigned),
        "i8" => ("signed char", Signed),
        "i16" => ("short", Signed),
        "i32" => ("int", Signed),
        "i64" => ("long long", Signed),
        "i128" => ("__int128", Signed),
        "isize" => ("long", Signed),
        "bool" => ("bool", Unsigned),
        "f32" => ("float", Unchecked),
        "f64" => ("double", Unchecked),
        _ => return None,
    })
}

fn c_type(name: &str) -> Option<(&'static str, Signedness)> {
    use Signedness::*;
    Some(match name {
        "c_char" => ("char", Unchecked),
        "c_schar" => ("signed char", Signed),
        "c_uchar" => ("unsigned char", Unsigned),
        "c_short" => ("short", Signed),
        "c_ushort" => ("unsigned short", Unsigned),
        "c_int" => ("int", Signed),
        "c_uint" => ("unsigned int", Unsigned),
        "c_long" => ("long", Signed),
        "c_ulong" => ("unsigned long", Unsigned),
        "c_longlong" => ("long long", Signed),
        "c_ulonglong" => ("unsigned long long", Unsigned),
        "c_float" => ("float", Unchecked),
        "c_double" => ("double", Unchecked),
        "c_void" => ("void", Unchecked),
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
