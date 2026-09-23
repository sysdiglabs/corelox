//! C header generation. The layout lives in `templates/header.h`.

mod ctype;

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::PathBuf;

use askama::Template;
use btf_rs_parser::BtfItem;
use btf_rs_parser::syn::ext::IdentExt;

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
    /// In source order
    structs: &'a [CStruct],
    /// Ordered so that each struct comes after the structs it contains
    definitions: Vec<&'a CStruct>,
}

struct CStruct {
    name: String,
    fields: Vec<CField>,
    /// Indices of structs that must be defined before this one
    deps: Vec<usize>,
}

struct CField {
    name: String,
    ty: CType,
}

pub(crate) fn generate(structs: &[SourceStruct]) -> Result<String, Error> {
    let mut errors = Vec::new();
    let known = index_by_name(structs, &mut errors);
    let cstructs: Vec<_> = structs
        .iter()
        .map(|s| to_c(s, &known, &mut errors))
        .collect();
    if !errors.is_empty() {
        return Err(Error::Source(errors));
    }

    let definitions = definition_order(&cstructs)
        .map_err(|idx| {
            let s = &structs[idx];
            Error::Source(vec![format!(
                "{}: error: #[btf] struct `{}` recursively contains itself",
                location(&s.path, s.item.ident.span()),
                cstructs[idx].name,
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
    Ok(header.render().expect("failed to render header"))
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

fn to_c(s: &SourceStruct, known: &HashMap<String, usize>, errors: &mut Vec<String>) -> CStruct {
    let mut mapper = TypeMapper {
        known,
        deps: Vec::new(),
    };
    let mut fields = Vec::new();
    for field in &s.item.fields {
        match mapper.map(&field.ty) {
            Ok(ty) => fields.push(CField {
                name: field.ident.unraw().to_string(),
                ty,
            }),
            Err(e) => error::syn_messages(&s.path, e, errors),
        }
    }
    CStruct {
        name: s.item.ident.unraw().to_string(),
        fields,
        deps: mapper.deps,
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
