# btf-rs

CO-RE (Compile Once – Run Everywhere) access to kernel structs from Rust BPF
programs.

Declare the fields you need of a kernel struct with `#[btf]`, and read them
through generated accessors. Each read is a CO-RE relocation, which libbpf
fixes up for the running kernel's layout when the program loads, so the
program works across kernel versions without recompiling.

```rust
use btf_rs_macros::btf;

#[btf]
pub struct task_struct {
    pub pid: i32,
    pub real_parent: *const task_struct,
    pub comm: [c_char; 16],
    /// Lives in the embedded `struct sched_entity se`
    #[btf(path = "se.vruntime")]
    pub vruntime: u64,
}

let task = task_struct::from_ptr(unsafe { bpf_get_current_task() } as *const _);
let parent: task_struct = task.real_parent()?;
let pid: i32 = parent.pid()?;
if task_struct::has_vruntime() {
    let vruntime = task.vruntime()?;
}
```

## How it works

Rust has no equivalent of clang's `__builtin_preserve_access_index`, so the
field accesses are done in C:

1. `btf_rs::Bindgen`, called from build.rs, scans the program's sources for
   `#[btf]` structs and generates a C header with matching struct definitions
   and a `static inline` accessor for each field
2. bindgen generates Rust bindings for the accessors, along with non-inline C
   wrappers for them (`--wrap-static-fns`)
3. clang compiles the wrappers to BPF LLVM bitcode, which bpf-linker LTOs with
   the Rust code, so the accessors get inlined into the program
4. the `#[btf]` macro turns each struct into a handle calling the accessors

## Usage

`build.rs`:

```rust
use std::path::PathBuf;

fn main() -> Result<(), btf_rs::Error> {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let header = btf_rs::Builder::new().glob("src/**/*.rs");
    btf_rs::Bindgen::new(header, out_dir.join("btf-rs")).generate()?;
    Ok(())
}
```

`Cargo.toml`:

```toml
[dependencies]
btf-rs-macros = { path = "..." }

[build-dependencies]
btf-rs = { path = "..." }
```

The build script has to belong to the BPF program's binary crate, which is
what the wrappers get linked into. The program also needs BTF for the
relocations, e.g. in `.cargo/config.toml`:

```toml
[build]
target = "bpfel-unknown-none"
rustflags = ["-C", "debuginfo=2", "-C", "link-arg=--btf"]

[unstable]
build-std = ["core"]
```

See [`examples/task-info`](examples/task-info) for a complete program, with
instructions for checking its relocations and running it.

### The generated API

A `#[btf]` struct becomes a `Copy` wrapper around a pointer to the kernel
struct, without any fields. The struct has to be named like the kernel's,
since that's how libbpf matches it. It gets:

- `from_ptr(*const c_void) -> Self` and `as_ptr(self) -> *const c_void`
- for each field `foo`:
  - `foo(self) -> Result<T, c_long>`, reading it with `bpf_probe_read_kernel`,
    which returns the (negative) error code on failure
  - `has_foo() -> bool`, checking whether the running kernel has the field

Fields can be:

| Rust type | Accessor returns |
|---|---|
| primitives (`u32`, `i64`, `bool`, ...) and `core::ffi` types (`c_int`, ...) | the same |
| arrays, e.g. `[c_char; 16]` | the same, with pointers to `#[btf]` structs mapped as below |
| pointers to `#[btf]` structs, e.g. `*const task_struct` | the struct's wrapper, e.g. `task_struct` |
| other pointers, e.g. `*const u8`, `*const c_void` | the same |

`#[btf]` structs can't be read by value, but their fields can be accessed with
`#[btf(path = "...")]`: a dot-separated path of the C member, relative to the
struct. The accessors are named after the Rust field. A single-part `path`
just renames the member.

Structs must have named fields and can't be generic.

## Requirements

- nightly Rust with `rust-src`, for `build-std` on the BPF target
- [bpf-linker](https://github.com/aya-rs/bpf-linker) (`cargo install bpf-linker`)
- clang and libclang, with LLVM no newer than the Rust toolchain's
  (`rustc -vV`), since bpf-linker uses the Rust toolchain's LLVM to link
  the bitcode clang produces. The build checks this.
- the libbpf headers (e.g. `libbpf-dev` on Debian/Ubuntu). If they aren't
  in a default include path, add it with `Bindgen::include_dir`.

## Crates

- `btf-rs`: the build-time part: header generation (`Builder`) and
  bindings, wrappers and linking (`Bindgen`)
- `btf-rs-macros`: the `#[btf]` attribute macro
- `btf-rs-parser`: parsing of `#[btf]` structs, shared by the two

## Limitations

- only a plain `#[btf]` is recognized when scanning sources, so the macro has
  to be imported under that name
- `mod foo;` declarations aren't followed when scanning: the files have to be
  covered by the builder's inputs (e.g. a glob)
- paths can't go through pointers or array elements
