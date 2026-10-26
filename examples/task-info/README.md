# task-info

This is an end-to-end example of `#[btf]` structs. It is a kprobe on
`do_nanosleep`. A kprobe is a BPF program attached to the start of a
kernel function. This kprobe logs some fields of the current task and its parent. It
reads the fields through CO-RE accessors. Thus it works on different kernel
versions without a new compile.

- `src/main.rs` declares the `struct task_struct` fields that it needs with
  `#[btf]`. It reads them with the generated accessors.
- `build.rs` uses `corelox_build::Bindgen` to generate the C header and the
  wrappers for the accessors. It also links them into the program.

## Requirements

- nightly Rust with `rust-src` (picked up from `rust-toolchain.toml`)
- [bpf-linker](https://github.com/aya-rs/bpf-linker) (`cargo install bpf-linker`)
- clang and libclang, with LLVM no newer than the nightly's
  (`rustc +nightly -vV`)
- the libbpf headers (for example, `libbpf-dev` on Debian or Ubuntu)
- bpftool, to check and load the program

## Building

```sh
cargo build --release
```

The program is `target/bpfel-unknown-none/release/task-info`. You can ignore
the "unable to open LLVM shared lib ... file too short" warning from
bpf-linker (see the [top-level README](../../README.md#requirements)).

## Checking the relocations (no root needed)

`bpftool gen min_core_btf` resolves the CO-RE relocations of the program
against the BTF of the running kernel. It does this in the same way as the
kernel does at load time:

```sh
bpftool -d gen min_core_btf /sys/kernel/btf/vmlinux /tmp/task-info.btf \
    target/bpfel-unknown-none/release/task-info 2>&1 | grep matching
```

Each field read is a relocation that matches the `struct task_struct` of
the kernel. The output shows the offset in the kernel struct. For example:

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
