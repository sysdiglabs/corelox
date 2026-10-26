# task-info

An end-to-end example of `#[btf]` structs: a kprobe on `do_nanosleep` that
logs a few fields of the current task and its parent, read through CO-RE
accessors, so it works across kernel versions without recompiling.

- `src/main.rs` declares the fields it needs of `struct task_struct` with
  `#[btf]`, and reads them with the generated accessors
- `build.rs` generates the C header and wrappers for them with
  `corelox_build::Bindgen`, and has them linked into the program

## Requirements

- nightly Rust with `rust-src` (picked up from `rust-toolchain.toml`)
- [bpf-linker](https://github.com/aya-rs/bpf-linker) (`cargo install bpf-linker`)
- clang and libclang, with LLVM no newer than the nightly's
  (`rustc +nightly -vV`)
- the libbpf headers (e.g. `libbpf-dev` on Debian/Ubuntu)
- bpftool, to check and load the program

## Building

```sh
cargo build --release
```

The program is `target/bpfel-unknown-none/release/task-info`. You can ignore
the "unable to open LLVM shared lib ... file too short" warning from
bpf-linker (see the [top-level README](../../README.md#requirements)).

## Checking the relocations (no root needed)

`bpftool gen min_core_btf` resolves the program's CO-RE relocations against
the running kernel's BTF, like loading it would:

```sh
bpftool -d gen min_core_btf /sys/kernel/btf/vmlinux /tmp/task-info.btf \
    target/bpfel-unknown-none/release/task-info 2>&1 | grep matching
```

Each field read shows up as a relocation matched to the kernel's
`struct task_struct`, with the kernel's offset, e.g.:

```
relo #2: matching candidate #0 <byte_off> [...] struct task_struct.tgid (0:95 @ offset 2804)
relo #7: matching candidate #0 <byte_off> [...] struct task_struct.se.vruntime (0:19:14 @ offset 248)
```

## Running

```sh
sudo bpftool prog load target/bpfel-unknown-none/release/task-info \
    /sys/fs/bpf/task_info autoattach
sleep 1
sudo cat /sys/kernel/tracing/trace_pipe    # e.g. "sleep[1234/1234] vruntime=..."
sudo rm /sys/fs/bpf/task_info              # detaches and unloads it
```
