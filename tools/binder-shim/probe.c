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
typedef unsigned long ulong;
/* Keep clang from treating this headerless shim's interposed libc declaration as
 * an incompatible redeclaration of the compiler built-in. */
void *fopen(const char *, const char *) __attribute__((nothrow));
typedef long ssize_t;
typedef long off_t;

extern long syscall(long number, ...);
extern void *malloc(unsigned long);
extern void free(void *);

#define SYS_read 0
#define SYS_write 1
#define SYS_close 3
#define SYS_dup 32
#define SYS_poll 7
#define SYS_mmap 9
#define SYS_nanosleep 35
#define SYS_getpid 39
#define SYS_ioctl 16
#define SYS_ftruncate 77
#define SYS_epoll_ctl 233
#define SYS_memfd_create 319
#define SYS_newfstatat 262
#define SYS_getrandom 318
#define SYS_write 1
#define SYS_lseek 8
#define SYS_openat 257
#define SYS_getuid 102
#define SYS_geteuid 107
#define SYS_eventfd2 290

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

/* Report each rewritten path once, so a run stays readable. The cap is high
 * enough for a whole boot: at 24 the paths a framework process asks for early --
 * the VINTF manifests, the apex trees -- were dropped from the log by the ones
 * before them, which is exactly the question the log was being read to answer. */
#define MAX_REDIRECTIONS 1024
static const char *redirected[MAX_REDIRECTIONS];
static int redirected_count = 0;

/* One stored copy of each path already reported. The copies are the shim's own:
 * `from` belongs to the caller, and a redirect's caller is usually a transient
 * `std::string` -- the string is gone by the time the next redirect compares
 * against it, and `strcmp` on it faults at address 0. That is exactly how this
 * broke: a new rule made new paths appear, the dedup reached a stale pointer, and
 * the fault came back through `libbase`'s `ReadFileToString`. */
static char reported_paths[MAX_REDIRECTIONS][256];

static void report(const char *from, const char *to) {
    if (redirected_count >= MAX_REDIRECTIONS) return;
    for (int i = 0; i < redirected_count; i++) {
        if (strcmp(reported_paths[i], from) == 0) return;
    }
    ulong n = 0;
    while (from[n] && n < sizeof(reported_paths[0]) - 1) {
        reported_paths[redirected_count][n] = from[n];
        n++;
    }
    reported_paths[redirected_count][n] = 0;
    /* Two statements, not one: `redirected[redirected_count++]` next to
     * `reported_paths[redirected_count - 1]` reads and writes the same object
     * without a sequence point between them, which is undefined and which the
     * shim build reports. */
    redirected[redirected_count] = reported_paths[redirected_count];
    redirected_count++;
    if (!tracing_on()) return;
    emit("android-paths: ");
    emit(from);
    emit(" -> ");
    emit(to);
    emit("\n");
    flush_log();
}
static void flush_log(void);
/* Thread-local, like the redirect buffer and for the same reason: the framework
 * opens paths and lists directories from many threads, and one shared buffer means
 * one thread's line is built from another thread's bytes. That is how two DIRINODE
 * lines for two different paths came to share an fd and carry each other's inode. */
static __thread char log_buffer[16384];
static __thread long log_length = 0;

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
/* libhwbinder -- the hwbinder side, the one HIDL speaks -- does not write
 * `BC_TRANSACTION`. It writes `BC_TRANSACTION_SG` and `BC_REPLY_SG`, which
 * carry a `binder_transaction_data_sg`: the same 64-byte transaction followed by
 * a 4-byte buffer count that the ABI aligns out to 8, so the whole operand is 72
 * bytes. They are numbers 17 and 18 in the same 'c' space, so `BC(17, 72)` and
 * `BC(18, 72)`.
 *
 * The walker below compares whole command words, so a stream written that way
 * matched nothing at all: every HIDL transaction was skipped as an unknown
 * command, no answer was ever produced, and the visible ends of that are
 *
 *   HidlServiceManagement: getService: defaultServiceManager() is null
 *   Cannot register android.frameworks.sensorservice@1.0::ISensorManager: -38
 *
 * which is how `SystemServer.startHidlServices` kills the boot. */
#define BC_TRANSACTION_SG BC(17, 72)
#define BC_REPLY_SG BC(18, 72)
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

/* The HIDL service manager, for handle 0 on an *hwbinder* device.
 *
 * Handle 0 on `/dev/binder` is `android.os.IServiceManager` and this shim answers
 * it in-process; handle 0 on `/dev/hwbinder` is a different interface,
 * `android.hidl.manager@1.0::IServiceManager`, with HIDL's marshalling, and it is
 * the one `SystemServer.startHidlServices` needs: its first registration is fatal
 * by construction (`LOG_ALWAYS_FATAL_IF(err != OK, "Cannot register %s: %d", ...)`).
 *
 * Answering that transaction with the AIDL service manager's reply -- which this
 * did, because both doors reached `mosaic_binder_reply` -- produced no answer at
 * all: the AIDL handler rejected the request (its interface token is not there)
 * and the HIDL client read its status out of an empty parcel.
 *
 * Defined in android-binder.c, where the registry and the broker are. */
extern int mosaic_hidl_service_manager(unsigned int code, const unsigned char *request,
                                       unsigned long request_size,
                                       const unsigned long *argument_offsets,
                                       unsigned long argument_count, unsigned char **out_data,
                                       unsigned long *out_size);


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

/* Whether the transaction being answered arrived on /dev/hwbinder, which is what
 * decides who owns handle 0. Set per thread around the write stream, because
 * the answer is produced while the stream is walked. */
static __thread int transaction_on_hwbinder = 0;

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
    int produced;
    if (transaction_on_hwbinder && tr->target_handle == 0) {
        /* Handle 0 on hwbinder is the HIDL service manager, and its reply is a HIDL
         * parcel: no AIDL interface token, HIDL's method codes, and the status in
         * the reply's own first word. Nothing else here answers that interface, so
         * a call that reaches this point goes there or is not answered at all. */
        produced = mosaic_hidl_service_manager(tr->code, (const unsigned char *)tr->data_buffer,
                                              tr->data_size, (const unsigned long *)tr->data_offsets,
                                              tr->offsets_size / sizeof(unsigned long), &data,
                                              &data_size);
    } else {
        produced = mosaic_binder_reply(tr->target_handle, tr->code,
                                       (const unsigned char *)tr->data_buffer, tr->data_size,
                                       (const unsigned long *)tr->data_offsets,
                                       tr->offsets_size / sizeof(unsigned long), &data,
                                       &data_size, &objects, &objects_count);
    }
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
static int hidl_decodes = 0;

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

/* The reference callbacks are resolved for the registry's permanent holds. The
 * driver-style per-transaction hold was removed after it was measured to call
 * `RefBase::incStrong` on objects that are not `RefBase` instances. */
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
        if (command == BC_TRANSACTION || command == BC_TRANSACTION_SG) {
            /* Both carry the 64-byte transaction at the head of their operand --
             * the SG variant only appends a buffer count after it -- so the same
             * struct is read either way. */
            struct binder_transaction_data *tr =
                (struct binder_transaction_data *)(buffer + offset);
            emit(command == BC_TRANSACTION_SG ? "binder-shim: hwbinder tr handle="
                                              : "binder-shim: tr handle=");
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
            /* The HIDL service manager's own parcel, decoded: what the kernel would
             * translate for the receiver, read here because the pointers are this
             * process's own. The interface token is the first string; each argument
             * is a `binder_buffer_object` (type BINDER_TYPE_PTR) in the object
             * table naming a buffer and its length, and *those* hold the strings.
             *
             * This is how the HIDL `IServiceManager`'s method codes were learned
             * rather than guessed: the strings say which call it is
             * (`android.hardware.power.stats@1.0::IPowerStats` is a `get`,
             * `default` beside a service is an `add`) and the code bytes say how it
             * is numbered. */
            if (command == BC_TRANSACTION_SG && tr->target_handle == 0
                && hidl_decodes < 3 && tr->data_buffer) {
                hidl_decodes++;
                const unsigned char *data = (const unsigned char *)tr->data_buffer;
                const unsigned int *offsets = (const unsigned int *)tr->data_offsets;
                unsigned long count = tr->offsets_size / 4;
                emit("binder-shim: hidl parcel code=");
                emit_dec((long)tr->code);
                emit(" bytes=");
                emit_dec((long)tr->data_size);
                emit(" objects=");
                emit_dec((long)count);
                emit("\n");
                /* The token and the inline part, as text. */
                emit("binder-shim:   inline [");
                for (unsigned long i = 0; i < tr->data_size && i < 96; i++) {
                    unsigned char c = data[i];
                    if (c >= 0x20 && c < 0x7f) {
                        emit((char[]){c, 0});
                    } else {
                        emit(".");
                    }
                }
                emit("]\n");
                for (unsigned long i = 0; i < count && i < 12; i++) {
                    unsigned long at = offsets[i];
                    if (at + 40 > tr->data_size) continue;
                    unsigned int type = 0;
                    __builtin_memcpy(&type, data + at, 4);
                    unsigned long long buffer = 0;
                    unsigned long long length = 0;
                    __builtin_memcpy(&buffer, data + at + 8, 8);
                    __builtin_memcpy(&length, data + at + 16, 8);
                    emit("binder-shim:   object ");
                    emit_dec((long)i);
                    emit(" type=0x");
                    emit_hex(type, 8);
                    emit(" buffer=0x");
                    emit_hex((unsigned long)buffer, 16);
                    emit(" length=");
                    emit_dec((long)length);
                    if ((type & 0xffffff) == 0x742a85 && buffer && length && length < 512) {
                        emit(" [");
                        const unsigned char *child = (const unsigned char *)buffer;
                        for (unsigned long k = 0; k < length; k++) {
                            unsigned char c = child[k];
                            if (c >= 0x20 && c < 0x7f) {
                                emit((char[]){c, 0});
                            } else {
                                emit(".");
                            }
                        }
                        emit("]");
                    }
                    emit("\n");
                }
            }
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
        } else if (command == BC_REPLY || command == BC_REPLY_SG) {
            /* The call has been answered: the target's hold goes, which is what the
             * driver does when the transaction completes. The SG variant is
             * hwbinder's way of saying the same thing. */
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
/* Four was one per device name and no room to spare: /dev/binder, /dev/hwbinder,
 * /dev/host_hwbinder and /dev/vndbinder fill it, and a process that opens one of
 * them twice -- which is what `defaultServiceManager1_2(getStub)` does when a
 * HAL lookup falls through to the *host* hwbinder -- gets a placeholder fd that
 * was never remembered. Its ioctls then go to the raw eventfd and come back
 * ENOTTY, and libhidlbase's ProcessState, which asserts on the driver's version,
 * takes the whole process down:
 *
 *   hw-ProcessState: Binder ioctl to obtain version failed: Inappropriate ioctl for device
 *   hw-ProcessState: Binder driver protocol(0) does not match user space protocol(8)!
 *
 * The table is a small array rather than a list for the same reason it was four:
 * a lookup happens on every ioctl. Room for the four names and their repeats is
 * what it needs. */
#define MAX_BINDER_DEVICES 16
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
    } else {
        /* Said out loud: a device the table does not know is a device whose ioctls
         * reach the placeholder fd instead of this shim. */
        emit("binder-shim: the device table is full; a binder fd is unhandled\n");
        flush_log();
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
    static __thread char buffer[4096];
/* Thread-local, not static: the framework opens paths from many threads at once, and
 * one shared buffer means one thread's mapped path can be handed to another thread's
 * syscall -- which is how `CompatConfig` came to list a directory whose entries were
 * the bundle root while the shim's own log said it had opened
 * `<bundle>/etc/compatconfig`, a directory with seven files in it. */

    const char *root = getenv("MOSAIC_ANDROID_ROOT");
    if (!root || !*root || !path || path[0] != '/') return path;

    const char *rest = 0;
    /* A path that begins with `//` is the same path: the framework builds some of
     * them by joining its root with an already absolute component, and every rule
     * below matches on a single leading slash. Left alone, `//system/priv-app` is
     * not a device path as far as these rules are concerned and passes through to a
     * host that has no `/system` -- which reads as "the directory is not there" and
     * gets silently skipped. Collapsing the prefix is what makes those paths
     * answerable. */

    const char *prefix = 0;
    if (strcmp(path, "/system") == 0) {
        /* The bare root itself, which is what `ANDROID_ROOT` holds: ART checks that
         * the directory exists -- `file_utils.cc:153] Failed to find ANDROID_ROOT
         * directory /system` -- and the rule below needs a trailing slash. Exactly
         * the shape of the bare `/data` and `/apex` rules. */
        prefix = "";
        rest = "";
    } else if (strncmp(path, "/system/", 8) == 0) {
        prefix = "/system";
        rest = path + 8;
    } else if (strncmp(path, "/data/", 6) == 0) {
        prefix = "/data";
        /* `data/...`, not `...`: the bundle's own `data` directory stands in for
         * the device's `/data`, exactly as the bundle root stands in for `/system`.
         * Dropping the component sent `/data/system/dropbox/x` to
         * `<bundle>/system/dropbox/x` -- which is why the dropbox writer's temp file
         * and its rename target ended up in a directory the framework then could not
         * find again (`Can't rename /data/system/dropbox/drop13.tmp`), and why
         * `/data/misc/zoneinfo/current/icu` was reported as `//misc/...`. */
        rest = path + 1;
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
    } else if ((strncmp(path, "/framework/", 11) == 0 || strncmp(path, "//framework/", 12) == 0)) {
        /* The ART boot image, which is `<root>/framework/boot.art`: with the root
         * at `/` the framework asks for `//framework/x86_64/boot.art` and gets
         * nothing. Same reason as the `/etc` rule below, same narrowness. */
        prefix = "/framework";
        rest = path + (path[1] == '/' ? 2 : 1);
    } else if ((strncmp(path, "/etc/", 5) == 0 || strncmp(path, "//etc/", 6) == 0) &&
               ((strncmp(path, "/etc/", 5) == 0
                     ? strncmp(path + 5, "public.libraries.txt", 20) == 0 ||
                       strncmp(path + 5, "compatconfig", 12) == 0
                     : strncmp(path + 6, "public.libraries.txt", 20) == 0 ||
                       strncmp(path + 6, "compatconfig", 12) == 0))) {
        /* Two paths the *framework* builds from its root rather than from a
         * constant. `ANDROID_ROOT` is `/system` on a device, so `<root>/etc/...`
         * is `/system/etc/...` there; with this runtime's root at `/` it is the
         * host's `/etc`, which has neither file and must not be read as if it
         * did. Named one by one rather than by mapping all of `/etc`, because the
         * host's `/etc` is a real place that Bionic itself reads -- `getpwuid`
         * and friends -- and the framework's own configuration happens to be the
         * only thing here that wants the bundle's. */
        /* The `rest` is `etc/...` and not `...`, because the shared tail below
         * appends it to the bundle root: `Environment.getRootDirectory()` with the
         * root at `/` builds `//etc/public.libraries.txt`, and this has to land on
         * `<bundle>/etc/public.libraries.txt`. The double slash is what the
         * framework builds and what its own error prints. */
        rest = path + (path[1] == '/' ? 2 : 1);
        prefix = "/etc";
    } else if (strstr(path, "/etc/compatconfig") != 0) {
        /* `CompatConfig.initConfigFromLib` walks `/apex/<module>/etc/compatconfig`
         * for every apex the package manager registered, and calls `listFiles()` on
         * each. A directory that is not there gives a *null* array, and the
         * framework dereferences it: `NullPointerException: Attempt to get length of
         * null array` in `startBootstrapServices`, which is one stage into the boot.
         * The compat configs themselves are one directory -- the bundle's, staged
         * from the image -- so every one of those paths resolves to it. Named rather
         * than by rewriting every apex `etc`, because an apex's own `etc` is a real
         * place that other callers read. */
        /* Keep the *tail*: every file inside those directories has to resolve to
         * the file of the same name, not to the directory -- mapping the whole
         * path to `<bundle>/etc/compatconfig` makes every open of a config file
         * inside it an `EISDIR`, 175 of them in one boot. */
        const char *tail = strstr(path, "/etc/compatconfig");
        rest = tail + 1;
        prefix = "/etc";
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

    /* Bounded, because `strcpy`/`strcat` into a fixed buffer with a path of unknown
     * length is how a shim damages the heap -- and the framework's paths grow long
     * (oat and dex paths are the worst of them). Truncating is a wrong answer, which
     * shows up in a log; overrunning is a wrong answer that hides until an allocator
     * notices, usually somewhere else entirely. */
    {
        const char *parts[2];
        unsigned long n = 0;
        if (strcmp(prefix, "/apex-javalib") == 0) {
            parts[0] = root;
            parts[1] = "/framework/";
        } else if (strcmp(prefix, "/apex-lib64") == 0) {
            parts[0] = root;
            parts[1] = "/lib64/";
        } else {
            /* The bundle root stands in for /system, so those paths lose the prefix
             * rather than gaining it. */
            parts[0] = root;
            parts[1] = "/";
        }
        for (int i = 0; i < 2; i++)
            for (const char *q = parts[i]; *q && n < sizeof(buffer) - 1; q++) buffer[n++] = *q;
        for (const char *q = rest; q && *q && n < sizeof(buffer) - 1; q++) buffer[n++] = *q;
        buffer[n] = 0;
        if (n >= sizeof(buffer) - 1) {
            /* The cap was reached, so the answer is a truncated path -- a different
             * file than the caller asked for. Reported rather than silent, because
             * that is a wrong answer that would otherwise look like a missing file. */
            emit("android-paths: TRUNCATED ");
            emit(path);
            emit("\n");
            flush_log();
        }
    }
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

/* The placeholder for a binder device.
 *
 * It has to be two things at once, and a memfd is only one of them. `mmap` is
 * the obvious requirement -- libbinder maps the driver, and `hwservicemanager`
 * maps it too -- but a memfd cannot be registered with an epoll set:
 * `epoll_ctl` on a regular file returns EPERM, and regular files are what a
 * memfd is. `hwservicemanager` registers its driver fd with a Looper before it
 * does anything else, and that call is fatal:
 *
 *   Looper: Error adding epoll events for fd 3: Operation not permitted
 *   hwservicemanager: Failed to add binder FD to Looper
 *
 * so with a memfd the HIDL service manager cannot start at all, and every HIDL
 * call the framework makes afterwards finds `defaultServiceManager() is null`.
 *
 * What is needed instead is a descriptor that is both pollable and mappable.
 * A `memfd` gives the mapping and an `eventfd` gives the poll, and the two are
 * not the same descriptor -- so the mapping is served from a memfd of its own
 * and the descriptor handed back is the eventfd. `mmap` below is the only path
 * that reaches the mapping, and it substitutes it there. */
static int mapping_fd = -1;

static int make_placeholder_fd(void) {
    int fd = (int)syscall(SYS_eventfd2, 0, 0);
    if (fd < 0) return fd;
    if (mapping_fd < 0) {
        mapping_fd = (int)syscall(SYS_memfd_create, "mosaic-binder-map", 0);
        if (mapping_fd >= 0) syscall(SYS_ftruncate, mapping_fd, 16 * 1024 * 1024);
    }
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

/* How many ashmem regions a process may hold. Android's own limit is
 * `/proc/sys/vm/max_map_count`-ish and effectively unbounded; the framework alone
 * opens more than eight before `ConnectivityService` starts, and the ninth was
 * silently dropped -- its `ASHMEM_SET_SIZE` then failed and libcore reported
 * "ashmem creation failed" far away from the table that was full. The overflow is
 * reported now rather than being silent. */
#define MAX_ASHMEM 1024
static int ashmem_fds[MAX_ASHMEM];
static long ashmem_sizes[MAX_ASHMEM];
static int ashmem_count = 0;

static void remember_ashmem(int fd) {
    if (ashmem_count < MAX_ASHMEM) {
        ashmem_fds[ashmem_count] = fd;
        ashmem_sizes[ashmem_count] = 0;
        ashmem_count++;
    } else {
        emit("android-ashmem: table full at ");
        emit_dec(MAX_ASHMEM);
        emit(" regions\n");
        flush_log();
    }
}

static int ashmem_index(int fd) {
    for (int i = 0; i < ashmem_count; i++) {
        if (ashmem_fds[i] == fd) return i;
    }
    return -1;
}

/* Does `path` end with `tail`? The framework builds some device paths relative to a
 * current directory, so the name is what matters rather than the prefix. */
static int suffix_is(const char *path, const char *tail) {
    unsigned long pl = 0, tl = 0;
    while (path[pl]) pl++;
    while (tail[tl]) tl++;
    if (tl > pl) return 0;
    return strcmp(path + (pl - tl), tail) == 0;
}

static int is_ashmem(int fd) { return ashmem_index(fd) >= 0; }

/* Bionic's <fcntl.h> defines open() as an inline wrapper around __openat, so a
 * preloaded open() is never called. These are the symbols that matter. */
int __openat(int dirfd, const char *path, int flags, int mode) {
    /* The framework hardcodes paths that cannot be configured, so they are
     * rewritten first; then the binder device is intercepted. */
    path = redirect(path);
    /* Every open whose path mentions an app directory, mapped or not: a host path
     * matches no rule and is reported nowhere else, so a scan working in host paths
     * is invisible without this. */
    /* The four device nodes the framework opens by name. The private /dev is a tmpfs
     * that starts empty, and neither route into it works inside a user namespace:
     * `mknod` for a real device is refused, and a node bind-mounted from the host
     * stays owned by the *init* namespace, so opening it here is `EPERM` -- measured,
     * with the boot stopping on `/dev/null: Permission denied` in the shell's own
     * redirects. What is left is to answer the name, which is also what this shim
     * does for `/dev/ashmem` and the binder devices.
     *
     * `null` and `zero` read as zeros; `random` and `urandom` are filled from
     * `getrandom` when the file is made. A memfd is readable and mappable, which is
     * what every caller here wants, and it is what the entropy callers read from:
     * `hwservicemanager: ReadRandomBytes: cannot read /dev/urandom` on every start
     * is what this replaces. */
    /* Anything that *ends* in one of these names, not just the absolute path:
     * `std::random_device` opens `dev/urandom` relative to the current directory,
     * which is why it kept reporting EOF even after these names were answered --
     * the hook matched `/dev/urandom` and never saw the call. */
    if (suffix_is(path, "/dev/null") || suffix_is(path, "/dev/zero") ||
        suffix_is(path, "/dev/random") || suffix_is(path, "/dev/urandom")) {
        int fd = (int)syscall(SYS_memfd_create, "mosaic-dev", 0);
        if (fd < 0) return fd;
        if (suffix_is(path, "/dev/random") || suffix_is(path, "/dev/urandom")) {
            /* Filled once and shared: `libc++`'s `random_device` reads in small blocks
             * and treats EOF as fatal ("random_device got EOF: No data available"),
             * which is what a 256-byte buffer produced. One 64 MiB region, created on
             * first use and handed out with `dup`, outlasts any caller here -- and the
             * offset being shared between readers is harmless for random bytes. */
            /* Per open, and filled generously. Sharing one filled region looked
             * tidier and cost the boot 30 stages: `dup` shares the file offset, so one
             * reader's position is every reader's, and a caller that reads a lot
             * leaves the next one at EOF. A megabyte per open is what keeps
             * `random_device` from seeing the end of the file, which it treats as
             * fatal. */
            unsigned char block[4096];
            long written = 0;
            while (written < 16L * 1024 * 1024) {
                long got = syscall(SYS_getrandom, block, sizeof(block), 0);
                if (got <= 0) break;
                syscall(SYS_write, fd, block, got);
                written += got;
            }
            /* Then the file is extended, not filled: reads past the written part
             * return zeros rather than the end of the file, so no caller can see EOF
             * however much it asks for. Sixteen megabytes of real bytes is more than
             * anything reads in a boot, and the rest costs nothing until touched --
             * a sparse memfd is pages that do not exist yet. */
            syscall(SYS_ftruncate, fd, 1024L * 1024 * 1024);
            syscall(SYS_lseek, fd, 0, 0);
        }
        return fd;
    }
    if (strncmp(path, "/dev/ashmem", 11) == 0) {
        /* A *memfd* of its own, not the placeholder eventfd the binder device gets:
         * ashmem exists to be mapped, and an eventfd cannot be -- `mmap` on one is
         * ENODEV, so the caller's map fails and the failure surfaces as "ashmem
         * creation failed" in a Java stack that never mentions the descriptor. One
         * memfd per region, because the regions must not share memory. The ioctls
         * that follow are answered below, so what the caller ends up with is
         * exactly what ashmem is: a fd it can size, map and share. */
        int fd = (int)syscall(SYS_memfd_create, "mosaic-ashmem", 0);
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

/* Two paths, one buffer. `redirect` returns a single thread-local buffer, so
 * `real(redirect(from), redirect(to))` hands the same string in both arguments --
 * the rename then resolves a name to itself and fails with ENOENT, which is what
 * every dropbox write did: `Can't rename /data/system/dropbox/drop13.tmp to ...
 * /system_server_strictmode@<ts>.txt`. The first mapping is copied out before the
 * second is evaluated, the same discipline `opendir` needed. */
static void redirect_into(const char *path, char *out, int cap) {
    const char *mapped = redirect(path);
    int n = 0;
    for (const char *q = mapped; q && *q && n < cap - 1; q++) out[n++] = *q;
    out[n] = 0;
}

int rename(const char *from, const char *to) {
    typedef int (*real_fn)(const char *, const char *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "rename");
    if (!real) return -1;
    char a[4096], b[4096];
    redirect_into(from, a, sizeof(a));
    redirect_into(to, b, sizeof(b));
    return real(a, b);
}

int renameat(int fromfd, const char *from, int tofd, const char *to) {
    typedef int (*real_fn)(int, const char *, int, const char *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "renameat");
    if (!real) return -1;
    char a[4096], b[4096];
    redirect_into(from, a, sizeof(a));
    redirect_into(to, b, sizeof(b));
    return real(fromfd, a, tofd, b);
}

int renameat2(int fromfd, const char *from, int tofd, const char *to, unsigned int flags) {
    typedef int (*real_fn)(int, const char *, int, const char *, unsigned int);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "renameat2");
    if (!real) return -1;
    char a[4096], b[4096];
    redirect_into(from, a, sizeof(a));
    redirect_into(to, b, sizeof(b));
    return real(fromfd, a, tofd, b, flags);
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
    int result;
    char mapped[4096];
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "statfs");
    if (!real) return -1;
    redirect_into(path, mapped, sizeof(mapped));
    result = real(mapped, buf);
    (void)result;
    return result;
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
/* `realpath`, which the framework reaches through `getCanonicalFile()`:
 * `PackagePartitions$SystemPartition` holds `DeferredCanonicalFile`s, so the scan
 * canonicalises `<root>/priv-app` before it lists it, and an unmapped
 * `/system/priv-app` does not exist on the host at all. Mapping here is what keeps
 * a device path canonicalisable.
 *
 * The mapping is copied out of the thread-local buffer before the call, the way
 * `opendir` does: one call, one string, no second evaluation to race with. */
static char *realpath_impl(const char *path, char *out) {
    typedef char *(*real_fn)(const char *, char *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "realpath");
    if (!real) return 0;
    return real(path, out);
}

/* `realpath`, which the framework reaches through `getCanonicalFile()`. Nothing is
 * mapped *in* -- mapping in cost the host layout fourteen packages, because
 * canonicalising a path the framework built and answering with a bundle path is
 * worse than leaving it alone. Only the other direction, and only one prefix: a
 * canonical answer under the bundle's `apex` comes back as `/apex/...`, which is the
 * spelling `ConnectivityResources` compares a package's `sourceDir` against.
 */
static void apex_reverse(const char *real, char *out, unsigned long cap) {
    const char *root = getenv("MOSAIC_ANDROID_ROOT");
    unsigned long rl = 0;
    const char *rest;
    unsigned long n = 1;
    if (!root || !*root) return;
    while (root[rl]) rl++;
    if (strncmp(real, root, rl) != 0) return;
    rest = real + rl;
    while (rest[0] == '/' && rest[1] == '/') rest++;
    if (strncmp(rest, "apex/", 5) != 0) return;
    out[0] = '/';
    for (const char *q = rest; *q && n < cap - 1; q++) out[n++] = *q;
    out[n] = 0;
}

static char *realpath_common(const char *path, char *resolved, unsigned long cap) {
    char host[4096];
    char *out;
    unsigned long n = 0;
    if (!realpath_impl(path, host)) return 0;
    /* With a NULL `resolved` the caller owns the result and frees it, so it has to be
     * a fresh allocation. Returning a static buffer here -- which is what this did --
     * makes the caller free memory it does not own: ART's
     * `DexFileLoader::GetDexCanonicalLocation` and `DlOpenOatFile::Dlopen` both take
     * that path, and the allocator reports the same chunk freed twice, which is the
     * corruption that stopped the boot at InstallSystemProviders. */
    if (resolved == 0) {
        extern void *malloc(unsigned long);
        out = (char *)malloc(cap < 256 ? 256 : cap);
        if (!out) return 0;
    } else {
        out = resolved;
    }
    while (host[n] && n < cap - 1) { out[n] = host[n]; n++; }
    out[n] = 0;
    {
        /* Temporary: whether the apex reverse fires at all, and for which path. */
        char before[4096];
        unsigned long k = 0;
        while (out[k] && k < sizeof(before) - 1) { before[k] = out[k]; k++; }
        before[k] = 0;
        apex_reverse(host, out, cap);
        if (strcmp(before, out) != 0 && tracing_on()) {
            emit("android-paths: REVERSED ");
            emit(before);
            emit(" -> ");
            emit(out);
            emit("\n");
            flush_log();
        }
    }
    return out;
}

/* The plain symbol and the `_FORTIFY_SOURCE` one both land here; a caller built with
 * the checked call never reaches `realpath` itself. */
char *realpath(const char *path, char *resolved) {
    return realpath_common(path, resolved, 4096);
}

char *__realpath_chk(const char *path, char *resolved, unsigned long len) {
    return realpath_common(path, resolved, len);
}

void *opendir(const char *path) {
    typedef void *(*real_fn)(const char *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "opendir");
    if (!real) return 0;
    /* The mapping is evaluated *once*, into a copy, and the copy is what the syscall
     * gets. `redirect` returns the thread-local buffer, and calling it again for the
     * log used to hand `real` a pointer whose contents a later call would rewrite. */
    const char *mapped = redirect(path);
    char mapped_copy[4096];
    int mc = 0;
    for (const char *q = mapped; q && *q && mc < 4000; q++) mapped_copy[mc++] = *q;
    mapped_copy[mc] = 0;
    void *result = real(mapped_copy);
    /* Every directory open, mapped or not. A path that no rule matches is not
     * reported anywhere else, so a scan that works entirely in host paths would
     * otherwise be invisible -- and absence of evidence would read as absence of the
     * call. */
    if (tracing_on()) {
        emit("android-paths: OPDIR ");
        emit(path);
        emit(" -> ");
        emit(mapped_copy);
        emit(result ? " ok\n" : " failed\n");
        flush_log();
    }
    /* A directory that will not open is reported whether or not tracing is on. A
     * reader that gets null here reports it far away and in other words -- the
     * framework's `CompatConfig` fails with a null list, not with this path -- and
     * the one thing that names it is this line. Rewrites are only listed when
     * tracing is on (see `report`); failures always are. */
    if (!result) {
        emit("android-paths: cannot open directory ");
        emit(path);
        emit(" (as ");
        emit(redirect(path));
        emit(")\n");
        flush_log();
    }
    return result;
}

/* A binder device is answered as present.
 *
 * This is where libhidlbase decides whether to build a HIDL service manager at
 * all, before any open: `defaultServiceManager1_2` starts with
 *
 *     access("/dev/hwbinder", R_OK | W_OK)
 *
 * and returns null when it fails. On a device the node is there; in this
 * namespace `/dev` is a tmpfs the harness makes and no node was made in it, so
 * the call failed and every HIDL registration the framework attempted was
 * refused -- `Cannot register ... : -38` -- with no transaction ever reaching the
 * driver. The open itself is redirected below, so answering "yes, it is there"
 * is the truth about this side. */
int access(const char *path, int mode) {
    typedef int (*real_fn)(const char *, int);
    static real_fn real;
    if (name_is_binder(path)) return 0;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "access");
    return real ? real(redirect(path), mode) : -1;
}

int faccessat(int dirfd, const char *path, int mode, int flags) {
    typedef int (*real_fn)(int, const char *, int, int);
    static real_fn real;
    if (name_is_binder(path)) return 0;
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
        /* A binder device's placeholder is an eventfd, which cannot be mapped,
         * and the mapping is what libbinder uses for the driver's command and
         * data buffers. Every ioctl that would read or write them is answered
         * in `ioctl` below, so what the caller needs from the mapping is that
         * it is addressable and big enough; the memfd kept for that purpose
         * alone is what backs it. */
        return (void *)syscall(SYS_mmap, addr, length, prot, flags, mapping_fd, offset);
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
            struct binder_device *device = device_for(fd);
            transaction_on_hwbinder = device && device->is_hwbinder;
            run_commands((unsigned char *)bwr->write_buffer, (unsigned long)bwr->write_size);
            transaction_on_hwbinder = 0;
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

/* A bare library name, which is how `System.loadLibrary` asks the linker for
 * anything: `System.loadLibrary("service-connectivity")` reaches
 * `android_dlopen_ext` as a name with no path at all, and the linker searches
 * only the namespaces it was configured with. ART builds a namespace per class
 * loader with `librarySearchPath = null` -- `SystemServerClassLoaderFactory`
 * passes exactly that, and the apex jar's libraries are found through its parent
 * on a device, where the linker configuration is generated by the build and by
 * init rather than here.
 *
 * So a bare name is resolved here first, against the directories this bundle
 * keeps its libraries in, and the linker is handed a path that exists. That is
 * the path layer's job (ADR-0009), and nothing is invented: a name that is in
 * none of these directories is passed through unchanged, so the linker's own
 * error is still the one that surfaces.
 *
 * The directories are the bundle's own and every apex's -- `<root>/lib64` and
 * `<root>/apex/<module>/lib64` -- which is where an Android image keeps them. */
#define MOSAIC_SYS_getdents64 217

struct mosaic_dirent64 {
    unsigned long long inode;
    long long offset;
    unsigned short record_length;
    unsigned char type;
    char name[];
};

/* `<root><relative><name>`, written into `out` and opened to see it is there. */
static int library_at(const char *root, const char *relative, const char *name, char *out,
                      unsigned long capacity) {
    unsigned long n = 0;
    for (const char *r = root; *r && n < capacity - 1; r++) out[n++] = *r;
    for (const char *r = relative; *r && n < capacity - 1; r++) out[n++] = *r;
    for (const char *r = name; *r && n < capacity - 1; r++) out[n++] = *r;
    out[n] = 0;
    int fd = (int)syscall(SYS_openat, AT_FDCWD, out, O_RDONLY, 0);
    if (fd < 0) return 0;
    syscall(SYS_close, fd);
    return 1;
}

/* The whole path of a library this bundle carries, or null. */
static const char *library_path(const char *root, const char *name, char *out, unsigned long capacity) {
    if (library_at(root, "/lib64/", name, out, capacity)) return out;

    char apexes[4096];
    unsigned long n = 0;
    for (const char *r = root; *r && n < sizeof(apexes) - 8; r++) apexes[n++] = *r;
    for (const char *r = "/apex"; *r && n < sizeof(apexes) - 1; r++) apexes[n++] = *r;
    apexes[n] = 0;

    int fd = (int)syscall(SYS_openat, AT_FDCWD, apexes, 0x10000 /* O_DIRECTORY */, 0);
    if (fd < 0) return 0;
    static char listing[8192];
    long got = syscall(MOSAIC_SYS_getdents64, fd, listing, sizeof(listing));
    syscall(SYS_close, fd);
    if (got <= 0) return 0;

    const char *const dirs[] = { "/lib64/", "/lib/" };
    for (long at = 0; at < got;) {
        struct mosaic_dirent64 *entry = (struct mosaic_dirent64 *)(listing + at);
        if (entry->record_length == 0) break;
        for (unsigned long d = 0; d < 2; d++) {
            char relative[4400];
            unsigned long r = 0;
            for (const char *s = "/apex/"; *s && r < sizeof(relative) - 1; s++) relative[r++] = *s;
            for (const char *s = entry->name; *s && r < sizeof(relative) - 300; s++) {
                relative[r++] = *s;
            }
            for (const char *s = dirs[d]; *s && r < sizeof(relative) - 1; s++) relative[r++] = *s;
            relative[r] = 0;
            if (library_at(root, relative, name, out, capacity)) return out;
        }
        at += entry->record_length;
    }
    return 0;
}

/* `android_dlextinfo`, whose layout is the linker's published ABI
 * (`android/dlext.h`): the namespace a load is made in is its last field. It is
 * spelled out rather than included because this shim is built without headers. */
typedef struct {
    unsigned long long flags;
    void *reserved_addr;
    unsigned long reserved_size;
    int relro_fd;
    int library_fd;
    long long library_fd_offset;
    void *library_namespace;
} mosaic_dlextinfo_t;

/* The namespace a bare name has to be loaded in. ART gives each class loader a
 * namespace whose permitted paths are the class path's own directories --
 * `SystemServerClassLoaderFactory` passes `librarySearchPath = null` -- so a path
 * into an apex's lib64 is refused there with `not permitted`. The *default*
 * namespace is the one this bundle's linker configuration describes, apex
 * directories and all, and it is exported (`namespace.default.visible = true`) for
 * exactly this kind of use. */
/* Weak: the symbol lives in the Android linker, and whether this build's linker
 * has it is what the call site is asking. Without the attribute the address of a
 * function is always non-null, so the check is a constant and the build says so. */
extern void *android_get_exported_namespace(const char *name) __attribute__((weak));

void *android_dlopen_ext(const char *name, int flags, const void *info) {
    typedef void *(*real_fn)(const char *, int, const void *);
    static real_fn real;
    static int announced;
    char resolved[4600];
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "android_dlopen_ext");
    if (!real) return 0;
    if (name && name[0] != '/') {
        const char *root = getenv("MOSAIC_ANDROID_ROOT");
        if (root && *root && library_path(root, name, resolved, sizeof(resolved))) {
            mosaic_dlextinfo_t adjusted;
            const void *use = info;
            void *namespace = 0;
            if (android_get_exported_namespace) {
                /* The exported name is the section's -- the linker creates the
                 * namespace from `[system]` and calls it `system`; `default` is the
                 * name it is *asked* about in some places. Both are tried, and the
                 * one that answered is logged. */
                namespace = android_get_exported_namespace("system");
                if (!namespace) namespace = android_get_exported_namespace("default");
            }
            if (info && namespace) {
                __builtin_memcpy(&adjusted, info, sizeof(adjusted));
                adjusted.library_namespace = namespace;
                use = &adjusted;
            }
            if (tracing_on() && !announced) {
                announced = 1;
                emit("android-paths: bare library ");
                emit(name);
                emit(" -> ");
                emit(resolved);
                emit(namespace ? " in the exported namespace\n" : " with no namespace\n");
                flush_log();
            }
            return real(resolved, flags, use);
        }
    }
    return real(name, flags, info);
}
