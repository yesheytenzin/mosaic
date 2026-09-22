/* A Bionic shared object that logs every binder driver call a process makes.
 *
 * ADR-0004 says Mosaic replaces the kernel Binder driver with a userspace
 * implementation. To do that without guessing, this preloads into the process
 * and reports exactly which device calls libbinder makes: the open, each ioctl
 * with its request code and the command bytes it carries, the mmap, and the
 * polls. It answers the calls that are trivial and refuses the rest, so a run
 * shows the whole required surface in order.
 *
 * Nothing here is needed by the product except the answers themselves; this is
 * how the answers were found.
 *
 * Built without an NDK and without headers, because the interesting constants
 * are stable ABI and everything else goes through raw syscalls. See build.sh.
 */

typedef unsigned long size_t;
typedef long ssize_t;
typedef long off_t;

extern long syscall(long number, ...);
extern void *malloc(unsigned long);
extern void free(void *);

#define SYS_read 0
#define SYS_write 1
#define SYS_close 3
#define SYS_poll 7
#define SYS_mmap 9
#define SYS_nanosleep 35
#define SYS_ioctl 16
#define SYS_ftruncate 77
#define SYS_epoll_ctl 233
#define SYS_openat 257
#define SYS_memfd_create 319

#define AT_FDCWD -100
#define O_RDONLY 0
#define O_RDWR 2
#define O_CREAT 0100
#define O_TRUNC 01000
#define O_CLOEXEC 02000000
#define PROT_READ 1
#define PROT_WRITE 2
#define MAP_SHARED 1
#define MAP_PRIVATE 2
#define MAP_NORESERVE 040000
#define EINVAL 22
#define ENOSYS 38
#define ENOTTY 25

/* linux/android/binder.h, which is stable ABI. _IOC is
 * (dir << 30) | (size << 16) | (type << 8) | number. */
#define BINDER_WRITE_READ 0xC0306201UL
#define BINDER_SET_MAX_THREADS 0x40046205UL
#define BINDER_SET_CONTEXT_MGR 0x40046207UL
#define BINDER_VERSION 0xC0046209UL
#define BINDER_CURRENT_PROTOCOL_VERSION 8

struct binder_write_read {
    long write_size;
    long write_consumed;
    unsigned long write_buffer;
    long read_size;
    long read_consumed;
    unsigned long read_buffer;
};

/* The logging helpers are defined further down; the command dumpers below are
 * declared up here so they can use them. */
static void emit(const char *s);
static int tracing_on(void);
static void emit_dec(long value);
static void emit_hex(unsigned long value, int digits);
static void flush_log(void);

extern int strncmp(const char *, const char *, unsigned long);
extern int strcmp(const char *, const char *);
extern char *strcpy(char *, const char *);
extern char *strcat(char *, const char *);
extern char *getenv(const char *);
extern void *dlsym(void *, const char *);
#define RTLD_NEXT ((void *)-1L)
extern char *strstr(const char *, const char *);

/* Report each rewritten path once, so a run stays readable. */
#define MAX_REDIRECTIONS 24
static const char *redirected[MAX_REDIRECTIONS];
static int redirected_count = 0;

static void report(const char *from, const char *to) {
    if (redirected_count >= MAX_REDIRECTIONS) return;
    for (int i = 0; i < redirected_count; i++) {
        if (redirected[i] && strcmp(redirected[i], from) == 0) return;
    }
    redirected[redirected_count++] = from;
    if (!tracing_on()) return;
    emit("android-paths: ");
    emit(from);
    emit(" -> ");
    emit(to);
    emit("\n");
    flush_log();
}
static void flush_log(void);
static char log_buffer[16384];
static long log_length = 0;

/* Binder command words.
 *
 * The BC_/BR_ constants in binder.h are _IOW/_IOR encodings, not small numbers:
 * bits 0..7 are the number, 8..15 the type ('c' for a command, 'r' for a reply),
 * 16..29 the argument length, 30..31 the direction. So each command word in the
 * stream carries its own operand length, and the stream is self-describing --
 * read a word, skip that many bytes, repeat.
 *
 * Four earlier attempts read the word as a command number and gave up on the
 * bytes. The two streams that looked inconsistent are BC_TRANSACTION
 * (_IOW('c',0,64) = 0x40406300, and 68 = 4 + 64) and a handle command
 * (_IOW('c',5,4) = 0x40046305, and 8 = 4 + 4). The text that looked like garbage
 * was the last four bytes of the 64-byte struct the constant itself declares.
 */
#define IOC_WRITE 1
#define IOC_READ 2
#define IOC(dir, type, nr, size) \
    ((((unsigned)(dir)) << 30) | (((unsigned)(size)) << 16) | \
     (((unsigned)(type)) << 8) | ((unsigned)(nr)))
#define BC(nr, size) IOC(IOC_WRITE, 'c', nr, size)
#define BR(nr, size) IOC(IOC_READ, 'r', nr, size)

#define BC_TRANSACTION BC(0, 64)
#define BC_REPLY BC(1, 64)
#define BC_FREE_BUFFER BC(3, 8)
#define BR_TRANSACTION_COMPLETE BR(4, 4)
#define BR_NOOP IOC(0, 'r', 12, 0)
#define BR_REPLY BR(3, 64)

/* struct binder_transaction_data, 64 bytes, as libbinder writes it: the code is
 * at 16, the data pointers at 48 and 56, and the sizes are 64-bit. */
struct binder_transaction_data {
    unsigned int target_handle;
    unsigned int target_padding;
    unsigned long cookie;
    unsigned int code;
    unsigned int flags;
    int sender_pid;
    unsigned int sender_euid;
    unsigned long data_size;
    unsigned long offsets_size;
    unsigned long data_buffer;
    unsigned long data_offsets;
};

#define TRANSACTION_DATA_SIZE 64
#define TF_ONE_WAY 1

/* The service-manager dispatch, in android-binder.so. Both doors must share one
 * registry, and the Android linker puts every LD_PRELOAD library in the global
 * group, so the symbol resolves there. */
extern int mosaic_binder_reply(unsigned int handle, unsigned int code, const unsigned char *request,
                               unsigned long request_size, const unsigned long *argument_offsets,
                               unsigned long argument_count, unsigned char **out_data,
                               unsigned long *out_size, unsigned long **out_objects,
                               unsigned long *out_objects_count);

/* One reply, per thread. A transaction is answered on the thread that made it. */
typedef struct {
    int have;
    unsigned int code;
    unsigned int flags;
    unsigned char *data;
    unsigned long size;
    unsigned long *objects;
    unsigned long objects_count;
} pending_reply_t;

static __thread pending_reply_t pending;

/* A reply's buffers belong to the framework once it has them, and it returns
 * them with BC_FREE_BUFFER. Until then they must stay alive, and the pointer it
 * returns names the data, which is how the object array is found. */
#define MAX_OUTSTANDING 64
static unsigned char *outstanding_data[MAX_OUTSTANDING];
static unsigned long *outstanding_objects[MAX_OUTSTANDING];
static int outstanding_lock = 0;

static void hold(unsigned char *data, unsigned long *objects) {
    while (__sync_lock_test_and_set(&outstanding_lock, 1)) {
    }
    for (int i = 0; i < MAX_OUTSTANDING; i++) {
        if (!outstanding_data[i]) {
            outstanding_data[i] = data;
            outstanding_objects[i] = objects;
            break;
        }
    }
    __sync_lock_release(&outstanding_lock);
}

static void release(unsigned char *data) {
    if (!data) return;
    while (__sync_lock_test_and_set(&outstanding_lock, 1)) {
    }
    for (int i = 0; i < MAX_OUTSTANDING; i++) {
        if (outstanding_data[i] == data) {
            outstanding_data[i] = 0;
            free(outstanding_objects[i]);
            outstanding_objects[i] = 0;
            break;
        }
    }
    __sync_lock_release(&outstanding_lock);
    free(data);
}

/* Answer one transaction.
 *
 * The handle decides who answers it: 0 is the service manager, which the shim
 * does itself, and anything else names an object in another process -- a handle
 * this process was handed by the broker, since the shim is the only thing here
 * that creates them. Those go to the broker, which carries the call to the
 * process that owns the object and relays the answer, exactly as the kernel does
 * on a device. Answering them here with EX_SERVICE_SPECIFIC, which this did, made
 * every service the broker hosts findable by name and then fail on the first
 * call. */
static void answer(struct binder_transaction_data *tr) {
    pending.have = 0;
    pending.code = tr->code;
    pending.flags = tr->flags;

    unsigned char *data = 0;
    unsigned long data_size = 0;
    unsigned long *objects = 0;
    unsigned long objects_count = 0;
    /* The objects among the arguments: libbinder fills `offsets` for an outgoing
     * transaction from the sending Parcel's own object table, which is the only
     * place that knows where they are -- the bytes alone cannot say. */
    int produced = mosaic_binder_reply(tr->target_handle, tr->code,
                                       (const unsigned char *)tr->data_buffer, tr->data_size,
                                       (const unsigned long *)tr->data_offsets,
                                       tr->offsets_size / sizeof(unsigned long), &data,
                                       &data_size, &objects, &objects_count);
    emit("binder-shim: transaction handle ");
    emit_dec((long)tr->target_handle);
    emit(" code ");
    emit_dec((long)tr->code);
    emit(" -> ");
    emit_dec((long)data_size);
    emit(" bytes, ");
    emit_dec((long)objects_count);
    emit(" object(s)\n");
    flush_log();
    if (!produced) return;

    hold(data, objects);
    pending.have = 1;
    pending.data = data;
    pending.size = data_size;
    pending.objects = objects;
    pending.objects_count = objects_count;
}

/* Walk the command stream. It is self-describing -- each command word carries
 * its operand length -- so a command this shim does not act on is not an error
 * and not a reason to stop: the length says how far to skip. Reference and
 * looper commands are consumed and forgotten, which is what a driver does with
 * them for a process that has no remote objects yet. */
static int dumps = 0;

static void dump_stream(const char *what, unsigned char *bytes, unsigned long size) {
    if (dumps >= 4) return;
    dumps++;
    emit("binder-shim: ");
    emit(what);
    emit(" [");
    for (unsigned long i = 0; i < size && i < 96; i++) {
        emit_hex(bytes[i], 2);
        emit(" ");
    }
    emit("]\n");
    flush_log();
}

static void run_commands(unsigned char *buffer, unsigned long size) {
    unsigned long offset = 0;
    while (offset + 4 <= size) {
        unsigned int command;
        __builtin_memcpy(&command, buffer + offset, 4);
        unsigned int operand = (command >> 16) & 0x3fff;
        offset += 4;
        if (offset + operand > size) {
            emit("binder-shim: truncated command stream\n");
            flush_log();
            break;
        }
        if (command == BC_TRANSACTION) {
            struct binder_transaction_data *tr =
                (struct binder_transaction_data *)(buffer + offset);
            emit("binder-shim: tr handle=");
            emit_dec((long)tr->target_handle);
            emit(" code=");
            emit_dec((long)tr->code);
            emit(" buffer=0x");
            emit_hex(tr->data_buffer, 16);
            emit(" size=");
            emit_dec((long)tr->data_size);
            emit(" offsets=");
            emit_dec((long)tr->offsets_size);
            emit("\n");
            flush_log();
            if (dumps < 2) dump_stream("stream", buffer, size);
            answer(tr);
        } else if (command == BC_FREE_BUFFER) {
            unsigned long data;
            __builtin_memcpy(&data, buffer + offset, 8);
            release((unsigned char *)data);
        }
        offset += operand;
    }
}

/* Write the commands the framework is waiting for. A synchronous transaction
 * gets BR_TRANSACTION_COMPLETE and then BR_REPLY; a oneway one gets only the
 * first, because nothing is owed and libbinder finishes on it. */
static long write_reply(unsigned char *out, long capacity) {
    unsigned int complete = BR_TRANSACTION_COMPLETE;
    if (capacity < 4) return 0;

    if (pending.flags & TF_ONE_WAY) {
        __builtin_memcpy(out, &complete, 4);
        return 4;
    }
    if (capacity < 4 + 4 + TRANSACTION_DATA_SIZE) return 0;

    long offset = 0;
    __builtin_memcpy(out + offset, &complete, 4);
    offset += 4;
    unsigned int reply = BR_REPLY;
    __builtin_memcpy(out + offset, &reply, 4);
    offset += 4;

    struct binder_transaction_data tr;
    unsigned char *raw = (unsigned char *)&tr;
    for (unsigned long i = 0; i < sizeof(tr); i++) raw[i] = 0;
    tr.code = pending.code;
    tr.data_size = pending.size;
    tr.offsets_size = pending.objects_count * 8;
    tr.data_buffer = (unsigned long)pending.data;
    tr.data_offsets = (unsigned long)pending.objects;
    __builtin_memcpy(out + offset, &tr, TRANSACTION_DATA_SIZE);
    offset += TRANSACTION_DATA_SIZE;
    return offset;
}

struct timespec {
    long tv_sec;
    long tv_nsec;
};

/* A thread with nothing to do and no work to give it.
 *
 * A driver blocks here. Returning nothing instead is worse than it sounds:
 * libbinder reads the next command out of the buffer it was given, an empty
 * buffer reads as command 0, and it logs "*** BAD COMMAND 0 received from Binder
 * driver" and carries on with a broken idea of where it is. So a waiting thread
 * is given BR_NOOP, which libbinder's command loop treats as exactly what it is
 * -- do nothing, ask again -- after a short wait so the asking is not a spin.
 *
 * Blocking for good is what a driver does and would work too, but it would wedge
 * a thread that some other path expects to come back. */
#define WAIT_STEP_NANOSECONDS 100000000

static void wait_for_work(void) {
    struct timespec step = {0, WAIT_STEP_NANOSECONDS};
    syscall(SYS_nanosleep, &step, 0);
}

static int binder_fd = -1;

/* Constructors run before the program, so a log line here distinguishes "the
 * shim never loaded" from "it loaded and nothing called it". */
__attribute__((constructor)) static void probe_loaded(void);

static size_t str_len(const char *s) {
    size_t n = 0;
    while (s[n]) n++;
    return n;
}

/* Diagnostics are accumulated and written once. Writing each piece separately
 * interleaves with the process's own stderr, which produces output that looks
 * like a corrupted buffer and sends you chasing a bug that is not there. */
/* The interesting events are always reported, up to a bound; the per-syscall
 * tracing is not, because this library sits on the busiest path in the process
 * and a run that traces everything buries its own results. MOSAIC_BINDER_DEBUG=1
 * turns the tracing on, which is how the answers below were found. */
#define MAX_LINES 400
static long lines = 0;

static int tracing = -1;

static int tracing_on(void) {
    if (tracing < 0) {
        const char *v = getenv("MOSAIC_BINDER_DEBUG");
        tracing = (v && v[0] == '1') ? 1 : 0;
    }
    return tracing;
}

static void flush_log(void) {
    if (lines >= MAX_LINES) {
        log_length = 0;
        return;
    }
    if (log_length > 0) {
        syscall(SYS_write, 2, log_buffer, log_length);
        log_length = 0;
        lines++;
    }
}

static void emit(const char *s) {
    if (!s) s = "(null)";
    size_t n = str_len(s);
    if (log_length + (long)n < (long)sizeof(log_buffer)) {
        __builtin_memcpy(log_buffer + log_length, s, n);
        log_length += (long)n;
    }
}

static void emit_hex(unsigned long value, int digits) {
    static const char digits_of[] = "0123456789abcdef";
    char out[32];
    if (digits > 16) digits = 16;
    for (int i = digits - 1; i >= 0; i--) {
        out[i] = digits_of[value & 0xf];
        value >>= 4;
    }
    if (log_length + digits < (long)sizeof(log_buffer)) {
        __builtin_memcpy(log_buffer + log_length, out, digits);
        log_length += digits;
    }
}

static void emit_dec(long value) {
    char out[24];
    int i = 0;
    unsigned long magnitude;
    if (value < 0) {
        out[i++] = '-';
        magnitude = (unsigned long)(-value);
    } else {
        magnitude = (unsigned long)value;
    }
    char tmp[20];
    int n = 0;
    do {
        tmp[n++] = '0' + (magnitude % 10);
        magnitude /= 10;
    } while (magnitude);
    while (n) out[i++] = tmp[--n];
    if (log_length + i < (long)sizeof(log_buffer)) {
        __builtin_memcpy(log_buffer + log_length, out, i);
        log_length += i;
    }
}

static void emit_prefixed(const char *prefix, const char *value) {
    emit("binder-shim: ");
    emit(prefix);
    emit(value);
    emit("\n");
}

/* Returns the rewritten path, or the original. */
static const char *redirect(const char *path) {
    static char buffer[4096];
    const char *root = getenv("MOSAIC_ANDROID_ROOT");
    if (!root || !*root || !path || path[0] != '/') return path;

    const char *rest = 0;
    const char *prefix = 0;
    if (strncmp(path, "/system/", 8) == 0) {
        prefix = "/system";
        rest = path + 8;
    } else if (strncmp(path, "/data/", 6) == 0) {
        prefix = "/data";
        rest = path + 6;
    } else if (strncmp(path, "/vendor/", 8) == 0 ||
               strncmp(path, "/product/", 9) == 0 ||
               strncmp(path, "/system_ext/", 12) == 0 ||
               strncmp(path, "/odm/", 5) == 0) {
        /* The other partitions, kept under the bundle with their own names, so
         * /vendor/etc/x resolves to <bundle>/vendor/etc/x. */
        prefix = "";
        rest = path + 1;
    } else if (strncmp(path, "/apex/", 6) == 0) {
        /* /apex/<module>/javalib/<file> and .../lib64/<file> both live in the
         * bundle's flat framework and lib64 directories. */
        const char *javalib = strstr(path, "/javalib/");
        const char *lib64 = strstr(path, "/lib64/");
        if (javalib) {
            prefix = "/apex-javalib";
            rest = javalib + 9;
        } else if (lib64) {
            prefix = "/apex-lib64";
            rest = lib64 + 7;
        } else {
            return path;
        }
    } else {
        return path;
    }

    if (strcmp(prefix, "/apex-javalib") == 0) {
        strcpy(buffer, root);
        strcat(buffer, "/framework/");
    } else if (strcmp(prefix, "/apex-lib64") == 0) {
        strcpy(buffer, root);
        strcat(buffer, "/lib64/");
    } else {
        /* The bundle root stands in for /system, so those paths lose the prefix
         * rather than gaining it. */
        strcpy(buffer, root);
        strcat(buffer, "/");
    }
    strcat(buffer, rest);
    report(path, buffer);
    return buffer;
}

static int name_is_binder(const char *path) {
    if (!path) return 0;
    /* The devices are named for what they are: binder, hwbinder, vndbinder. An
     * exact match on "binder" silently skipped the other two, so their
     * ProcessState never opened and libhidl's service manager was null --
     * which is what stopped HAL registration. */
    const char *last = path;
    for (const char *p = path; *p; p++) {
        if (*p == '/') last = p + 1;
    }
    const char *suffix = "binder";
    size_t n = str_len(last);
    size_t m = str_len(suffix);
    if (n < m) return 0;
    for (size_t i = 0; i < m; i++) {
        if (last[n - m + i] != suffix[i]) return 0;
    }
    return 1;
}

static int make_placeholder_fd(void) {
    /* libbinder expects to mmap the driver. A memfd stands in so those calls
     * succeed and the interesting ones, the ioctls, reach us. */
    int fd = (int)syscall(SYS_memfd_create, "mosaic-binder", 0);
    if (fd < 0) return fd;
    syscall(SYS_ftruncate, fd, 1024 * 1024);
    return fd;
}

/* Bionic's <fcntl.h> defines open() as an inline wrapper around __openat, so a
 * preloaded open() is never called. These are the symbols that matter. */
int __openat(int dirfd, const char *path, int flags, int mode) {
    /* The framework hardcodes paths that cannot be configured, so they are
     * rewritten first; then the binder device is intercepted. */
    path = redirect(path);
    if (name_is_binder(path)) {
        binder_fd = make_placeholder_fd();
        emit_prefixed("open ", path);
        emit("binder-shim:   -> placeholder fd ");
        emit_dec(binder_fd);
        emit("\n");
        return binder_fd;
    }
    return (int)syscall(SYS_openat, dirfd, path, flags, mode);
}

int __openat64(int dirfd, const char *path, int flags, int mode) {
    return __openat(dirfd, path, flags, mode);
}

/* _FORTIFY_SOURCE turns open() into __open_2, which is what libbinder's
 * open("/dev/binder", O_RDWR | O_CLOEXEC) actually calls. */
int __open_2(const char *path, int flags) {
    return __openat(AT_FDCWD, path, flags, 0);
}

int __openat_2(int dirfd, const char *path, int flags) {
    return __openat(dirfd, path, flags, 0);
}

/* Redirecting opens is not enough: code that asks whether a file exists uses
 * stat or access, and the font parser filters out fonts whose files it cannot
 * see. The structures are opaque here -- only the path is rewritten, and the same
 * pointer is passed on for the kernel to fill. */
#define REDIRECT_PASSTHROUGH(name, ...)                                        \
    int name(__VA_ARGS__);                                                     \
    int name(__VA_ARGS__)

extern int __statx(int, const char *, int, unsigned int, void *);

int stat(const char *path, void *buf) {
    typedef int (*real_fn)(const char *, void *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "stat");
    return real ? real(redirect(path), buf) : -1;
}

int lstat(const char *path, void *buf) {
    typedef int (*real_fn)(const char *, void *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "lstat");
    return real ? real(redirect(path), buf) : -1;
}

int stat64(const char *path, void *buf) {
    typedef int (*real_fn)(const char *, void *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "stat64");
    return real ? real(redirect(path), buf) : -1;
}

int lstat64(const char *path, void *buf) {
    typedef int (*real_fn)(const char *, void *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "lstat64");
    return real ? real(redirect(path), buf) : -1;
}

int statx(int dirfd, const char *path, int flags, unsigned int mask, void *buf) {
    typedef int (*real_fn)(int, const char *, int, unsigned int, void *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "statx");
    return real ? real(dirfd, redirect(path), flags, mask, buf) : -1;
}

int access(const char *path, int mode) {
    typedef int (*real_fn)(const char *, int);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "access");
    return real ? real(redirect(path), mode) : -1;
}

int faccessat(int dirfd, const char *path, int mode, int flags) {
    typedef int (*real_fn)(int, const char *, int, int);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "faccessat");
    return real ? real(dirfd, redirect(path), mode, flags) : -1;
}

int open(const char *path, int flags, ...) {
    long mode = 0;
    if (flags & O_CREAT) {
        /* variadic: mode is only present with O_CREAT */
        __builtin_va_list args;
        __builtin_va_start(args, flags);
        mode = __builtin_va_arg(args, long);
        __builtin_va_end(args);
    }
    return __openat(AT_FDCWD, path, flags, (int)mode);
}

int open64(const char *path, int flags, ...) {
    long mode = 0;
    if (flags & O_CREAT) {
        __builtin_va_list args;
        __builtin_va_start(args, flags);
        mode = __builtin_va_arg(args, long);
        __builtin_va_end(args);
    }
    return open(path, flags, (int)mode);
}

void *mmap(void *addr, size_t length, int prot, int flags, int fd, off_t offset) {
    if (binder_fd >= 0 && fd == binder_fd) {
        emit("binder-shim: mmap of the binder fd, length ");
        emit_dec((long)length);
        emit("\n");
    }
    return (void *)syscall(SYS_mmap, addr, length, prot, flags, fd, offset);
}

int ioctl(int fd, unsigned long request, ...) {
    __builtin_va_list args;
    __builtin_va_start(args, request);
    void *arg = __builtin_va_arg(args, void *);
    __builtin_va_end(args);

    if (binder_fd < 0 || fd != binder_fd) {
        return (int)syscall(SYS_ioctl, fd, request, arg);
    }

    emit("binder-shim: ioctl 0x");
    emit_hex(request, 8);
    emit(" ");
    if (request == BINDER_VERSION) {
        emit("BINDER_VERSION -> 8\n");
        *(int *)arg = BINDER_CURRENT_PROTOCOL_VERSION;
        return 0;
    }
    if (request == BINDER_SET_MAX_THREADS) {
        emit("BINDER_SET_MAX_THREADS -> ok\n");
        return 0;
    }
    if (request == BINDER_SET_CONTEXT_MGR) {
        emit("BINDER_SET_CONTEXT_MGR -> ok\n");
        return 0;
    }
    if (request == BINDER_WRITE_READ) {
        struct binder_write_read *bwr = (struct binder_write_read *)arg;

        if (bwr->write_size > 0 && bwr->write_buffer) {
            emit("binder-shim: write=");
            emit_dec(bwr->write_size);
            emit(" read=");
            emit_dec(bwr->read_size);
            emit("\n");
            run_commands((unsigned char *)bwr->write_buffer, (unsigned long)bwr->write_size);
            bwr->write_consumed = bwr->write_size;
        } else {
            bwr->write_consumed = 0;
        }

        /* A read-only call with nothing to hand over is a thread waiting for
         * work. Pause before answering it with a no-op, so the re-asking is not
         * a spin. */
        if (!pending.have && bwr->write_size == 0 && bwr->read_size > 0) {
            wait_for_work();
        }

        bwr->read_consumed = 0;
        if (bwr->read_size > 0 && bwr->read_buffer) {
            if (pending.have) {
                bwr->read_consumed =
                    write_reply((unsigned char *)bwr->read_buffer, bwr->read_size);
                pending.have = 0;
            } else {
                /* Nothing to answer, and read space to answer it in: a no-op keeps
                 * the reader where it is. This used to happen only for a read-only
                 * call, so a *write* that also had read space got an empty buffer --
                 * and an empty buffer is not a command, whatever the reader makes of
                 * it. A reply that is owed is delivered above; this is the case where
                 * none is. */
                if (bwr->read_size >= 4) {
                    unsigned int noop = BR_NOOP;
                    __builtin_memcpy((void *)bwr->read_buffer, &noop, 4);
                    bwr->read_consumed = 4;
                }
            }
        }
        flush_log();
        return 0;
    }
    if (request == 0x40046210UL) {
        /* BINDER_ENABLE_ONEWAY_SPAM_DETECTION: advisory, and refusing it makes
         * libbinder log a warning and carry on, so accept it. */
        emit("BINDER_ENABLE_ONEWAY_SPAM_DETECTION -> ok\n");
        return 0;
    }

    emit("(unhandled) -> ENOTTY\n");
    (void)ENOTTY;
    (void)EINVAL;
    (void)ENOSYS;
    return -1;
}

long poll(void *fds, unsigned long nfds, int timeout) {
    long result = syscall(SYS_poll, fds, nfds, timeout);
    return result;
}

int close(int fd) {
    if (binder_fd >= 0 && fd == binder_fd) {
        emit("binder-shim: close of the binder fd\n");
        binder_fd = -1;
        return (int)syscall(SYS_close, fd);
    }
    return (int)syscall(SYS_close, fd);
}

static void probe_loaded(void) {
    emit("binder-shim: loaded\n");
    flush_log();
}

/* Diagnostic: a preloaded library only helps if its symbols win the lookup, and
 * whether they do is not worth assuming. __system_property_find is called
 * constantly by ART, so if this never logs, interposition is not working and the
 * device-call interceptors are not being reached either. */
extern void *dlsym(void *handle, const char *symbol);
#define RTLD_NEXT ((void *)-1L)

const void *__system_property_find(const char *name) {
    if (tracing_on()) {
        emit("binder-shim: property_find ");
        emit(name ? name : "(null)");
        emit("\n");
    }

    typedef const void *(*next_fn)(const char *);
    static next_fn next;
    if (!next) {
        next = (next_fn)dlsym(RTLD_NEXT, "__system_property_find");
    }
    if (next) return next(name);
    return 0;
}
