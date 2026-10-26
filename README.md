# corelox

corelox gives Rust BPF programs CO-RE (Compile Once, Run Everywhere) access
to kernel structs.

You declare the kernel struct fields that you need with `#[btf]`. Then you
read them through generated accessors. Each read is a CO-RE relocation. A
CO-RE relocation is a reference that libbpf adjusts at load time. When the
program loads, libbpf fixes each read for the layout of the running kernel.
Thus the program works on different kernel versions without a new compile.

```rust
use corelox::btf;

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

Rust has no equivalent of the clang builtin
`__builtin_preserve_access_index`. For this reason, C code does the field
accesses:

1. `corelox_build::Bindgen` runs from build.rs. It scans the program's
   sources for `#[btf]` structs. Then it generates a C header with matching
   struct definitions and a `static inline` accessor for each field.
2. bindgen generates Rust bindings for the accessors. It also generates
   non-inline C wrappers for them (`--wrap-static-fns`).
3. clang compiles the wrappers to BPF LLVM bitcode. bpf-linker links this
   bitcode with the Rust code through LTO (link-time optimization). Thus the
   accessors are inlined into the program.
4. The `#[btf]` macro turns each struct into a handle that calls the
   accessors.

## Usage

`build.rs`:

```rust
use std::path::PathBuf;

fn main() -> Result<(), corelox_build::Error> {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let header = corelox_build::Builder::new().glob("src/**/*.rs");
    corelox_build::Bindgen::new(header, out_dir.join("corelox")).generate()?;
    Ok(())
}
```

`Cargo.toml`:

```toml
[dependencies]
corelox = { path = "..." }

[build-dependencies]
corelox-build = { path = "..." }
```

The build script must belong to the binary crate of the BPF program,
because the wrappers are linked into that crate. The program also needs BTF
for the relocations. BTF (BPF Type Format) is type information for BPF
programs. For example, you can enable it in `.cargo/config.toml`:

```toml
[build]
target = "bpfel-unknown-none"
rustflags = ["-C", "debuginfo=2", "-C", "link-arg=--btf"]

[unstable]
build-std = ["core"]
```

For a complete program, see [`examples/task-info`](examples/task-info). Its
README also tells you how to check its relocations and how to run it.

### The generated API

A `#[btf]` struct becomes a `Copy` wrapper around a pointer to the kernel
struct, without fields. The struct must have the same name as the kernel
struct, because libbpf uses the name to match them. The struct gets these
functions:

- `from_ptr(*const c_void) -> Self` and `as_ptr(self) -> *const c_void`
- for each field `foo`:
  - `foo(self) -> Result<T, c_long>`, which reads the field with
    `bpf_probe_read_kernel`. On failure, it returns the negative error code.
  - `has_foo() -> bool`, which tells you whether the running kernel has the field

Fields can have these types:

| Rust type | Accessor returns |
|---|---|
| primitives (`u32`, `i64`, `bool`, ...) and `core::ffi` types (`c_int`, ...) | the same |
| arrays, for example `[c_char; 16]` | the same, with pointers to `#[btf]` structs mapped as below |
| pointers to `#[btf]` structs, for example `*const task_struct` | the wrapper of the struct, for example `task_struct` |
| other pointers, for example `*const u8`, `*const c_void` | the same |

You cannot read a `#[btf]` struct by value. But you can access its fields
with `#[btf(path = "...")]`. The path is a dot-separated path to the C
member, relative to the struct. The accessors get the name of the Rust
field. A path with one part only renames the member.

Each field of a struct must have a name. Structs cannot be generic.

### Type checks

The Rust field types must match the kernel types. When libbpf relocates reads,
it does not check sizes or signedness. Thus a `u64` read from a 4-byte
`int` also gets the next 4 bytes. For this reason, corelox checks the types
two times:

- At load time, on the kernel that loads the program. Each accessor uses
  CO-RE relocations to compare the size of the Rust type with the size of
  the kernel type. For integers, it also compares the signedness. If the
  types do not match, the accessor calls a helper that does not exist. The
  verifier then rejects the program with `invalid func unknown#195900001`
  (size) or `#195900002` (signedness). The message is next to a source line
  that names the field.
- At build time, against the BTF of one kernel. By default, this is the
  running kernel (`/sys/kernel/btf/vmlinux`). To use a different file, set it
  with `Bindgen::type_check`. If the types do not match, the build fails and
  shows the source location. For example:

  ```
  src/main.rs:18:9: error: `task_struct::pid`: the kernel's `pid` is `pid_t` (4 bytes, signed), but the Rust type is 8 bytes
  ```

  Missing structs and fields cause only warnings, because other kernels can
  have them. To check for them at runtime, use `has_foo()`.

corelox does not check signedness for single bytes. In BTF, the kernel's
`char` is `unsigned char` on some kernels and `signed char` on others.
Without this exception, a field such as `comm: [u8; 16]` fails on some
kernels. corelox does not support bitfields.

## Requirements

- nightly Rust with `rust-src`, for `build-std` on the BPF target
- [bpf-linker](https://github.com/aya-rs/bpf-linker) (`cargo install bpf-linker`)
- clang and libclang. Their LLVM version must not be newer than the LLVM
  version of the Rust toolchain (`rustc -vV`). The reason is that bpf-linker
  uses the LLVM of the Rust toolchain to link the bitcode from clang. The
  build makes sure that the versions are compatible.
- the libbpf headers (for example, `libbpf-dev` on Debian or Ubuntu). If the
  headers are not in a default include path, add the path with
  `Bindgen::include_dir`.

The build can show this warning from bpf-linker:

```
warning: linker stderr: unable to open LLVM shared lib .../libLLVM-23-rust-1.100.0-nightly.so: ... file too short
```

You can ignore it. In the rustup toolchain, that file is a linker script that
points to the real library (`libLLVM.so.23.1-rust-...`). It is not a shared
library, so bpf-linker cannot load it, but the link still succeeds.

## Crates

- `corelox-build`: the build-time part. It contains header generation
  (`Builder`) and bindings, wrappers and linking (`Bindgen`).
- `corelox`: the `#[btf]` attribute macro
- `corelox-parser`: the parser for `#[btf]` structs, which the other two
  crates share

## Limitations

- The source scan recognizes only a plain `#[btf]`. Thus you must import the
  macro under that name.
- The source scan does not follow `mod foo;` declarations. The builder inputs
  must include these files, for example through a glob.
- A path cannot go through pointers or array elements.
