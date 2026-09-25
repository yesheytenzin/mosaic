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
#define SYS_getpid 39
#define SYS_ioctl 16
#define SYS_ftruncate 77
#define SYS_epoll_ctl 233
#define SYS_openat 257
#define SYS_memfd_create 319
#define SYS_getuid 102
#define SYS_geteuid 107

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
/* Taking and giving back a reference to an object in another process. These were
 * consumed and forgotten, which is why an object died while a holder still
 * believed in it: the count was kept on one side of the socket only.
 * _IOW('c', 5, __u32) and _IOW('c', 6, __u32) -- the operand is the handle. */
#define BC_ACQUIRE BC(5, 4)
#define BC_RELEASE BC(6, 4)
/* The weak pair. `RefBase` keeps a strong count and a weak one, `BC_INCREFS` and
 * `BC_DECREFS` are how the weak one crosses processes, and dropping them is what
 * `RefBase: decWeak called on ... too many times` means: the object was
 * destroyed while a holder still had a weak reference to it. _IOW('c', 4) and
 * _IOW('c', 7). */
#define BC_INCREFS BC(4, 4)
#define BC_DECREFS BC(7, 4)
/* _IOW('c', 14, struct binder_ptr_cookie): a handle and the cookie to report it
 * with. The cookie is the caller's, and it is the whole point of the pair. */
#define BC_REQUEST_DEATH_NOTIFICATION BC(14, 16)
#define BC_CLEAR_DEATH_NOTIFICATION BC(15, 16)
/* The values below are checked against the image's own libbinder rather than the
 * kernel's header: it is the reader that decides, and it was compiled against its
 * own. `_IOR('r', 2, struct binder_transaction_data)` is present at size 64 and at
 * size 72 -- the second is the variant carrying a security context -- and
 * `_IOR('r', 3, ...)` is present at 64, which is what this sends for a reply. */
/* 64, and this time the number is backed by a measurement rather than a guess:
 * trying 72 makes the boot worse (`no answer for node` goes from 1 to 2), so this
 * reader walks the plain transaction data. The image's libbinder carries the command
 * at both sizes because it also handles the security-context variant; this build
 * reads the smaller one, which is the one the header describes as the default. */
#define BR_TRANSACTION BR(2, 64)
/* `_IO('r', 6)`: direction NONE, not READ. `BR(nr, size)` builds with the READ
 * direction, which is right for commands that carry a payload and wrong for the
 * ones that do not -- `_IO` has no direction at all, and the reader compares the
 * whole word. `BAD COMMAND -2147454458` is 0x80007206, which is this value built
 * with READ; the driver's is 0x00007206. `BR_NOOP` below is built with
 * `IOC(0, 'r', 12, 0)` for the same reason. */
#define BR_TRANSACTION_COMPLETE IOC(0, 'r', 6, 0)
/* _IOR('r', 15, binder_uintptr_t): the argument is the cookie the caller handed
 * to linkToDeath, which is what tells it *which* death this is. */
#define BR_DEAD_BINDER BR(15, 8)
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

/* Death notification, which the driver would do.
 *
 * A caller asks to hear about a handle dying and gives a cookie to be told with.
 * When the process on the other end goes away the driver hands that cookie back
 * as BR_DEAD_BINDER, on the reading thread. Nothing here is optional: a caller
 * that is not told keeps using a handle that no longer routes, and the failure
 * surfaces far away as a garbage status in the middle of an unrelated call.
 *
 * The cookie is the caller's and opaque, so it is carried, never interpreted. */
#define MAX_DEATHS 32

static struct {
    unsigned int handle;
    unsigned long long cookie;
} deaths[MAX_DEATHS];
static int death_count;
static unsigned int deaths_pending[MAX_DEATHS];
static int death_pending_count;

/* Told to the broker from the other half of the shim, which speaks its protocol.
 * The broker only needs the handle: it reports the death by handle, and the
 * cookie is the caller's and stays here. */
void shim_death_requested(unsigned int handle);
void shim_reference_taken(unsigned int handle);
void shim_reference_released(unsigned int handle);
/* The weak pair, which `RefBase` keeps for death notification. */
void shim_weak_reference_taken(unsigned int handle);
void shim_weak_reference_released(unsigned int handle);
void shim_death_cleared(unsigned int handle);

static void death_remember(unsigned int handle, unsigned long long cookie) {
    for (int i = 0; i < death_count; i++) {
        if (deaths[i].handle == handle) {
            deaths[i].cookie = cookie;
            return;
        }
    }
    if (death_count < MAX_DEATHS) {
        deaths[death_count].handle = handle;
        deaths[death_count].cookie = cookie;
        death_count++;
    }
}

static void death_forget(unsigned int handle) {
    for (int i = 0; i < death_count; i++) {
        if (deaths[i].handle != handle) continue;
        deaths[i] = deaths[death_count - 1];
        death_count--;
        return;
    }
}

/* An incoming transaction: the broker asking this process to serve a call on an
 * object it owns.
 *
 * It has to reach the process's own binder thread as BR_TRANSACTION, which is what
 * the driver would write -- the object is the caller's, its cookie is the only
 * thing that finds it, and the framework's Binder is the only thing that can run
 * it. Serving it here instead, which this did, leaves the framework's binder
 * thread with nothing to read and no way to be called back. */
#define MAX_INCOMING 8

/* The strong hold a transaction takes on its target, which the driver takes and
 * this side did not.
 *
 * On a device the kernel keeps the target alive for the duration of the call and
 * drops it after, and that is the reference that makes the difference between an
 * object being destroyed inside its own transaction and being destroyed when the
 * last holder lets go. Without it, `IPCThreadState`'s own balanced `sp<BBinder>`
 * around the call is the *last* reference, the object dies at the end of the
 * call, and the weak release that follows is against a count already gone:
 * `RefBase: decWeak called on ... too many times`. */
static void *strong_hold;
typedef void (*strong_fn)(void *);
static strong_fn refbase_inc_strong;
static strong_fn refbase_dec_strong;

void shim_resolve_refbase(void *incref_symbol, void *decref_symbol) {
    refbase_inc_strong = (strong_fn)incref_symbol;
    refbase_dec_strong = (strong_fn)decref_symbol;
}

/* A reference kept for good, by a side that will never give it back: what a
 * registry does for a service it now owns, and what the fallback reader needed --
 * it returned an object with no reference at all, so anything the framework
 * registered from a temporary died underneath the registry. */
void shim_hold_object(void *object) {
    if (object && refbase_inc_strong) refbase_inc_strong(object);
}

/* The driver holds a strong reference on a transaction's target for the duration of
 * the call, and this tried to mirror that by calling `RefBase::incStrong` on the
 * object directly. That crashes: the pointer is not always a `RefBase` -- it depends
 * on what the publisher put in the word -- and `incStrong` on it faults inside
 * libutils, which is a worse failure than the one it was there to prevent.
 *
 * It was written for `decWeak called ... too many times`, and the core later showed
 * that message is a static object's destructor during `exit`: shutdown noise, not a
 * live count. So the emulation is not needed, and a crash it introduces is not a
 * trade worth making. If it is ever wanted, the object has to be *known* to be a
 * `RefBase` first, which means the publisher saying so rather than this side
 * guessing from a word that sometimes is one. */
static void hold_target(void *object, unsigned int flags) {
    (void)object;
    (void)flags;
}

/* The call has been answered. See `hold_target`: there is nothing to release. */
void shim_release_target(void) {}

static struct {
    void *object;
    unsigned long long cookie;
    unsigned int code;
    unsigned int flags;
    unsigned char *data;
    unsigned long size;
    /// How many of the body's trailing words are object offsets, which the reader
    /// needs: it walks them to find the objects among the arguments, and a count of
    /// zero over a body that has them is what it cannot survive.
    unsigned int objects;
} incoming[MAX_INCOMING];
static int incoming_count;

void shim_incoming(void *object, unsigned long long cookie, unsigned int code,
                   unsigned int flags, const unsigned char *data, unsigned long size,
                   unsigned int objects) {
    if (incoming_count >= MAX_INCOMING) return;
    unsigned char *copy = (unsigned char *)malloc(size ? size : 1);
    if (!copy) return;
    if (size) __builtin_memcpy(copy, data, size);
    /* Held rather than freed after the read: the reader keeps the pointer and
     * frees it when it is done with the parcel, by sending BC_FREE_BUFFER -- the
     * same way a reply's buffer is handled. */
    hold(copy, 0);
    incoming[incoming_count].object = object;
    incoming[incoming_count].cookie = cookie;
    incoming[incoming_count].code = code;
    incoming[incoming_count].flags = flags;
    incoming[incoming_count].data = copy;
    incoming[incoming_count].size = size;
    incoming[incoming_count].objects = objects;
    incoming_count++;
}

/* Write one incoming transaction, if there is one and there is room.
 *
 * The command stream is commands only: the reader advances by the size each
 * command's word carries, so the parcel cannot live inline. `data.ptr.buffer`
 * points at the parcel and the reader follows it, which is what the reply path
 * already does with a buffer it holds. Writing the data after the struct and
 * counting it in `read_consumed`, which this did, put the reader's next command
 * wherever the arithmetic landed -- and what it read there was a pointer. */
static long incoming_write(unsigned char *out, long capacity) {
    if (incoming_count == 0) return 0;
    if (capacity < 4 + 64) return 0;
    unsigned int command = BR_TRANSACTION;
    __builtin_memcpy(out, &command, 4);
    unsigned char *tr = out + 4;
    for (int i = 0; i < 64; i++) tr[i] = 0;
    /* `target.ptr` is the object itself -- the reader calls a virtual method on it
     * -- and `cookie` is the publishing process's cookie, which its `onTransact`
     * sees. Putting the cookie in both, which this did, is a virtual call through
     * a value that is not a vtable. */
    if (tracing_on() == 0 && incoming[0].object) {
        emit("binder-shim: target 0x");
        {
            unsigned long v = (unsigned long)incoming[0].object;
            char digit[2];
            for (int shift = 60; shift >= 0; shift -= 4) {
                digit[0] = "0123456789abcdef"[(v >> shift) & 0xf];
                digit[1] = 0;
                emit(digit);
            }
        }
        emit("\n");
        flush_log();
    }
    hold_target(incoming[0].object, incoming[0].flags);
    unsigned long long object = (unsigned long long)incoming[0].object;
    __builtin_memcpy(tr + 0, &object, 8);   /* target.ptr */

    unsigned long long cookie = incoming[0].cookie;
    __builtin_memcpy(tr + 8, &cookie, 8);   /* cookie */
    unsigned int code = incoming[0].code;
    __builtin_memcpy(tr + 16, &code, 4);
    /* Only the flags the driver defines. The framework sends `2` for this call and
     * the kernel has no such value -- TF_ONE_WAY 0x01, TF_ACCEPT_FDS 0x10,
     * TF_CLEAR_BUF 0x20, TF_UPDATE_TXN 0x40 -- so a real driver would never deliver
     * it, and a vendored handler that acts on the word rather than ignoring it is a
     * handler that never answers. Stripping what cannot exist is what the driver
     * does; passing it through is what this did. */
    unsigned int flags = incoming[0].flags & 0x71u;
    __builtin_memcpy(tr + 20, &flags, 4);
    unsigned long size = incoming[0].size;
    __builtin_memcpy(tr + 32, &size, 8);    /* data_size */
    /* The sender, which the reader reports to the object it calls. Zero is not a
     * pid, and the object this is going to is the framework's own. */
    unsigned int pid = (unsigned int)syscall(SYS_getpid);
    __builtin_memcpy(tr + 24, &pid, 4);     /* sender_pid */
    unsigned long long buffer = (unsigned long long)incoming[0].data;
    __builtin_memcpy(tr + 48, &buffer, 8);  /* data.ptr.buffer */
    /* The object offsets ride at the end of the body, four bytes each, and the
     * size of that table is part of what the reader walks. */
    unsigned long offsets_size = (unsigned long)incoming[0].objects * 4;
    __builtin_memcpy(tr + 40, &offsets_size, 8);   /* offsets_size */
    unsigned long long offsets = (unsigned long long)(incoming[0].data + size -
                                                      offsets_size);
    __builtin_memcpy(tr + 56, &offsets, 8);        /* data.ptr.offsets */
    incoming_count--;
    for (int i = 0; i < incoming_count; i++) incoming[i] = incoming[i + 1];
    return 4 + 64;
}

/* A death the broker reported. Delivered on the next read, because that is when
 * the caller is listening. */
void shim_death_arrived(unsigned int handle) {
    if (death_pending_count < MAX_DEATHS) deaths_pending[death_pending_count++] = handle;
}

static long death_write(unsigned char *out, long capacity) {
    if (death_pending_count == 0) return 0;
    unsigned int handle = deaths_pending[0];
    death_pending_count--;
    for (int i = 0; i < death_pending_count; i++) deaths_pending[i] = deaths_pending[i + 1];
    if (capacity < 12) return 0;
    unsigned int command = BR_DEAD_BINDER;
    __builtin_memcpy(out, &command, 4);
    unsigned long long cookie = 0;
    for (int i = 0; i < death_count; i++) {
        if (deaths[i].handle == handle) cookie = deaths[i].cookie;
    }
    __builtin_memcpy(out + 4, &cookie, 8);
    death_forget(handle);
    return 12;
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
        } else if (command == BC_INCREFS || command == BC_DECREFS) {
            unsigned int handle;
            __builtin_memcpy(&handle, buffer + offset, 4);
            if (command == BC_INCREFS) {
                shim_weak_reference_taken(handle);
            } else {
                shim_weak_reference_released(handle);
            }
        } else if (command == BC_REPLY) {
            /* The call has been answered: the target's hold goes, which is what the
             * driver does when the transaction completes. */
            shim_release_target();
        } else if (command == BC_ACQUIRE || command == BC_RELEASE) {
            /* A reference to an object another process owns. The count belongs to
             * the side that can see every holder, so it is forwarded rather than
             * kept here -- and it used to be dropped, which is how an object came
             * to be destroyed while someone still believed they held it. */
            unsigned int handle;
            __builtin_memcpy(&handle, buffer + offset, 4);
            if (command == BC_ACQUIRE) {
                shim_reference_taken(handle);
            } else {
                shim_reference_released(handle);
            }
        } else if (command == BC_REQUEST_DEATH_NOTIFICATION ||
                   command == BC_CLEAR_DEATH_NOTIFICATION) {
            unsigned int handle;
            unsigned long long cookie;
            __builtin_memcpy(&handle, buffer + offset, 4);
            __builtin_memcpy(&cookie, buffer + offset + 4, 8);
            if (command == BC_REQUEST_DEATH_NOTIFICATION) {
                death_remember(handle, cookie);
                shim_death_requested(handle);
            } else {
                death_forget(handle);
                shim_death_cleared(handle);
            }
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

/* The binder devices, each with its own placeholder descriptor.
 *
 * One `binder_fd` was enough while only /dev/binder mattered. With hwbinder and
 * vndbinder in play the *last* device opened took the slot, so the shim answered
 * the wrong device's ioctls afterwards -- including the service manager's, which is
 * `/dev/binder`'s alone. The health HAL is HIDL and goes through /dev/hwbinder, so
 * this is where that showed up.
 *
 * `is_hwbinder` is what the routing needs next: handle 0 on that device is the HIDL
 * service manager, which is a daemon of its own (`hwservicemanager`), not the
 * registry this shim keeps for /dev/binder. */
#define MAX_BINDER_DEVICES 4
static struct binder_device {
    int fd;
    int is_hwbinder;
} binder_devices[MAX_BINDER_DEVICES];
static int binder_device_count = 0;

static struct binder_device *device_for(int fd) {
    for (int i = 0; i < binder_device_count; i++) {
        if (binder_devices[i].fd == fd) return &binder_devices[i];
    }
    return 0;
}

static int is_binder_device(int fd) { return device_for(fd) != 0; }

static int remember_device(int fd, int is_hwbinder) {
    if (binder_device_count < MAX_BINDER_DEVICES) {
        binder_devices[binder_device_count].fd = fd;
        binder_devices[binder_device_count].is_hwbinder = is_hwbinder;
        binder_device_count++;
    }
    return fd;
}

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
    } else if (strcmp(path, "/data") == 0) {
        /* The directory itself, which is what `statvfs` and `stat` are asked about
         * when the framework starts a provider -- with no trailing slash, so the rule
         * above does not match it, and the host has no `/data`:
         *
         *   java.lang.IllegalArgumentException: Invalid path: /data
         *   Caused by: android.system.ErrnoException: statvfs failed: ENOENT
         */
        prefix = "/data";
        rest = "data";
    } else if (strcmp(path, "/apex") == 0) {
        /* The directory itself, which the framework opens to list the apexes --
         * with no trailing slash, so the rule below does not match it. */
        prefix = "/apex";
        rest = "apex";
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
            /* Anything else under an apex -- its priv-app, its etc, its bin --
             * is carried in the bundle under apex/, so it resolves there. The
             * framework looks for the extension package's apk under its apex's
             * priv-app directory and refuses to finish the boot when the package
             * it names is not there: "Required services extension package is
             * missing". */
            prefix = "/apex";
            rest = path + 1;
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

/* The uid the framework is told this process has.
 *
 * `Process.myUid()` is `getuid()`, and the framework checks it against Android's
 * `SYSTEM_UID` (1000) in a great many places. The first one to run refuses the
 * boot outright:
 *
 *   PackageManager: Non System Server process reporting dex loads as system server. uid=0
 *   java.lang.SecurityException: Non-system caller
 *     at IPackageManagerBase.getSetupWizardPackageName(IPackageManagerBase.java:769)
 *
 * The harness runs the framework as the user namespace's *root*, because it needs
 * the namespace's capabilities: a private /dev from a mount, and the property area's
 * files, which libc requires to be owned by root -- and the namespace maps one uid,
 * so root and 1000 cannot both be had.
 *
 * On a device the process *is* the system uid, so reporting it is what the harness
 * means; MOSAIC_UID names it. The real uid is untouched: only these two calls lie,
 * so file access is still the kernel's business and nothing about permissions
 * changes.
 */
static unsigned int reported_uid(unsigned int real) {
    static int configured = -1;
    static unsigned int value = 0;
    if (configured < 0) {
        const char *v = getenv("MOSAIC_UID");
        unsigned int parsed = 0;
        for (const char *p = v; p && *p >= '0' && *p <= '9'; p++) parsed = parsed * 10 + (unsigned)(*p - '0');
        value = parsed;
        configured = value ? 1 : 0;
    }
    return configured ? value : real;
}

unsigned int getuid(void) {
    return reported_uid((unsigned int)syscall(SYS_getuid));
}

unsigned int geteuid(void) {
    return reported_uid((unsigned int)syscall(SYS_geteuid));
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

/* ---- ashmem, which no host has ------------------------------------------ */

/* Android allocates shared memory through `/dev/ashmem`, and there is no such
 * device here and no node to make: the region is a kernel driver's. What the
 * device gives is a file descriptor that can be sized and mapped, and a memfd is
 * exactly that -- the same substitution the binder device gets, one caller over.
 *
 * `ashmem_create_region` is the caller, and it does not stop at the open: it sets
 * the name and the size with ioctls and gives up if either fails, so those have to
 * be answered too. The size is the one that matters -- it is what makes the region
 * as large as the caller asked before it maps it.
 *
 *   ashmem: Unable to open ashmem device /dev/ashmem<name> ... and /dev/ashmem
 *   java.io.IOException: ashmem creation failed
 *     at android.util.MemoryIntArray.nativeCreate
 */
#define ASHMEM_NAME_LEN 256
#define __ASHMEMIOC 0x77
#define ASHMEM_SET_NAME (1u << 30 | (ASHMEM_NAME_LEN << 16) | (__ASHMEMIOC << 8) | 1)
#define ASHMEM_GET_NAME (2u << 30 | (ASHMEM_NAME_LEN << 16) | (__ASHMEMIOC << 8) | 2)
#define ASHMEM_SET_SIZE (1u << 30 | (8 << 16) | (__ASHMEMIOC << 8) | 3)
#define ASHMEM_GET_SIZE (__ASHMEMIOC << 8 | 4)
#define ASHMEM_SET_PROT_MASK (1u << 30 | (8 << 16) | (__ASHMEMIOC << 8) | 5)
#define ASHMEM_GET_PROT_MASK (__ASHMEMIOC << 8 | 6)
#define ASHMEM_PIN (1u << 30 | (8 << 16) | (__ASHMEMIOC << 8) | 7)
#define ASHMEM_UNPIN (1u << 30 | (8 << 16) | (__ASHMEMIOC << 8) | 8)
#define ASHMEM_GET_PIN_STATUS (__ASHMEMIOC << 8 | 9)
#define ASHMEM_PURGE_ALL_CACHES (__ASHMEMIOC << 8 | 10)

#define MAX_ASHMEM 8
static int ashmem_fds[MAX_ASHMEM];
static long ashmem_sizes[MAX_ASHMEM];
static int ashmem_count = 0;

static void remember_ashmem(int fd) {
    if (ashmem_count < MAX_ASHMEM) {
        ashmem_fds[ashmem_count] = fd;
        ashmem_sizes[ashmem_count] = 0;
        ashmem_count++;
    }
}

static int ashmem_index(int fd) {
    for (int i = 0; i < ashmem_count; i++) {
        if (ashmem_fds[i] == fd) return i;
    }
    return -1;
}

static int is_ashmem(int fd) { return ashmem_index(fd) >= 0; }

/* Bionic's <fcntl.h> defines open() as an inline wrapper around __openat, so a
 * preloaded open() is never called. These are the symbols that matter. */
int __openat(int dirfd, const char *path, int flags, int mode) {
    /* The framework hardcodes paths that cannot be configured, so they are
     * rewritten first; then the binder device is intercepted. */
    path = redirect(path);
    if (strncmp(path, "/dev/ashmem", 11) == 0) {
        int fd = make_placeholder_fd();
        if (fd < 0) return fd;
        remember_ashmem(fd);
        emit_prefixed("open ", path);
        emit("binder-shim:   -> ashmem placeholder fd ");
        emit_dec(fd);
        emit("\n");
        flush_log();
        return fd;
    }
    if (name_is_binder(path)) {
        int fd = make_placeholder_fd();
        /* Which device it is decides what handle 0 means on it. */
        int hwbinder = 0;
        for (const char *p = path; *p; p++) {
            if (p[0] == 'h' && p[1] == 'w' && p[2] == 'b') hwbinder = 1;
        }
        remember_device(fd, hwbinder);
        emit_prefixed("open ", path);
        emit("binder-shim:   -> placeholder fd ");
        emit_dec(fd);
        emit(hwbinder ? " (hwbinder)\n" : "\n");
        return fd;
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

/* What a memfd has to *look* like. `libcutils` does not stop at opening the
 * region: it checks that what it opened is the device.
 *
 *   if (!S_ISCHR(st.st_mode) || !st.st_rdev) { close(fd); errno = ENOTTY; return -1; }
 *
 * A memfd is a regular file, so that check refuses it and every caller sees
 * "ashmem creation failed" even though the open succeeded. `fstat` on a descriptor
 * this side handed out as ashmem therefore reports a character device with a
 * nonzero device number -- any one will do: the caller remembers the first it sees
 * and compares later opens against it.
 *
 * No headers here. probe is built `-nostdlib` against no sysroot, so the two fields
 * are addressed by their offsets in Bionic's LP64 `struct stat`, which is the struct
 * the caller reads: `st_mode` at 16, four bytes; `st_rdev` at 32, eight. */
/* The GNU C library and Bionic agree here: `st_dev` and `st_ino` are eight bytes
 * each and `st_nlink` is eight too, so `st_mode` sits at 24 and `st_rdev` at 40. The
 * first version of this patched 16 and 32 -- the *link count* and *gid* -- which is
 * how you prove to yourself that an offset with no name is a number. */
#define BIONIC_STAT_MODE_OFFSET 24
#define BIONIC_STAT_RDEV_OFFSET 40
#define S_IFCHR_VALUE 0020000
#define ASHMEM_RDEV_VALUE 0x0a37ul /* makedev(10, 55), the misc device ashmem uses */

int fstat(int fd, void *buf) {
    typedef int (*real_fn)(int, void *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "fstat");
    if (!real) return -1;
    int status = real(fd, buf);
    if (status == 0 && buf && is_ashmem(fd)) {
        unsigned char *bytes = (unsigned char *)buf;
        unsigned int mode = 0;
        __builtin_memcpy(&mode, bytes + BIONIC_STAT_MODE_OFFSET, 4);
        mode = (mode & ~0170000u) | S_IFCHR_VALUE;
        __builtin_memcpy(bytes + BIONIC_STAT_MODE_OFFSET, &mode, 4);
        unsigned long rdev = ASHMEM_RDEV_VALUE;
        __builtin_memcpy(bytes + BIONIC_STAT_RDEV_OFFSET, &rdev, 8);
    }
    return status;
}

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

/* `statvfs` and `statfs`, which is how the framework asks how much room a directory
 * has. `PackageManagerService` starts a provider by checking the space on its data
 * directory, and `statvfs("/data")` without a redirect finds nothing:
 *
 *   java.lang.IllegalArgumentException: Invalid path: /data
 *   Caused by: android.system.ErrnoException: statvfs failed: ENOENT
 *
 * The other path-taking calls were covered; these two were not, and the provider is
 * the first thing that asks. */
/* Files and directories are made, not only asked about. `DropBoxManagerService`
 * keeps crash logs under /data/system/dropbox, which the boot never got far enough to
 * make, and it creates its own directory -- but everything under /data lives in the
 * bundle, so the open that makes it has to be rewritten before the kernel sees it,
 * the same as every other path here. The rest of the directory calls ride along on
 * the same shape: small, loud when missing, and not worth a second round each. */
int mkdir(const char *path, unsigned int mode) {
    typedef int (*real_fn)(const char *, unsigned int);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "mkdir");
    return real ? real(redirect(path), mode) : -1;
}

int mkdirat(int dirfd, const char *path, unsigned int mode) {
    typedef int (*real_fn)(int, const char *, unsigned int);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "mkdirat");
    return real ? real(dirfd, redirect(path), mode) : -1;
}

int rename(const char *from, const char *to) {
    typedef int (*real_fn)(const char *, const char *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "rename");
    return real ? real(redirect(from), redirect(to)) : -1;
}

int renameat(int fromfd, const char *from, int tofd, const char *to) {
    typedef int (*real_fn)(int, const char *, int, const char *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "renameat");
    return real ? real(fromfd, redirect(from), tofd, redirect(to)) : -1;
}

int renameat2(int fromfd, const char *from, int tofd, const char *to, unsigned int flags) {
    typedef int (*real_fn)(int, const char *, int, const char *, unsigned int);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "renameat2");
    return real ? real(fromfd, redirect(from), tofd, redirect(to), flags) : -1;
}

int unlink(const char *path) {
    typedef int (*real_fn)(const char *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "unlink");
    return real ? real(redirect(path)) : -1;
}

int unlinkat(int dirfd, const char *path, int flags) {
    typedef int (*real_fn)(int, const char *, int);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "unlinkat");
    return real ? real(dirfd, redirect(path), flags) : -1;
}

int statvfs(const char *path, void *buf) {
    typedef int (*real_fn)(const char *, void *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "statvfs");
    return real ? real(redirect(path), buf) : -1;
}

int statvfs64(const char *path, void *buf) {
    typedef int (*real_fn)(const char *, void *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "statvfs64");
    return real ? real(redirect(path), buf) : -1;
}

int statfs(const char *path, void *buf) {
    typedef int (*real_fn)(const char *, void *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "statfs");
    return real ? real(redirect(path), buf) : -1;
}

int statfs64(const char *path, void *buf) {
    typedef int (*real_fn)(const char *, void *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "statfs64");
    return real ? real(redirect(path), buf) : -1;
}

int statx(int dirfd, const char *path, int flags, unsigned int mask, void *buf) {
    typedef int (*real_fn)(int, const char *, int, unsigned int, void *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "statx");
    return real ? real(dirfd, redirect(path), flags, mask, buf) : -1;
}

/* The call the Java `waitForService` makes. `ServiceManager.waitForService` is
 * `Binder.allowBlocking(waitForServiceNative(name))`, and this is the NDK function
 * behind it. Interposed for the same reason as the check above: the health HAL is
 * declared now and the framework waits a second for the service, and this says
 * whether the wait arrives here at all. */
void *AServiceManager_waitForService(const char *instance) {
    typedef void *(*real_fn)(const char *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "AServiceManager_waitForService");
    void *result = real ? real(instance) : 0;
    emit("android-binder: waitForService(");
    emit(instance ? instance : "(null)");
    emit(result ? ") -> found\n" : ") -> null\n");
    flush_log();
    return result;
}

/* What `ServiceManager.waitForDeclaredService` actually asks, and what it is told.
 *
 * The AIDL health HAL is not found even though it is declared in the manifest this
 * side carries, and the check that decides is this one. A name that arrives and a
 * "no" that comes back says the manifest is not being read; nothing arriving says
 * the check is somewhere else entirely. */
int AServiceManager_isDeclared(const char *instance) {
    typedef int (*real_fn)(const char *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "AServiceManager_isDeclared");
    int result = real ? real(instance) : 0;
    emit("android-binder: isDeclared(");
    emit(instance ? instance : "(null)");
    emit(result ? ") -> yes\n" : ") -> no\n");
    flush_log();
    return result;
}

/* `fopen`, because a library that reads a file through it does not come through
 * `open` at all: `fopen` calls `open` *inside libc*, and a preloaded `open` is not
 * called for an internal call. That is why the framework could not find the VINTF
 * manifest this side carries -- `libvintf` uses `fopen`, the path was never
 * redirected, and the file it looked for does not exist on a host. */
void *fopen(const char *path, const char *mode) {
    typedef void *(*real_fn)(const char *, const char *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "fopen");
    return real ? real(redirect(path), mode) : 0;
}

void *fopen64(const char *path, const char *mode) {
    typedef void *(*real_fn)(const char *, const char *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "fopen64");
    return real ? real(redirect(path), mode) : 0;
}

/* The one that matters for a *directory*: `File.listFiles()` is `opendir` and
 * `readdir`, and a path this shim redirects for `open` and `stat` is not redirected
 * for `opendir` unless it is named here. That is what kept the apex list empty:
 * `ApexManagerFlattenedApex` -- the implementation a *flattened* apex build uses,
 * which never asks the apex service -- lists `/apex` itself, found nothing, and the
 * package manager went on to abort on a package sitting in the bundle. */
void *opendir(const char *path) {
    typedef void *(*real_fn)(const char *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "opendir");
    return real ? real(redirect(path)) : 0;
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
    if (is_binder_device(fd)) {
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

    if (is_ashmem(fd)) {
        int index = ashmem_index(fd);
        if (request == ASHMEM_SET_NAME || request == ASHMEM_SET_PROT_MASK) return 0;
        if (request == ASHMEM_SET_SIZE) {
            long size = (long)arg;
            if (syscall(SYS_ftruncate, fd, size) < 0) return -1;
            ashmem_sizes[index] = size;
            return 0;
        }
        if (request == ASHMEM_GET_SIZE) return (int)ashmem_sizes[index];
        if (request == ASHMEM_GET_PROT_MASK) return 3; /* PROT_READ | PROT_WRITE */
        if (request == ASHMEM_GET_NAME) {
            if (arg) ((char *)arg)[0] = 0;
            return 0;
        }
        /* This side has no pinning and nothing to purge: a region is ordinary
         * memory here, and the calls that manage the driver's cache are answered
         * rather than refused, because a caller that fails them treats the region
         * as unusable. */
        return 0;
    }

    if (!is_binder_device(fd)) {
        return (int)syscall(SYS_ioctl, fd, request, arg);
    }

    /* Every ioctl, which is the loudest thing here and the least useful once the
     * surface is known: it fills the log's line budget, and a line written after
     * the budget runs out is simply dropped, so an instrument added to answer a
     * later question never appears. Behind MOSAIC_BINDER_DEBUG with the rest of
     * the tracing. */
    if (tracing_on()) {
        emit("binder-shim: ioctl 0x");
        emit_hex(request, 8);
        emit(" ");
    }
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
        struct binder_device *device = device_for(fd);
        emit(device && device->is_hwbinder ? "BINDER_SET_CONTEXT_MGR (hwbinder) -> ok\n"
                                           : "BINDER_SET_CONTEXT_MGR -> ok\n");
        return 0;
    }
    if (request == BINDER_WRITE_READ) {
        struct binder_write_read *bwr = (struct binder_write_read *)arg;

        if (bwr->write_size > 0 && bwr->write_buffer) {
            /* Behind the trace flag with the ioctl line: this is the second loudest
             * thing here, and the log's line budget is finite -- a line written after
             * it runs out is dropped, so an instrument added to answer a later
             * question never appears. That has cost this work several rounds. */
            if (tracing_on()) {
                emit("binder-shim: write=");
                emit_dec(bwr->write_size);
                emit(" read=");
                emit_dec(bwr->read_size);
                emit("\n");
            }
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
            /* A death comes first: it is what the caller is waiting for, and a
             * thread that has gone to sleep waiting for work will not ask again
             * until something arrives. */
            long death = death_write((unsigned char *)bwr->read_buffer, bwr->read_size);
            if (death > 0) {
                bwr->read_consumed = death;
                flush_log();
                return 0;
            }
            /* An incoming transaction comes before a reply: it is work, and the
             * thread reading is the one that has to do it. */
            long arriving = incoming_write((unsigned char *)bwr->read_buffer, bwr->read_size);
            if (arriving > 0) {
                bwr->read_consumed = arriving;
                flush_log();
                return 0;
            }
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
    if (is_binder_device(fd)) {
        emit("binder-shim: close of the binder fd\n");
        /* Nothing to clear: the device table holds the placeholders. */
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
