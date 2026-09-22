/* An allocation tracer for the binder shim.
 *
 * The question it exists to answer: when the framework crashes on a pointer that
 * was handed to it by the shim, was that address *freed* and then handed out
 * again? A crash address changes every run (ASLR and heap layout), so the answer
 * has to come from a record of the allocations themselves:
 *
 *   MOSAIC_ALLOC_TRACE=1        write every small allocation and free to
 *                               /tmp/mosaic-alloc-trace.log
 *   MOSAIC_ALLOC_TRACE_MIN/MAX  the size window, in bytes (default 16..256)
 *
 * Then, with the crash address X from the core (`coredumpctl list`,
 * `print/x $r14`):
 *
 *   grep -ai "$(printf '%x' 0x<X>)" /tmp/mosaic-alloc-trace.log
 *
 * gives every allocation and free of that address, in sequence order, with the
 * thread that did each one. The shape that means a premature free is an alloc, a
 * free, and then another alloc of the same address before the crash -- the third
 * one being whatever the allocator handed the address to next.
 *
 * Why malloc and not RefBase's refcounts: the allocator event is what actually
 * reuses the memory, whatever C++ mechanism caused the free (a `decStrong`
 * reaching zero, a direct `delete`, a `weakref_impl` teardown), and it needs no
 * mangled-symbol guessing. RefBase's hooks are the second-pass tool if this log
 * turns out ambiguous.
 *
 * Built like the rest of the shim: no libc headers, raw syscalls, and the real
 * allocator reached through dlsym(RTLD_NEXT, ...). See tools/binder-shim/build.sh.
 */

extern long write(int, const void *, unsigned long);
extern long syscall(long, ...);
extern void *dlsym(void *, const char *);
extern void *dlopen(const char *, int);
extern char *getenv(const char *);

#define RTLD_NEXT ((void *)-1L)
#define RTLD_NOW 2
#define SYS_WRITE 1
#define SYS_OPENAT 257
#define SYS_GETTID 186

/* O_WRONLY | O_CREAT | O_APPEND */
#define OPEN_FLAGS 0x441

#define TRACE_PATH "/tmp/mosaic-alloc-trace.log"

static int trace_fd = -2;
static long alloc_seq = 0;
static unsigned long window_min = 0;
static unsigned long window_max = 0;

/* While the real allocator is being resolved, a call back into malloc must not
 * recurse into dlsym. A loader lookup asks for a handful of bytes at most, so a
 * static arena covers it; the blocks are never freed, which is why this is only
 * for that window. */
static char bootstrap_arena[4096];
static unsigned long bootstrap_used = 0;
static __thread int resolving = 0;

static int trace_wanted(void) {
    if (trace_fd == -2) {
        const char *on = getenv("MOSAIC_ALLOC_TRACE");
        trace_fd = (on && on[0] == '1')
                       ? (int)syscall(SYS_OPENAT, -100, TRACE_PATH, OPEN_FLAGS, 0644)
                       : -1;
    }
    return trace_fd >= 0;
}

/* A decimal parser of its own: strtoul is libc, and this library is built
 * without one. */
static long atol_stub(const char *s) {
    long value = 0;
    int negative = 0;
    if (*s == '-') {
        negative = 1;
        s++;
    }
    while (*s >= '0' && *s <= '9') value = value * 10 + (*s++ - '0');
    return negative ? -value : value;
}

/* The window, read once. It defaults to the size of a small binder object: a
 * BpBinder is a vtable and a handful of fields, and the proxy that replaced one
 * is the same order of magnitude. Widen it if the address never appears. */
static void read_window(void) {
    if (window_max != 0) return;
    const char *min = getenv("MOSAIC_ALLOC_TRACE_MIN");
    const char *max = getenv("MOSAIC_ALLOC_TRACE_MAX");
    window_min = min ? (unsigned long)atol_stub(min) : 16;
    window_max = max ? (unsigned long)atol_stub(max) : 256;
}

/* One line per event, written once: a piece-by-piece write interleaves with the
 * process's own stderr and makes the log unreadable.
 *
 * Per *thread*, because several threads allocate at once and a shared buffer
 * interleaves their lines into each other -- which is exactly the corruption the
 * first version of this produced, and it made the sequence unreadable. */
static __thread char line_buffer[256];
static __thread long line_length = 0;

static void put(const char *s) {
    while (*s && line_length < (long)sizeof(line_buffer) - 1) line_buffer[line_length++] = *s++;
}

static void put_dec(long value) {
    char digits[24];
    int n = 0;
    unsigned long magnitude;
    if (value < 0) {
        put("-");
        magnitude = (unsigned long)-value;
    } else {
        magnitude = (unsigned long)value;
    }
    do {
        digits[n++] = (char)('0' + (magnitude % 10));
        magnitude /= 10;
    } while (magnitude);
    while (n > 0) {
        char one[2];
        one[0] = digits[--n];
        one[1] = 0;
        put(one);
    }
}

static void put_hex(unsigned long value) {
    static const char digits[] = "0123456789abcdef";
    char out[19];
    int i;
    out[0] = '0';
    out[1] = 'x';
    for (i = 0; i < 16; i++) out[2 + i] = digits[(value >> (4 * (15 - i))) & 0xf];
    out[18] = 0;
    /* Skip leading zeros so a pointer reads the way gdb prints it. */
    i = 2;
    while (i < 17 && out[i] == '0') i++;
    put("0x");
    put(out + i);
}

static void flush_line(void) {
    if (line_length == 0) return;
    line_buffer[line_length++] = '\n';
    syscall(SYS_WRITE, trace_fd, line_buffer, (unsigned long)line_length);
    line_length = 0;
}

static void event(const char *what, const void *pointer, long size, void *caller) {
    read_window();
    put(what);
    put(" ");
    put_dec(__sync_fetch_and_add(&alloc_seq, 1));
    put(" ");
    put_dec((long)syscall(SYS_GETTID));
    put(" ");
    if (size >= 0) {
        put_dec(size);
        put(" ");
    }
    put_hex((unsigned long)pointer);
    /* Who asked: the address the call will return to, which names the code that
     * did the allocation or the free once it is resolved against the library it
     * belongs to. Without it the log says an address was freed but not by whom,
     * and "by whom" is the whole question. */
    put(" from ");
    put_hex((unsigned long)caller);
    flush_line();
}

void *malloc(unsigned long size) {
    typedef void *(*real_fn)(unsigned long);
    static real_fn real;
    if (real == 0) {
        if (resolving) {
            /* Mid-resolution: hand out the static arena rather than recurse. */
            unsigned long aligned = (bootstrap_used + 15) & ~15UL;
            if (aligned + size <= sizeof(bootstrap_arena)) {
                bootstrap_used = aligned + size;
                return bootstrap_arena + aligned;
            }
            return 0;
        }
        resolving = 1;
        real = (real_fn)dlsym(RTLD_NEXT, "malloc");
        resolving = 0;
    }
    void *pointer = real(size);
    read_window();
    if (trace_wanted() && pointer && size >= window_min && size <= window_max) {
        event("alloc", pointer, (long)size, __builtin_return_address(0));
    }
    return pointer;
}

/* Whether a pointer came from the bootstrap arena rather than the real
 * allocator. Those must never reach the real free: handing libc a pointer it did
 * not allocate corrupts its heap, and the first version of this did exactly that
 * -- the process died with a bus error in an unrelated mmap. */
static int from_bootstrap(const void *pointer) {
    const char *p = (const char *)pointer;
    return p >= bootstrap_arena && p < bootstrap_arena + sizeof(bootstrap_arena);
}

/* A marker the shim can drop into the same log, so a hand-back and the
 * allocations around it share one sequence: "which hand-back was followed by a
 * release of the same address" is the question, and two files cannot answer it. */
void mosaic_alloc_trace_mark(const char *what, const void *pointer) {
    if (!trace_wanted()) return;
    put(what);
    put(" ");
    put_dec(__sync_fetch_and_add(&alloc_seq, 1));
    put(" ");
    put_dec((long)syscall(SYS_GETTID));
    put(" ");
    put_hex((unsigned long)pointer);
    flush_line();
}

typedef struct {
    const char *dli_fname;
    void *dli_fbase;
    const char *dli_sname;
    void *dli_saddr;
} dl_info_t;
extern int dladdr(const void *, dl_info_t *);

/* Every `BpRefBase` construction, with the pointer it is handed.
 *
 * `BpRefBase::mRemote` is written exactly once, by this constructor, as
 * `o.get()` -- so when a proxy ends up with `mRemote` pointing at itself, the
 * value came through here and the question is what was passed in. The
 * constructor is a dynamic symbol in libbinder and the callers are outside it,
 * so their calls go through the PLT and this interposes them. The real
 * constructor is called through, unchanged.
 *
 * The third column of a `bp-refbase` line is `self - o.get()`: for the object's
 * own address in `mRemote` that number is the offset of the `BpRefBase`
 * sub-object (0x28 in these proxies), which is how the self-reference shows up
 * in the log. */

typedef void (*bp_refbase_ctor_fn)(void *self, const void *vtt, const void *o);

static bp_refbase_ctor_fn real_bp_refbase_c2;
static bp_refbase_ctor_fn real_bp_refbase_c1;

__attribute__((constructor)) static void resolve_bp_refbase(void) {
    void *binder = dlopen("libbinder.so", RTLD_NOW);
    if (!binder) return;
    real_bp_refbase_c2 =
        (bp_refbase_ctor_fn)dlsym(binder, "_ZN7android9BpRefBaseC2ERKNS_2spINS_7IBinderEEE");
    real_bp_refbase_c1 =
        (bp_refbase_ctor_fn)dlsym(binder, "_ZN7android9BpRefBaseC1ERKNS_2spINS_7IBinderEEE");
}

static void log_bp_refbase(const char *which, void *self, const void *o, void *caller) {
    if (!trace_wanted()) return;
    const void *remote = o ? *(const void *const *)o : 0;
    event(which, remote, (long)((const char *)self - (const char *)remote), caller);
}

void _ZN7android9BpRefBaseC2ERKNS_2spINS_7IBinderEEE(void *self, const void *vtt, const void *o) {
    log_bp_refbase("bp-refbase", self, o, __builtin_return_address(0));
    if (real_bp_refbase_c2) real_bp_refbase_c2(self, vtt, o);
}

void _ZN7android9BpRefBaseC1ERKNS_2spINS_7IBinderEEE(void *self, const void *vtt, const void *o) {
    log_bp_refbase("bp-refbase-c1", self, o, __builtin_return_address(0));
    if (real_bp_refbase_c1) real_bp_refbase_c1(self, vtt, o);
}

/* Every strong reference taken and dropped on an object whose vtable lives in
 * libbinder, with the caller.
 *
 * `release_object` calls `decStrong` through the PLT (read off its disassembly),
 * so this interposes it and the caller's return address is trustworthy. That is
 * what answers "who dropped the last reference" for a binder that should still
 * have one. Filtered to libbinder vtables, because a global strong-count trace
 * of every `sp` in the process is unreadable. */

typedef void (*refbase_ref_fn)(const void *self, const void *id);

static refbase_ref_fn real_inc_strong;
static refbase_ref_fn real_dec_strong;
static unsigned long libbinder_lo, libbinder_hi;

__attribute__((constructor)) static void resolve_refbase(void) {
    void *binder = dlopen("libbinder.so", RTLD_NOW);
    if (!binder) return;
    real_inc_strong = (refbase_ref_fn)dlsym(binder, "_ZNK7android7RefBase9incStrongEPKv");
    real_dec_strong = (refbase_ref_fn)dlsym(binder, "_ZNK7android7RefBase9decStrongEPKv");
    /* The library's own address range, from its first mapping. dladdr on the
     * vtable would be more precise but needs a symbol table this build does not
     * always have; the range is enough to tell "a binder" from "everything". */
    dl_info_t info;
    __builtin_memset(&info, 0, sizeof info);
    if (dladdr((const void *)dlsym(binder, "malloc"), &info) && info.dli_fbase) {
        libbinder_lo = (unsigned long)info.dli_fbase;
        libbinder_hi = libbinder_lo + 0x1000000;
    }
}

static int looks_like_a_binder(const void *object) {
    if (!object || !libbinder_lo) return 0;
    unsigned long address = (unsigned long)object;
    if (address < 0x10000 || address >= 0x800000000000UL) return 0;
    unsigned long vtable = *(const unsigned long *)object;
    return vtable >= libbinder_lo && vtable < libbinder_hi;
}

void _ZNK7android7RefBase9incStrongEPKv(const void *self, const void *id) {
    if (trace_wanted() && looks_like_a_binder(self)) {
        event("inc-strong", self, -1, __builtin_return_address(0));
    }
    if (real_inc_strong) real_inc_strong(self, id);
}

void _ZNK7android7RefBase9decStrongEPKv(const void *self, const void *id) {
    if (trace_wanted() && looks_like_a_binder(self)) {
        event("dec-strong", self, -1, __builtin_return_address(0));
    }
    if (real_dec_strong) real_dec_strong(self, id);
}

void free(void *pointer) {
    typedef void (*real_fn)(void *);
    static real_fn real;
    if (real == 0) {
        if (resolving) return; /* nothing from the arena is ever freed */
        resolving = 1;
        real = (real_fn)dlsym(RTLD_NEXT, "free");
        resolving = 0;
    }
    if (from_bootstrap(pointer)) return;
    /* The size is not known here, which is why every free inside the window is
     * logged: correlation is by pointer value against the alloc lines. */
    if (trace_wanted() && pointer) event("free", pointer, -1, __builtin_return_address(0));
    real(pointer);
}
