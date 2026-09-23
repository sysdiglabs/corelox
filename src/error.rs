use std::fmt;
use std::path::{Path, PathBuf};

use btf_rs_parser::{Diagnostic, syn};
use proc_macro2::Span;

// `Debug` is implemented via `Display`, because that's what build scripts
// print when they `unwrap()` or return an error from `main`.
pub enum Error {
    /// An invalid glob pattern
    Glob {
        pattern: String,
        message: String,
    },
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Errors in the input sources, each formatted as `file:line:col: error: message`
    Source(Vec<String>),
    Bindgen(bindgen::BindgenError),
    /// clang's LLVM is newer than the Rust toolchain's, which bpf-linker uses
    LlvmVersion {
        clang: u32,
        rust: u32,
    },
    /// clang failed to compile the accessor wrappers
    Clang {
        command: String,
        status: std::process::ExitStatus,
        stderr: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Glob { pattern, message } => {
                write!(f, "invalid glob pattern `{pattern}`: {message}")
            }
            Error::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Error::Source(messages) => f.write_str(&messages.join("\n")),
            Error::Bindgen(e) => write!(f, "bindgen: {e}"),
            Error::LlvmVersion { clang, rust } => write!(
                f,
                "clang uses LLVM {clang}, but the Rust toolchain uses LLVM {rust}: \
                 bpf-linker (which uses the Rust toolchain's LLVM) won't be able to link \
                 the code compiled by clang. Use clang {rust} or older (see `Bindgen::clang` \
                 or `$CLANG`), or a Rust toolchain with LLVM {clang} or newer"
            ),
            Error::Clang {
                command,
                status,
                stderr,
            } => write!(f, "{command} failed ({status}):\n{stderr}"),
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            Error::Bindgen(e) => Some(e),
            _ => None,
        }
    }
}

pub(crate) fn location(path: &Path, span: Span) -> String {
    let start = span.start();
    format!("{}:{}:{}", path.display(), start.line, start.column + 1)
}

pub(crate) fn syn_messages(path: &Path, err: syn::Error, out: &mut Vec<String>) {
    for e in err {
        out.push(format!("{}: error: {e}", location(path, e.span())));
    }
}

/// Flattens a zyn diagnostic tree into `file:line:col: level: message` lines.
///
/// zyn doesn't expose a diagnostic's own message, only its `Display`
/// (the own message, if any, followed by the children, one per line),
/// so a node has a message of its own iff its `Display` differs from
/// that of its children.
pub(crate) fn diagnostic_messages(path: &Path, diag: &Diagnostic, out: &mut Vec<String>) {
    let text = diag.to_string();
    let children: Vec<_> = diag.iter().map(ToString::to_string).collect();
    if text != children.join("\n")
        && let Some(own) = text.lines().next()
    {
        out.push(format!("{}: {own}", location(path, diag.span())));
    }
    for child in diag.iter() {
        diagnostic_messages(path, child, out);
    }
}
