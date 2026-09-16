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

#define SYS_read 0
#define SYS_write 1
#define SYS_close 3
#define SYS_poll 7
#define SYS_mmap 9
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
static void emit_dec(long value);
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

/* Binder command numbers, from binder.h. Only the ones worth naming. */
#define BC_TRANSACTION 0
#define BC_REPLY 1
#define BC_FREE_BUFFER 3
#define BC_INCREFS 4
#define BC_ACQUIRE 5
#define BC_RELEASE 6
#define BC_DECREFS 7
#define BC_ENTER_LOOPER 12
#define BC_REGISTER_LOOPER 13
#define BR_NOOP 0x200C
#define BR_TRANSACTION 0x2002
#define BR_REPLY 0x2003

/* Commands carry a fixed operand after the command word, with no padding:
 * a transaction is command + struct binder_transaction_data, a handle command is
 * command + u32, and a free is command + pointer. */
#define TRANSACTION_DATA_SIZE 64

static const char *command_name(unsigned int command) {
    switch (command) {
        case BC_TRANSACTION: return "BC_TRANSACTION";
        case BC_REPLY: return "BC_REPLY";
        case BC_FREE_BUFFER: return "BC_FREE_BUFFER";
        case BC_INCREFS: return "BC_INCREFS";
        case BC_ACQUIRE: return "BC_ACQUIRE";
        case BC_RELEASE: return "BC_RELEASE";
        case BC_DECREFS: return "BC_DECREFS";
        case BC_ENTER_LOOPER: return "BC_ENTER_LOOPER";
        case BC_REGISTER_LOOPER: return "BC_REGISTER_LOOPER";
        default: return 0;
    }
}

/* A Parcel starts with the interface token as a string16: a character count and
 * then UTF-16 characters. Printing it names the interface being called, which is
 * the quickest way to see what the framework is asking for. */
static void dump_parcel_string(unsigned char *data, long size) {
    if (size < 4) return;
    unsigned int chars;
    __builtin_memcpy(&chars, data, 4);
    if (chars == 0 || chars > 128) return;
    long needed = 4 + (long)chars * 2;
    if (needed > size) return;
    emit(" \"");
    for (unsigned int i = 0; i < chars; i++) {
        unsigned short c;
        __builtin_memcpy(&c, data + 4 + i * 2, 2);
        if (c < 32 || c > 126) {
            emit("?");
            continue;
        }
        char out[1];
        out[0] = (char)c;
        if (log_length + 1 < (long)sizeof(log_buffer)) {
            log_buffer[log_length++] = out[0];
        }
    }
    emit("\"");
}

static int saw_unknown_command = 0;

static int dump_commands(unsigned char *buffer, long size) {
    long offset = 0;
    int wants_reply = 0;
    saw_unknown_command = 0;
    while (offset + 4 <= size) {
        unsigned int command;
        __builtin_memcpy(&command, buffer + offset, 4);
        offset += 4;

        const char *name = command_name(command);
        emit(" ");
        if (!name) {
            emit("cmd:");
            emit_dec((long)command);
            saw_unknown_command = 1;
            continue;
        }
        emit(name);

        if (command == BC_TRANSACTION || command == BC_REPLY) {
            if (offset + TRANSACTION_DATA_SIZE > size) break;
            unsigned char *tr = buffer + offset;
            unsigned int handle, code, flags;
            __builtin_memcpy(&handle, tr, 4);        /* target.handle */
            __builtin_memcpy(&code, tr + 16, 4);
            __builtin_memcpy(&flags, tr + 20, 4);
            unsigned long data_ptr;
            long data_size;
            __builtin_memcpy(&data_ptr, tr + 40, 8);
            __builtin_memcpy(&data_size, tr + 24, 8);
            emit("(handle=");
            emit_dec((long)handle);
            emit(" code=");
            emit_dec((long)code);
            emit(" flags=");
            emit_dec((long)flags);
            if (data_ptr && data_size > 0) {
                dump_parcel_string((unsigned char *)data_ptr, data_size);
            }
            emit(")");
            if (command == BC_TRANSACTION && !(flags & 1)) {
                /* TF_ONE_WAY is bit 0; anything else expects an answer. */
                wants_reply = 1;
            }
            offset += TRANSACTION_DATA_SIZE;
        } else if (command == BC_FREE_BUFFER) {
            offset += 8;
        } else if (command == BC_INCREFS || command == BC_ACQUIRE ||
                   command == BC_RELEASE || command == BC_DECREFS) {
            unsigned int handle;
            if (offset + 4 <= size) {
                __builtin_memcpy(&handle, buffer + offset, 4);
                emit("(handle=");
                emit_dec((long)handle);
                emit(")");
            }
            offset += 4;
        }
    }
    return wants_reply;
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
static void flush_log(void) {
    if (log_length > 0) {
        syscall(SYS_write, 2, log_buffer, log_length);
        log_length = 0;
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
    /* "/dev/binder", "/dev/binderfs/binder", anything that ends in binder. */
    const char *p = path;
    while (*p) p++;
    while (p > path && p[-1] != '/') p--;
    const char *last = p;
    const char *want = "binder";
    int i = 0;
    while (want[i]) {
        if (last[i] != want[i]) return 0;
        i++;
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
        emit("bwr=0x");
        emit_hex((unsigned long)bwr, 12);
        emit(" fields: ");
        unsigned long *fields = (unsigned long *)bwr;
        for (int fi = 0; fi < 6; fi++) {
            emit_hex(fields[fi], 16);
            emit(" ");
        }
        emit("| BINDER_WRITE_READ write_size=");
        emit_dec(bwr->write_size);
        emit(" read_size=");
        emit_dec(bwr->read_size);
        emit(" commands:");
        {
            /* The first bytes of the stream are the quickest way to see whether
             * a command starts at offset 0 or after a header. */
            emit(" [");
            unsigned char *raw = (unsigned char *)bwr->write_buffer;
            for (long i = 0; i < 96 && i < bwr->write_size; i++) {
                emit_hex(raw[i], 2);
                emit(" ");
            }
            emit("]");
        }
        int replied = dump_commands((unsigned char *)bwr->write_buffer, bwr->write_size);
        emit("\n");
        bwr->write_consumed = bwr->write_size;
        if (replied) {
            /* The reply is an empty Parcel: the generated AIDL proxy reads an
             * exception code (0 from an empty buffer) and then a strong binder,
             * which an empty buffer returns as null. That is the honest answer
             * while no service exists, and it keeps the failure in Java where
             * the framework can report it, instead of in the driver. */
            long written = 0;
            unsigned char *out = (unsigned char *)bwr->read_buffer;
            if (bwr->read_size >= 4 + TRANSACTION_DATA_SIZE) {
                unsigned int br_reply = BR_REPLY;
                __builtin_memcpy(out, &br_reply, 4);
                unsigned char *tr = out + 4;
                long zero = 0;
                for (int i = 0; i < TRANSACTION_DATA_SIZE; i++) tr[i] = 0;
                /* target.handle, code, flags, sender ids, sizes and pointers stay
                 * zero: an empty reply with no objects. */
                __builtin_memcpy(tr + 24, &zero, 8);  /* data_size */
                __builtin_memcpy(tr + 32, &zero, 8);  /* offsets_size */
                written = 4 + TRANSACTION_DATA_SIZE;
            }
            bwr->read_consumed = written;
            emit("binder-shim: replied with an empty parcel (");
            emit_dec(written);
            emit(" bytes)\n");
        } else {
            bwr->read_consumed = 0;
            if (saw_unknown_command) {
                emit("binder-shim: unparsed command stream; failing fast\n");
                flush_log();
                return -1;
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
    emit("binder-shim: property_find ");
    emit(name ? name : "(null)");
    emit("\n");

    typedef const void *(*next_fn)(const char *);
    static next_fn next;
    if (!next) {
        next = (next_fn)dlsym(RTLD_NEXT, "__system_property_find");
    }
    if (next) return next(name);
    return 0;
}
