//! C header generation. The layout lives in `templates/header.h`.

mod check;
mod ctype;

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::PathBuf;

use askama::Template;
use corelox_parser::BtfItem;
use corelox_parser::syn::ext::IdentExt;

use crate::error::{self, Error, location};
use ctype::{CType, TypeMapper};

/// A `#[btf]` struct along with the file it was found in.
pub(crate) struct SourceStruct {
    pub path: PathBuf,
    pub item: BtfItem,
}

#[derive(Template)]
#[template(path = "header.h", escape = "none")]
struct Header<'a> {
    /// `#[btf]` structs in source order, followed by generated nested structs
    structs: &'a [CStruct],
    /// Ordered so that each struct comes after the structs it contains
    definitions: Vec<&'a CStruct>,
}

struct CStruct {
    name: String,
    fields: Vec<CField>,
    /// One per Rust field; empty for generated nested structs
    accessors: Vec<Accessor>,
    /// Indices of structs that must be defined before this one
    deps: Vec<usize>,
    /// Index of the `#[btf]` struct this one comes from
    origin: usize,
}

struct CField {
    name: String,
    ty: CType,
}

struct Accessor {
    /// The Rust field name
    name: String,
    /// The C member, e.g. `se.vruntime`
    member: String,
    ty: CType,
    signedness_check: Option<SignednessCheck>,
}

/// Checks that an integer has the same signedness in the kernel as in Rust
struct SignednessCheck {
    /// The integer, e.g. `se.vruntime` or `comm[0]`
    member: String,
    signed: bool,
}

/// A struct member while grouping `#[btf(path)]` fields into nested structs
enum Member {
    Field { ty: CType, deps: Vec<usize> },
    Nested(Vec<(String, Member)>),
}

/// Generates the header, checking field types against `btf` if given.
/// Returns the header and any warnings.
pub(crate) fn generate(
    structs: &[SourceStruct],
    btf: Option<&btf_rs::Btf>,
) -> Result<(String, Vec<String>), Error> {
    let mut errors = Vec::new();
    let known = index_by_name(structs, &mut errors);

    let mut cstructs = Vec::with_capacity(structs.len());
    let mut nested = Vec::new();
    for (idx, s) in structs.iter().enumerate() {
        let (members, accessors) = members(s, &known, &mut errors);
        let name = s.item.ident.unraw().to_string();
        let (fields, deps) = flatten(&name, members, idx, structs.len(), &mut nested);
        cstructs.push(CStruct {
            name,
            fields,
            accessors,
            deps,
            origin: idx,
        });
    }
    cstructs.append(&mut nested);

    if !errors.is_empty() {
        return Err(Error::Source(errors));
    }

    let mut warnings = Vec::new();
    if let Some(btf) = btf {
        // the first cstructs are the #[btf] structs, in the same order
        for (s, c) in structs.iter().zip(&cstructs) {
            check::check_struct(btf, s, &c.accessors, &mut errors, &mut warnings)
                .map_err(Error::Btf)?;
        }
        if !errors.is_empty() {
            return Err(Error::Source(errors));
        }
    }

    let definitions = definition_order(&cstructs)
        .map_err(|idx| {
            let s = &structs[cstructs[idx].origin];
            Error::Source(vec![format!(
                "{}: error: #[btf] struct `{}` recursively contains itself",
                location(&s.path, s.item.ident.span()),
                s.item.ident.unraw(),
            )])
        })?
        .into_iter()
        .map(|idx| &cstructs[idx])
        .collect();

    let header = Header {
        structs: &cstructs,
        definitions,
    };
    // rendering only fails if a `Display` impl does, and ours don't
    Ok((header.render().expect("failed to render header"), warnings))
}

/// Returns struct indices by name, reporting duplicates.
fn index_by_name(structs: &[SourceStruct], errors: &mut Vec<String>) -> HashMap<String, usize> {
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
    known
}

/// Maps the fields of `s` to C, grouped into members by their paths.
fn members(
    s: &SourceStruct,
    known: &HashMap<String, usize>,
    errors: &mut Vec<String>,
) -> (Vec<(String, Member)>, Vec<Accessor>) {
    let mut members = Vec::new();
    let mut accessors = Vec::new();
    for field in &s.item.fields {
        let mut mapper = TypeMapper {
            known,
            deps: Vec::new(),
        };
        let ty = match mapper.map(&field.ty) {
            Ok(ty) => ty,
            Err(e) => {
                error::syn_messages(&s.path, e, errors);
                continue;
            }
        };

        let path = field.member_path();
        let member = path.join(".");
        accessors.push(Accessor {
            name: field.ident.unraw().to_string(),
            member: member.clone(),
            ty: ty.clone(),
            signedness_check: ty
                .signedness_check()
                .map(|(index, signed)| SignednessCheck {
                    member: format!("{member}{index}"),
                    signed,
                }),
        });
        let field_member = Member::Field {
            ty,
            deps: mapper.deps,
        };
        if insert(&mut members, &path, field_member).is_err() {
            errors.push(format!(
                "{}: error: C member `{member}` conflicts with another field",
                location(&s.path, field.ident.span()),
            ));
        }
    }
    (members, accessors)
}

/// Inserts `field` at `path`, creating nested members as needed.
/// Fails if the path is already taken or runs through a non-nested member.
fn insert(members: &mut Vec<(String, Member)>, path: &[String], field: Member) -> Result<(), ()> {
    let (first, rest) = path.split_first().expect("non-empty path");
    let existing = members.iter_mut().find(|(name, _)| name == first);
    match (existing, rest.is_empty()) {
        (None, true) => members.push((first.clone(), field)),
        (None, false) => {
            let mut nested = Vec::new();
            insert(&mut nested, rest, field)?;
            members.push((first.clone(), Member::Nested(nested)));
        }
        (Some((_, Member::Nested(nested))), false) => insert(nested, rest, field)?,
        (Some(_), _) => return Err(()),
    }
    Ok(())
}

/// Turns the members of struct `name` into C fields, adding a generated
/// `{name}__{member}` struct to `nested` for each nested member.
/// Returns the fields and the indices of structs they contain by value.
fn flatten(
    name: &str,
    members: Vec<(String, Member)>,
    origin: usize,
    // index of the first generated struct
    nested_base: usize,
    nested: &mut Vec<CStruct>,
) -> (Vec<CField>, Vec<usize>) {
    let mut fields = Vec::new();
    let mut deps = Vec::new();
    for (member, m) in members {
        match m {
            Member::Field { ty, deps: d } => {
                deps.extend(d);
                fields.push(CField { name: member, ty });
            }
            Member::Nested(members) => {
                let nested_name = format!("{name}__{member}");
                let (nested_fields, nested_deps) =
                    flatten(&nested_name, members, origin, nested_base, nested);
                deps.push(nested_base + nested.len());
                fields.push(CField {
                    name: member,
                    ty: CType::Struct(nested_name.clone()),
                });
                nested.push(CStruct {
                    name: nested_name,
                    fields: nested_fields,
                    accessors: Vec::new(),
                    deps: nested_deps,
                    origin,
                });
            }
        }
    }
    (fields, deps)
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
