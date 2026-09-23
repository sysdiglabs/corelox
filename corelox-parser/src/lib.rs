pub use zyn::{Diagnostic, syn};

use zyn::TokenStream;
use zyn::syn::ext::IdentExt;
use zyn::syn::spanned::Spanned;
pub mod types;

use zyn::syn::{Attribute, Field, Fields, Ident, ItemStruct, Meta, Type, Visibility};

// No `#[zyn("btf")]`: the derived `FromInput` rejects a bare `#[btf]`,
// so we only derive `from_args` and build the attributes by hand below.
/// Struct-level `#[btf]` settings (none so far)
#[derive(zyn::Attribute)]
pub struct BtfStruct {}

#[derive(zyn::Attribute)]
pub struct BtfField {
    /// Path of the C member this field maps to, relative to the struct,
    /// e.g. `"se.vruntime"` for a member of an embedded struct.
    /// Defaults to the field name.
    #[zyn(default)]
    pub path: Option<String>,
}

/// Collects the arguments of all `#[btf]` attributes in `attrs`,
/// treating a bare `#[btf]` as an empty argument list.
fn btf_args(attrs: &[Attribute]) -> zyn::Result<zyn::Args> {
    let mut merged = zyn::Args::new();
    for attr in attrs.iter().filter(|a| is_btf_attr(a)) {
        match &attr.meta {
            Meta::Path(_) => {}
            Meta::List(_) => merged.extend(attr.parse_args().map_err(zyn::Diagnostic::from)?),
            Meta::NameValue(nv) => {
                return Err(zyn::mark::error("expected #[btf] or #[btf(...)]")
                    .span(nv.eq_token.span)
                    .build());
            }
        }
    }
    Ok(merged)
}

impl zyn::FromInput for BtfStruct {
    fn from_input(input: &zyn::Input) -> zyn::Result<Self> {
        Self::from_args(&btf_args(input.attrs())?)
    }
}

impl BtfStruct {
    /// Parses the arguments of the `#[btf(...)]` attribute macro itself
    /// (i.e. the tokens between the parentheses).
    pub fn from_tokens(tokens: TokenStream) -> zyn::Result<Self> {
        Self::from_args(&zyn::syn::parse2(tokens).map_err(zyn::Diagnostic::from)?)
    }
}

impl BtfField {
    pub fn from_field(field: &Field) -> zyn::Result<Self> {
        let attr = Self::from_args(&btf_args(&field.attrs)?)?;
        if let Some(path) = &attr.path
            && !path.split('.').all(is_c_ident)
        {
            let btf_attr = field.attrs.iter().find(|a| is_btf_attr(a));
            return Err(zyn::mark::error(format!(
                "invalid path `{path}`: expected C identifiers separated by dots"
            ))
            .span(btf_attr.expect("path comes from #[btf]").span())
            .build());
        }
        Ok(attr)
    }
}

fn is_c_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn is_btf_attr(attr: &Attribute) -> bool {
    attr.path().is_ident("btf")
}

fn without_btf_attrs(attrs: &[Attribute]) -> Vec<Attribute> {
    attrs.iter().filter(|a| !is_btf_attr(a)).cloned().collect()
}

/// A `#[btf]` struct: a non-generic struct with named fields.
pub struct BtfItem {
    pub attr: BtfStruct,
    /// Attributes other than `#[btf]` (docs, derives, `#[repr]`, ...)
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub ident: Ident,
    pub fields: Vec<BtfItemField>,
}

pub struct BtfItemField {
    pub attr: BtfField,
    /// Attributes other than `#[btf]`
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub ident: Ident,
    pub ty: Type,
}

impl BtfItemField {
    /// The C member path of this field: `path` split at dots, or the field name.
    pub fn member_path(&self) -> Vec<String> {
        match &self.attr.path {
            Some(path) => path.split('.').map(str::to_owned).collect(),
            None => vec![self.ident.unraw().to_string()],
        }
    }
}

impl BtfItem {
    /// Builds a `BtfItem` from struct-level settings obtained separately,
    /// e.g. from the attribute macro's arguments.
    pub fn new(attr: BtfStruct, item: &ItemStruct) -> zyn::Result<Self> {
        let mut diags = zyn::mark::new();
        let mut has_errors = false;
        let mut error = |d: zyn::mark::Builder| {
            diags = std::mem::take(&mut diags).add(d);
            has_errors = true;
        };

        if !item.generics.params.is_empty() || item.generics.where_clause.is_some() {
            error(zyn::mark::error("#[btf] structs cannot be generic").span(item.ident.span()));
        }

        let mut fields = Vec::new();
        match &item.fields {
            Fields::Named(named) => {
                for field in &named.named {
                    match BtfField::from_field(field) {
                        Ok(attr) => fields.push(BtfItemField {
                            attr,
                            attrs: without_btf_attrs(&field.attrs),
                            vis: field.vis.clone(),
                            ident: field.ident.clone().expect("named field"),
                            ty: field.ty.clone(),
                        }),
                        Err(d) => error(d.into()),
                    }
                }
            }
            Fields::Unnamed(_) | Fields::Unit => {
                error(
                    zyn::mark::error("#[btf] structs must have named fields")
                        .span(item.ident.span()),
                );
            }
        }

        if has_errors {
            return Err(diags.build());
        }

        Ok(Self {
            attr,
            attrs: without_btf_attrs(&item.attrs),
            vis: item.vis.clone(),
            ident: item.ident.clone(),
            fields,
        })
    }

    /// Builds a `BtfItem` taking struct-level settings from the `#[btf]`
    /// attribute(s) on the struct itself, e.g. when scanning sources in build.rs.
    ///
    /// Returns `None` if the struct has no `#[btf]` attribute.
    pub fn from_item(item: &ItemStruct) -> zyn::Result<Option<Self>> {
        if !item.attrs.iter().any(is_btf_attr) {
            return Ok(None);
        }
        let attr = BtfStruct::from_args(&btf_args(&item.attrs)?)?;
        Self::new(attr, item).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zyn::FromInput;

    fn input(src: &str) -> zyn::Input {
        zyn::syn::parse_str(src).unwrap()
    }

    fn field(src: &str) -> Field {
        zyn::syn::parse::Parser::parse_str(Field::parse_named, src).unwrap()
    }

    #[test]
    fn struct_bare() {
        assert!(BtfStruct::from_input(&input("#[btf] struct Foo;")).is_ok());
    }

    #[test]
    fn field_bare() {
        let attr = BtfField::from_field(&field("#[btf] x: u32")).unwrap();
        assert!(attr.path.is_none());
    }

    #[test]
    fn empty_parens() {
        assert!(BtfStruct::from_input(&input("#[btf()] struct Foo;")).is_ok());
        assert!(
            BtfField::from_field(&field("#[btf()] x: u32"))
                .unwrap()
                .path
                .is_none()
        );
    }

    #[test]
    fn no_attr() {
        assert!(BtfStruct::from_input(&input("struct Foo;")).is_ok());
        assert!(
            BtfField::from_field(&field("x: u32"))
                .unwrap()
                .path
                .is_none()
        );
    }

    #[test]
    fn name_value_is_err() {
        assert!(BtfStruct::from_input(&input(r#"#[btf = "x"] struct Foo;"#)).is_err());
    }

    #[test]
    fn unknown_arg_is_err() {
        assert!(BtfStruct::from_input(&input("#[btf(path = \"a\")] struct Foo;")).is_err());
        assert!(BtfField::from_field(&field("#[btf(bogus)] x: u32")).is_err());
    }

    #[test]
    fn field_path() {
        let attr = BtfField::from_field(&field(r#"#[btf(path = "foo.bar.baz")] x: u32"#)).unwrap();
        assert_eq!(attr.path.as_deref(), Some("foo.bar.baz"));
    }

    #[test]
    fn invalid_path_is_err() {
        for path in ["", ".", "a.", ".a", "a..b", "a.1b", "a-b", "a[0]", "a->b"] {
            let src = format!(r#"#[btf(path = "{path}")] x: u32"#);
            assert!(BtfField::from_field(&field(&src)).is_err(), "{path}");
        }
    }

    #[test]
    fn member_path() {
        let item = BtfItem::from_item(&item(
            r#"#[btf] struct Foo { r#type: u32, #[btf(path = "a.b_1._c")] x: u32, #[btf(path = "y")] z: u32 }"#,
        ))
        .unwrap()
        .unwrap();
        let paths: Vec<_> = item.fields.iter().map(BtfItemField::member_path).collect();
        assert_eq!(paths, [vec!["type"], vec!["a", "b_1", "_c"], vec!["y"]]);
    }

    fn item(src: &str) -> ItemStruct {
        zyn::syn::parse_str(src).unwrap()
    }

    fn attr_names(attrs: &[Attribute]) -> Vec<String> {
        attrs
            .iter()
            .map(|a| a.path().get_ident().unwrap().to_string())
            .collect()
    }

    #[test]
    fn struct_from_tokens() {
        assert!(BtfStruct::from_tokens(TokenStream::new()).is_ok());
        assert!(BtfStruct::from_tokens("bogus".parse().unwrap()).is_err());
    }

    #[test]
    fn item_fields() {
        let item = BtfItem::from_item(&item(
            r#"
            /// docs
            #[btf]
            #[repr(C)]
            pub struct Foo {
                #[btf(path = "a.b")]
                pub a: u32,
                /// docs
                b: *const u8,
            }
            "#,
        ))
        .unwrap()
        .unwrap();

        assert_eq!(attr_names(&item.attrs), ["doc", "repr"]);
        assert!(matches!(item.vis, Visibility::Public(_)));
        assert_eq!(item.ident, "Foo");

        let [a, b] = &item.fields[..] else {
            panic!("expected 2 fields");
        };
        assert_eq!(a.ident, "a");
        assert_eq!(a.attr.path.as_deref(), Some("a.b"));
        assert!(a.attrs.is_empty());
        assert!(matches!(a.vis, Visibility::Public(_)));
        assert_eq!(b.ident, "b");
        assert!(b.attr.path.is_none());
        assert_eq!(attr_names(&b.attrs), ["doc"]);
        assert!(matches!(b.vis, Visibility::Inherited));
    }

    #[test]
    fn item_bad_struct_attr_is_err() {
        assert!(BtfItem::from_item(&item("#[btf(bogus)] struct Foo {}")).is_err());
    }

    #[test]
    fn item_without_btf_attr_is_none() {
        assert!(
            BtfItem::from_item(&item("struct Foo {}"))
                .unwrap()
                .is_none()
        );
        // not a #[btf] struct, so it isn't validated either
        assert!(
            BtfItem::from_item(&item("struct Foo<T>(T);"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn item_new() {
        let item = BtfItem::new(BtfStruct {}, &item("struct Foo { x: u32 }")).unwrap();
        assert_eq!(item.ident, "Foo");
        assert_eq!(item.fields.len(), 1);
    }

    #[test]
    fn item_generics_is_err() {
        for src in [
            "#[btf] struct Foo<T> { x: T }",
            "#[btf] struct Foo<'a> { x: &'a u8 }",
            "#[btf] struct Foo<const N: usize> { x: [u8; N] }",
            "#[btf] struct Foo where u8: Copy { x: u8 }",
        ] {
            assert!(BtfItem::from_item(&item(src)).is_err(), "{src}");
        }
    }

    #[test]
    fn item_unnamed_fields_is_err() {
        assert!(BtfItem::from_item(&item("#[btf] struct Foo(u32);")).is_err());
        assert!(BtfItem::from_item(&item("#[btf] struct Foo;")).is_err());
    }

    #[test]
    fn item_bad_field_attr_is_err() {
        let src = "#[btf] struct Foo { #[btf(bogus)] x: u32 }";
        assert!(BtfItem::from_item(&item(src)).is_err());
    }

    #[test]
    fn item_errors_accumulate() {
        let src = "#[btf] struct Foo<T> { #[btf(bogus)] x: T, #[btf(bogus)] y: T }";
        let err = BtfItem::from_item(&item(src)).err().unwrap();
        // generics, 2x field
        assert_eq!(err.len(), 3, "{err}");
    }
}
