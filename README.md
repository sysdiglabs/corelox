# corelox

CO-RE (Compile Once – Run Everywhere) access to kernel structs from Rust BPF
programs.

Declare the fields you need of a kernel struct with `#[btf]`, and read them
through generated accessors. Each read is a CO-RE relocation, which libbpf
fixes up for the running kernel's layout when the program loads, so the
program works across kernel versions without recompiling.

```rust
use corelox_macros::btf;

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

1. `corelox::Bindgen`, called from build.rs, scans the program's sources for
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

fn main() -> Result<(), corelox::Error> {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let header = corelox::Builder::new().glob("src/**/*.rs");
    corelox::Bindgen::new(header, out_dir.join("corelox")).generate()?;
    Ok(())
}
```

`Cargo.toml`:

```toml
[dependencies]
corelox-macros = { path = "..." }

[build-dependencies]
corelox = { path = "..." }
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

### Type checks

The Rust field types have to match the kernel's: a `u64` read from a 4-byte
`int` would pick up the neighbouring bytes, and libbpf doesn't check sizes
or signedness when it relocates reads. So the types get checked twice:

- **when the program is loaded**, on the kernel it's loaded on: each accessor
  compares the Rust type's size (and signedness, for integers) with the
  kernel's, using CO-RE relocations. On a mismatch, it calls a nonexistent
  helper, so the verifier rejects the program with
  `invalid func unknown#195900001` (size) or `#195900002` (signedness),
  next to a source line naming the field
- **when building**, against the BTF of one kernel: the running one's
  (`/sys/kernel/btf/vmlinux`) by default, or a file set with
  `Bindgen::type_check`. Mismatches fail the build with their source
  locations, e.g.

  ```
  src/main.rs:18:9: error: `task_struct::pid`: the kernel's `pid` is `pid_t` (4 bytes, signed), but the Rust type is 8 bytes
  ```

  Missing structs and fields are only warnings, since other kernels may have
  them (use `has_foo()` to check at runtime).

Signedness isn't checked for single bytes: the kernel's `char` looks like
either `unsigned char` or `signed char` in BTF, depending on the kernel, so
e.g. `comm: [u8; 16]` wouldn't work everywhere otherwise. Bitfields aren't
supported.

## Requirements

- nightly Rust with `rust-src`, for `build-std` on the BPF target
- [bpf-linker](https://github.com/aya-rs/bpf-linker) (`cargo install bpf-linker`)
- clang and libclang, with LLVM no newer than the Rust toolchain's
  (`rustc -vV`), since bpf-linker uses the Rust toolchain's LLVM to link
  the bitcode clang produces. The build checks this.
- the libbpf headers (e.g. `libbpf-dev` on Debian/Ubuntu). If they aren't
  in a default include path, add it with `Bindgen::include_dir`.

## Crates

- `corelox`: the build-time part: header generation (`Builder`) and
  bindings, wrappers and linking (`Bindgen`)
- `corelox-macros`: the `#[btf]` attribute macro
- `corelox-parser`: parsing of `#[btf]` structs, shared by the two

## Limitations

- only a plain `#[btf]` is recognized when scanning sources, so the macro has
  to be imported under that name
- `mod foo;` declarations aren't followed when scanning: the files have to be
  covered by the builder's inputs (e.g. a glob)
- paths can't go through pointers or array elements
