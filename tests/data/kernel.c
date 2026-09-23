/*
 * Kernel-like types for testing the type check against BTF.
 *
 * Regenerate kernel.btf after changing this:
 *   clang --target=bpf -g -c kernel.c -o kernel.o
 *   llvm-objcopy --dump-section=.BTF=kernel.btf kernel.o
 */

typedef int __kernel_pid_t;
typedef __kernel_pid_t pid_t;

enum state { STATE_DEAD = -1, STATE_RUNNING };

struct sched_avg {
	unsigned long load_avg;
};

struct sched_entity {
	unsigned long long vruntime;
	struct sched_avg avg;
};

struct mm_struct {
	unsigned long total_vm;
};

struct task_struct {
	pid_t pid;
	unsigned int flags;
	struct task_struct *real_parent;
	struct mm_struct *mm;
	char comm[16];
	struct sched_entity se;
	union {
		int anon_int;
		struct {
			long anon_nested;
		};
	};
	unsigned int bits : 3;
	enum state state;
	const volatile long qualified;
	void *opaque;
	unsigned short shorts[4];
	double ratio;
};

/* referenced so that clang emits full definitions, like in the kernel */
struct task_struct task;
struct mm_struct mm;
