//! Logs a few fields of the current task and its parent whenever a task
//! goes to sleep with `nanosleep`, reading them through `#[btf]` accessors.

#![no_std]
#![no_main]

use core::ffi::c_char;

use aya_ebpf::helpers::{bpf_get_current_task, bpf_printk};
use aya_ebpf::macros::kprobe;
use aya_ebpf::programs::ProbeContext;
use corelox::btf;

/// Only the fields we need, named as in the kernel's `struct task_struct`.
/// Their offsets get fixed up for the running kernel when the program loads.
#[btf]
pub struct task_struct {
    pub pid: i32,
    pub tgid: i32,
    pub real_parent: *const task_struct,
    pub comm: [c_char; 16],
    /// Lives in the embedded `struct sched_entity se`
    #[btf(path = "se.vruntime")]
    pub vruntime: u64,
}

#[kprobe(function = "do_nanosleep")]
pub fn task_info(_ctx: ProbeContext) -> u32 {
    let _ = try_task_info();
    0
}

fn try_task_info() -> Result<(), i64> {
    // SAFETY: always returns a valid pointer to the current task
    let task = task_struct::from_ptr(unsafe { bpf_get_current_task() } as *const _);
    let parent = task.real_parent()?;

    let pid = task.pid()?;
    let tgid = task.tgid()?;
    let comm = task.comm()?;
    let parent_pid = parent.pid()?;
    let parent_comm = parent.comm()?;
    // not every kernel has to have it, so check first
    let vruntime = if task_struct::has_vruntime() {
        task.vruntime()?
    } else {
        0
    };

    // SAFETY: the format string matches the arguments, and the comms are NUL-terminated
    unsafe {
        bpf_printk!(
            c"%s[%d/%d] vruntime=%llu",
            comm.as_ptr(),
            pid,
            tgid,
            vruntime
        );
        bpf_printk!(c"  parent: %s[%d]", parent_comm.as_ptr(), parent_pid);
    }
    Ok(())
}

#[unsafe(no_mangle)]
#[unsafe(link_section = "license")]
pub static LICENSE: [u8; 4] = *b"GPL\0";

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
