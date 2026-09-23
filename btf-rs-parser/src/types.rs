//! Field types supported in `#[btf]` structs.
//!
//! Besides the primitives below, fields can be raw pointers, arrays
//! and other `#[btf]` structs.

use zyn::syn::{Path, PathArguments};

/// Returns the C equivalent of a primitive type: a Rust primitive like `u32`
/// (without a path) or a `core::ffi` type like `c_int` (through any path).
pub fn primitive_c_type(path: &Path) -> Option<&'static str> {
    let last = path.segments.last()?;
    if !matches!(last.arguments, PathArguments::None) {
        return None;
    }
    let name = last.ident.to_string();
    let single = path.leading_colon.is_none() && path.segments.len() == 1;
    if single && let Some(c) = rust_primitive(&name) {
        return Some(c);
    }
    c_type(&name)
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
        "bool" => "bool",
        "f32" => "float",
        "f64" => "double",
        _ => return None,
    })
}

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
}
