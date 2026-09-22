/* Binder at the level libbinder exports, and at the level the driver speaks.
 *
 * Two doors reach the service manager, and the framework uses both:
 *
 *   libbinder's API   BpBinder::transact / IPCThreadState::transact, which a
 *                     preload can interpose when the caller is outside
 *                     libbinder. libbinder_ndk's AServiceManager_* pair is
 *                     plain C exported for other libraries, so it is reachable
 *                     too. Together these cover the AIDL path.
 *
 *   the driver        ioctl(BINDER_WRITE_READ), which is all the *Java* path can
 *                     reach: BinderProxy.transact -> IBinder::transact ->
 *                     BpBinder::transact -> IPCThreadState::transact all resolve
 *                     inside libbinder, where -fno-semantic-interposition binds
 *                     them locally. probe.so answers that door and calls the
 *                     dispatch below.
 *
 * Both doors must see one registry, or a service registered through one is
 * missing through the other. That is why the registry here is exported rather
 * than static: probe.so is a separate preload, and the Android linker puts every
 * LD_PRELOAD library in the global group, so its undefined symbols resolve here.
 *
 * Reading arguments means reading the request payload. Its bytes are the same
 * whether they arrived through a Parcel or through the driver's transaction
 * buffer -- an AIDL payload is [interface token][name] as string16s followed by
 * the arguments -- so the parsing below takes (data, size) and works for both.
 */

typedef unsigned int uint32;
typedef unsigned long ulong;

/* The allocation tracer, if it is loaded: a marker in its log so a hand-back and
 * the allocations around it share one sequence. Weak, so the shim works without
 * it. */
extern void mosaic_alloc_trace_mark(const char *what, const void *pointer)
    __attribute__((weak));

extern void *dlsym(void *, const char *);
extern void *dlopen(const char *, int);
extern long write(int, const void *, unsigned long);
extern void *malloc(ulong);
extern void free(void *);

#define RTLD_NOW 2

#define SYM_WRITE_INT32 "_ZN7android6Parcel10writeInt32Ei"
#define SYM_WRITE_BINDER "_ZN7android6Parcel17writeStrongBinderERKNS_2spINS_7IBinderEEE"
#define SYM_WRITE_BYTES "_ZN7android6Parcel5writeEPKvm"
#define SYM_DATA "_ZNK7android6Parcel4dataEv"
#define SYM_DATA_SIZE "_ZNK7android6Parcel8dataSizeEv"
#define SYM_PARCEL_CTOR "_ZN7android6ParcelC1Ev"
#define SYM_PARCEL_DTOR "_ZN7android6ParcelD1Ev"
#define SYM_IPC_DATA "_ZNK7android6Parcel7ipcDataEv"
#define SYM_IPC_DATA_SIZE "_ZNK7android6Parcel11ipcDataSizeEv"
#define SYM_IPC_OBJECTS "_ZNK7android6Parcel10ipcObjectsEv"
#define SYM_IPC_OBJECTS_COUNT "_ZNK7android6Parcel15ipcObjectsCountEv"
#define SYM_SET_POSITION "_ZNK7android6Parcel15setDataPositionEm"
#define SYM_GET_POSITION "_ZNK7android6Parcel12dataPositionEv"
#define SYM_READ_BINDER "_ZNK7android6Parcel16readStrongBinderEv"
#define SYM_WRITE_OBJECT "_ZN7android6Parcel11writeObjectERK18flat_binder_objectb"
#define SYM_SET_REFERENCE "_ZN7android6Parcel19ipcSetDataReferenceEPKhmPKymPFvPS0_S2_mS4_mE"
#define SYM_BINDER_TRANSACT "_ZN7android7BBinder8transactEjRKNS_6ParcelEPS1_j"
extern int pthread_create(unsigned long *, const void *, void *(*)(void *), void *);

/* A Parcel is a few hundred bytes and its layout is libbinder's business, so it
 * is allocated with room to spare and initialised by its own constructor. */
#define PARCEL_BYTES 1024

/* android.os.IServiceManager's methods, in declaration order, which is what an
 * AIDL transaction code is (frameworks/native/libs/binder/aidl/android/os/
 * IServiceManager.aidl at android-13.0.0_r1):
 *
 *   1 getService          6 unregisterForNotifications  11 registerClientCallback
 *   2 checkService        7 isDeclared                  12 tryUnregisterService
 *   3 addService          8 getDeclaredInstances        13 getServiceDebugInfo
 *   4 listServices        9 updatableViaApex
 *   5 registerForNotifications  10 getConnectionInfo
 *
 * `isDeclared` was 5 here, which is `registerForNotifications`: the framework's
 * `isDeclared` fell through to the catch-all, was answered with an exception code
 * and nothing else, and the reader asked for an int32 that was not there --
 * `Failed to get isDeclard for android.hardware.power.IPower/default:
 * Status(-129, EX_TRANSACTION_FAILED): 'NOT_ENOUGH_DATA: '`. Code 4 =
 * listServices and 7 = isDeclared were confirmed against a live boot. */
#define TRANSACTION_GET_SERVICE 1
#define TRANSACTION_CHECK_SERVICE 2
#define TRANSACTION_ADD_SERVICE 3
#define TRANSACTION_LIST_SERVICES 4
#define TRANSACTION_REGISTER_FOR_NOTIFICATIONS 5
#define TRANSACTION_UNREGISTER_FOR_NOTIFICATIONS 6
#define TRANSACTION_IS_DECLARED 7
#define TRANSACTION_GET_DECLARED_INSTANCES 8
#define TRANSACTION_UPDATABLE_VIA_APEX 9
#define TRANSACTION_GET_CONNECTION_INFO 10
#define TRANSACTION_REGISTER_CLIENT_CALLBACK 11
#define TRANSACTION_TRY_UNREGISTER_SERVICE 12
#define TRANSACTION_GET_SERVICE_DEBUG_INFO 13

/* The object types libbinder actually writes, read out of Parcel::unflattenBinder:
 *
 *   0x73682a85  BINDER_TYPE_HANDLE -- a remote object
 *   0x73622a85  BINDER_TYPE_BINDER -- a local one
 *
 * The 0x73 this used to carry is the *old* ASCII encoding and matches nothing,
 * so every addService was read as "not a local binder" and remembered nothing --
 * which is why the framework's own services were never found. */
#define BINDER_TYPE_BINDER 0x73622a85u
#define BINDER_TYPE_WEAK_BINDER 0x73622a86u
#define BINDER_TYPE_FD 0x73662a85u
#define BINDER_TYPE_HANDLE 0x73682a85u

#define MAX_SERVICES 128
#define NAME_MAX 128

/* Servers register from one thread while clients look up from another. */
static int registry_lock = 0;

static void lock_registry(void) {
    while (__sync_lock_test_and_set(&registry_lock, 1)) {
        /* spin; the critical sections are a name comparison */
    }
}

static void unlock_registry(void) {
    __sync_lock_release(&registry_lock);
}

typedef struct {
    char name[NAME_MAX];
    void *object;
    unsigned long cookie;
    /* The node this service was published under, which is what a transaction
     * arriving from another process names. */
    unsigned long node;
} service_t;

static service_t services[MAX_SERVICES];
static int service_count = 0;

/* Logging, bounded: this library sits on the busiest path in the process, and a
 * run that logs every call fills a disk. */
#define MAX_LINES 4000
static int lines = 0;

static ulong length(const char *s) {
    ulong n = 0;
    if (!s) return 0;
    while (s[n]) n++;
    return n;
}

/* Messages are built up and written once, at the newline. Writing each piece
 * separately interleaves with the process's own stderr -- two libraries writing
 * to fd 2 a fragment at a time -- and produces output that reads like a
 * corrupted buffer. */
static char line[2048];
static ulong line_length = 0;

static void flush_line(void) {
    if (line_length) {
        write(2, line, line_length);
        line_length = 0;
    }
}

static void say(const char *s) {
    if (!s) return;
    for (const char *p = s; *p; p++) {
        if (*p == '\n') {
            if (line_length + 1 < sizeof(line)) line[line_length++] = '\n';
            flush_line();
            continue;
        }
        if (line_length + 1 < sizeof(line)) line[line_length++] = *p;
    }
}

static void say_dec(long value) {
    char out[24];
    int i = 0;
    ulong magnitude;
    if (value < 0) {
        out[i++] = '-';
        magnitude = (ulong)(-value);
    } else {
        magnitude = (ulong)value;
    }
    char tmp[20];
    int n = 0;
    do {
        tmp[n++] = '0' + (magnitude % 10);
        magnitude /= 10;
    } while (magnitude);
    while (n) out[i++] = tmp[--n];
    out[i] = 0;
    say(out);
}

/* Emit one line, or nothing once the budget is spent. The caller writes the
 * whole line and then calls this once, so a line is never torn. */
static void say_once(void) {
    if (lines >= MAX_LINES) {
        if (lines == MAX_LINES) {
            lines++;
            static const char notice[] =
                "android-binder: (further binder logs suppressed)\n";
            write(2, notice, sizeof(notice) - 1);
        }
        return;
    }
    lines++;
}

typedef int (*write_int32_fn)(void *, int);
typedef int (*write_binder_fn)(void *, const void *);
typedef int (*write_bytes_fn)(void *, const void *, ulong);
typedef const unsigned char *(*parcel_data_fn)(const void *);
typedef ulong (*parcel_size_fn)(const void *);
typedef void (*parcel_ctor_fn)(void *);
typedef void (*parcel_dtor_fn)(void *);
typedef const unsigned long *(*parcel_objects_fn)(const void *);
typedef ulong (*parcel_count_fn)(const void *);
/* setDataPosition returns void, not status_t: reading its return as an int and
 * treating a non-zero value as failure is what kept readStrongBinder from ever
 * being reached -- the reader looked like it "returns nothing for the Java
 * requests", and the object was taken by pointer archaeology instead, with no
 * reference held. */
typedef void (*parcel_position_fn)(const void *, ulong);
typedef ulong (*parcel_where_fn)(const void *);
/* readStrongBinder returns an sp<IBinder> by value. sp has a user-declared
 * destructor, so the Itanium ABI returns it through a hidden first pointer --
 * calling it as if it returned a pointer in rax is what made the first attempt
 * at this crash. */
typedef void (*parcel_read_fn)(void *out, const void *self);
typedef int (*write_object_fn)(void *self, const void *obj, int null_meta_data);
typedef void (*parcel_set_reference_fn)(void *self, const unsigned char *data, ulong size,
                                        const ulong *objects, ulong count, void *release);
typedef int (*binder_transact_fn)(void *self, uint32 code, const void *data, void *reply,
                                  uint32 flags);

static write_int32_fn parcel_write_int32;
static write_binder_fn parcel_write_binder;
static write_object_fn parcel_write_object;
static write_bytes_fn parcel_write_bytes;
static parcel_data_fn parcel_data;
static parcel_size_fn parcel_data_size;
static parcel_ctor_fn parcel_ctor;
static parcel_dtor_fn parcel_dtor;
static parcel_data_fn parcel_ipc_data;
static parcel_size_fn parcel_ipc_data_size;
static parcel_objects_fn parcel_ipc_objects;
static parcel_count_fn parcel_ipc_objects_count;
static parcel_position_fn parcel_set_position;
static parcel_where_fn parcel_data_position;
static parcel_read_fn parcel_read_strong;
static parcel_set_reference_fn parcel_set_reference;
static binder_transact_fn binder_transact;

static void resolve(void) {
    if (parcel_write_int32) return;
    void *binder = dlopen("libbinder.so", RTLD_NOW);
    if (!binder) {
        say("android-binder: cannot open libbinder.so\n");
        return;
    }
    parcel_write_int32 = (write_int32_fn)dlsym(binder, SYM_WRITE_INT32);
    parcel_write_bytes = (write_bytes_fn)dlsym(binder, SYM_WRITE_BYTES);
    parcel_write_binder = (write_binder_fn)dlsym(binder, SYM_WRITE_BINDER);
    parcel_write_object = (write_object_fn)dlsym(binder, SYM_WRITE_OBJECT);
    parcel_data = (parcel_data_fn)dlsym(binder, SYM_DATA);
    parcel_data_size = (parcel_size_fn)dlsym(binder, SYM_DATA_SIZE);
    parcel_ctor = (parcel_ctor_fn)dlsym(binder, SYM_PARCEL_CTOR);
    parcel_dtor = (parcel_dtor_fn)dlsym(binder, SYM_PARCEL_DTOR);
    parcel_ipc_data = (parcel_data_fn)dlsym(binder, SYM_IPC_DATA);
    parcel_ipc_data_size = (parcel_size_fn)dlsym(binder, SYM_IPC_DATA_SIZE);
    parcel_ipc_objects = (parcel_objects_fn)dlsym(binder, SYM_IPC_OBJECTS);
    parcel_ipc_objects_count = (parcel_count_fn)dlsym(binder, SYM_IPC_OBJECTS_COUNT);
    parcel_set_position = (parcel_position_fn)dlsym(binder, SYM_SET_POSITION);
    parcel_data_position = (parcel_where_fn)dlsym(binder, SYM_GET_POSITION);
    parcel_read_strong = (parcel_read_fn)dlsym(binder, SYM_READ_BINDER);
    parcel_set_reference = (parcel_set_reference_fn)dlsym(binder, SYM_SET_REFERENCE);
    binder_transact = (binder_transact_fn)dlsym(binder, SYM_BINDER_TRANSACT);
}

/* A flat_binder_object is 24 bytes: type at 0, flags at 4, the object or the
 * handle at 8, and the cookie at 16 -- and libbinder writes an *int32 stability
 * word after it*, so a binder on the wire is 28 bytes. A writer for each of the
 * three shapes the registry hands back, because the shape is part of the answer:
 *
 *   local binder  the object is this process's, so the caller talks to it here
 *   handle        the object is the broker's or another process's, so the caller
 *                 gets a number in this process's table and the shim forwards
 *                 every transaction on it
 *   null          there is no such object
 *
 * The stability word is not decoration. `Parcel::flattenBinder` ends in
 * `finishFlattenBinder`, which writes it, and `unflattenBinder` ends in
 * `finishUnflattenBinder`, which *reads* it and fails the whole read if it is not
 * there:
 *
 *     status_t status = readInt32(&stability);
 *     if (status != OK) return status;
 *
 * so an object written as 24 bytes is an object the reader cannot read. The value
 * is a `Stability::Level` (frameworks/native/libs/binder/Stability.cpp):
 *
 *   a null binder is written as UNDECLARED (0) -- `setRepr` accepts nothing else
 *   for one -- and anything else has to be a *declared* level, which for a handle
 *   the reader turns into a proxy is SYSTEM (0b001100 = 12), the level a system
 *   process's own binders carry.
 *
 * The null case also used to be written as *nothing at all*, and that is not what
 * a null binder is: Parcel::readObject finds no object word, Parcel::unflattenBinder
 * returns BAD_TYPE, and the framework reports a failed transaction where it
 * should see an absent service --
 *
 *   ServiceManager: Failed to getService in waitForService for suspend_control:
 *     Status(-129, EX_TRANSACTION_FAILED): 'BAD_TYPE: '
 *
 * is that, and the same shape is `NOT_ENOUGH_DATA` on the AIDL door.
 * libbinder's own Parcel::flattenBinder writes a null as BINDER_TYPE_BINDER with
 * both pointers zero, so that is what is written here. It reaches readObject's
 * "transferring a NULL object" path, which is why a null written this way is read
 * as one.
 *
 * Parcel::writeObject is the writer rather than raw int32s so the object lands in
 * the parcel's own object table: a *handle* object has a non-zero pointer field,
 * so the reader checks that table, and an object missing from it is refused. */

#define STABILITY_UNDECLARED 0
#define STABILITY_SYSTEM 12 /* 0b001100, Stability::Level::SYSTEM */
extern char *strstr(const char *, const char *);
#define STABILITY_VINTF 63  /* 0b111111, Stability::Level::VINTF */

/* Where a BpBinder keeps the handle it stands for. Read out of
 * BpBinder::transact's disassembly, which loads it there to pass to
 * IPCThreadState::transact. */
#define BINDER_HANDLE_OFFSET 0x10

typedef struct {
    uint32 type;
    uint32 flags;
    ulong binder;
    ulong cookie;
} flat_object_t;

static void write_object_into(void *reply, uint32 type, ulong value, ulong cookie, int stability) {
    flat_object_t object;
    object.type = type;
    object.flags = 0;
    object.binder = value;
    object.cookie = cookie;
    if (parcel_write_object) {
        parcel_write_object(reply, &object, 0);
        parcel_write_int32(reply, stability);
        return;
    }
    /* Without libbinder's writer, raw words: 24 bytes then the stability word.
     * The object table is then empty, which the reader accepts for the null case
     * and not for a handle. */
    parcel_write_int32(reply, (int)type);
    parcel_write_int32(reply, 0);
    parcel_write_int32(reply, (int)(value & 0xffffffffu));
    parcel_write_int32(reply, (int)(value >> 32));
    parcel_write_int32(reply, (int)(cookie & 0xffffffffu));
    parcel_write_int32(reply, (int)(cookie >> 32));
    parcel_write_int32(reply, stability);
}

/* A service another process owns: the caller gets a handle in its own table.
 *
 * The stability word is the level the *reader* will mark its proxy with, and
 * `Stability::setRepr` accepts any declared level for a fresh proxy, so this is
 * about telling the truth rather than passing a check: a VINTF-stable HAL is
 * VINTF, and a system process's own service is SYSTEM. The name is what this side
 * has; the broker's table is where a per-service answer belongs. */
static void write_handle_into(void *reply, uint32 handle, const char *name) {
    int stability = STABILITY_SYSTEM;
    if (name && strstr(name, "ISystemSuspend")) stability = STABILITY_VINTF;
    write_object_into(reply, BINDER_TYPE_HANDLE, (ulong)handle, 0, stability);
}

/* No such object. */
static void write_null_into(void *reply) {
    write_object_into(reply, BINDER_TYPE_BINDER, 0, 0, STABILITY_UNDECLARED);
}

/* A string16, as Parcel::writeString16 writes one: the unit count, the UTF-16
 * units, a NUL, and padding to four bytes. ASCII in, because everything this
 * answers with -- interface tokens -- is. */
static void write_string16_into(void *reply, const char *text) {
    ulong len = length(text);
    parcel_write_int32(reply, (int)len);
    for (ulong i = 0; i < len; i++) {
        unsigned char pair[2];
        pair[0] = (unsigned char)text[i];
        pair[1] = 0;
        parcel_write_bytes(reply, pair, 2);
    }
    /* The terminator and the padding to a four byte boundary: two bytes when the
     * count is odd, four when it is even. */
    unsigned char zeroes[4] = {0, 0, 0, 0};
    parcel_write_bytes(reply, zeroes, (len % 2) ? 2 : 4);
}

/* The two transactions every binder object answers (IBinder.h, B_PACK_CHARS):
 * a ping, whose answer is an empty reply, and an interface query, whose answer is
 * the interface token. */
#define TRANSACTION_PING 0x5f504e47u      /* B_PACK_CHARS('_','P','N','G') */
#define TRANSACTION_INTERFACE 0x5f4e5446u /* B_PACK_CHARS('_','N','T','F') */

static uint32 u32_at(const unsigned char *p) {
    uint32 v;
    __builtin_memcpy(&v, p, 4);
    return v;
}

/* A Parcel string16 is a character count and its UTF-16 data, then padding to a
 * four byte boundary -- two bytes when the count is odd, because four plus an
 * even number of characters leaves it two short.
 *
 * Getting this wrong cost three attempts. Four bytes of padding put the object
 * that follows a service name past its first field, so the type read out of an
 * addService was the object's *flags* instead of BINDER_TYPE_BINDER and every
 * registration was dropped as "not a local binder"; no padding read it one field
 * early. The counts here are direct: a fifteen character name puts
 * BINDER_TYPE_BINDER (0x73622a85) at offset 4 + 30 + 2. */
static const unsigned char *skip_string16(const unsigned char *p, const unsigned char *end) {
    if (!p || p + 4 > end) return 0;
    uint32 chars = u32_at(p);
    const unsigned char *q = p + 4 + (ulong)chars * 2 + ((chars & 1) ? 2 : 0);
    return q <= end ? q : 0;
}

static int read_string16(const unsigned char *p, const unsigned char *end, char *out, int cap) {
    if (!p || p + 4 > end) return 0;
    uint32 chars = u32_at(p);
    if (p + 4 + (ulong)chars * 2 > end) return 0;
    int n = 0;
    for (uint32 i = 0; i < chars && n < cap - 1; i++) {
        unsigned short c;
        __builtin_memcpy(&c, p + 4 + (ulong)i * 2, 2);
        out[n++] = (c < 128) ? (char)c : '?';
    }
    out[n] = 0;
    return 1;
}

/* Where the arguments start.
 *
 * The interface token is a string16, but it is not always the first thing in the
 * request: this build prefixes it with twelve bytes -- a strict mode policy and
 * two more words -- so skipping one string16 from the start lands in the prefix
 * and the name comes out unreadable. Searching for the descriptor is
 * independent of whatever precedes it, and it also says which interface is being
 * called, which matters: every service's transaction codes start at 1, so
 * answering by code alone would answer someone else's call.
 *
 * Returns the position just after the token, or 0 if this is not that interface.
 */
static const unsigned char *after_token(const unsigned char *data, ulong size, const char *token) {
    if (!data) return 0;
    const unsigned char *end = data + size;
    ulong len = length(token);
    for (const unsigned char *p = data; p + 4 <= end; p += 4) {
        if (u32_at(p) != len) continue;
        if (p + 4 + len * 2 > end) continue;
        int same = 1;
        for (ulong i = 0; i < len; i++) {
            unsigned short c;
            __builtin_memcpy(&c, p + 4 + i * 2, 2);
            if ((char)c != token[i]) {
                same = 0;
                break;
            }
        }
        if (same) return skip_string16(p, end);
    }
    return 0;
}

#define SERVICE_MANAGER_TOKEN "android.os.IServiceManager"

/* The name is the first readable string16 after the token.
 *
 * It is not immediately after it: this build puts an int32 -- a hash of the
 * descriptor, by the look of it -- in between, which reads as an empty string.
 * Scanning for the first non-empty, printable one finds the name instead of the
 * gap in front of it. Names are ASCII, so requiring that keeps the scan from
 * mistaking a length or a pointer for one.
 *
 * Returns the position just after the name, so the caller can find what follows
 * it, or 0 if there is no name (listServices has none).
 */
static const unsigned char *name_after_token(const unsigned char *data, ulong size, char *out,
                                             int cap) {
    const unsigned char *end = data + size;
    const unsigned char *p = after_token(data, size, SERVICE_MANAGER_TOKEN);
    if (!p) return 0;
    while (p + 4 <= end) {
        uint32 chars = u32_at(p);
        if (chars > 0 && chars <= 128 && p + 4 + (ulong)chars * 2 <= end) {
            int printable = 1;
            for (uint32 i = 0; i < chars; i++) {
                unsigned short c;
                __builtin_memcpy(&c, p + 4 + (ulong)i * 2, 2);
                if (c < 32 || c > 126) {
                    printable = 0;
                    break;
                }
            }
            if (printable && read_string16(p, end, out, cap)) {
                return skip_string16(p, end);
            }
        }
        p += 4;
    }
    return 0;
}

/* ---- the broker's socket --------------------------------------------------
 *
 * The broker owns the name space that more than one process can see, so a
 * service registered here is published to it, a name this process does not have
 * is asked of it, and a transaction for an object in another process is sent to
 * it. It also sends transactions the other way: the reader thread below serves
 * them out of the local objects.
 *
 * The wire format is the broker's, and is fixed size and big-endian so this
 * side can write it too:
 *
 *   0  u8   kind        4  u32 a      16 u64 node
 *   1  u8   version     8  u32 b      24 u32 data length
 *   2  u16  reserved    12 u32 c      28 u32 descriptor count
 *
 * A connection begins with the magic MSBD, which as a control-plane length
 * prefix would be refused long before MAX_FRAME.
 */

#define MOSAIC_WIRE_MAGIC "MSBD"
#define MOSAIC_WIRE_VERSION 1

#define KIND_TRANSACTION 0
#define KIND_REPLY 1
#define KIND_ACQUIRE 2
#define KIND_RELEASE 3
#define KIND_INCREFS 4
#define KIND_DECREFS 5
#define KIND_DEAD 6
#define KIND_INCOMING 7
#define KIND_BYE 8
#define KIND_EXPORT 9
#define KIND_LOOKUP 10
#define KIND_FOUND 11
#define KIND_INCOMING_REPLY 12

#define NO_HANDLE 0xffffffffu

#define MAX_BROKER_FRAME (1024 * 1024)
#define MAX_BROKER_FDS 8
#define CMSG_SPACE_BYTES 64
#define SOL_SOCKET 1
#define SCM_RIGHTS 1

#define SYS_READ 0
#define SYS_CLOSE 3
#define SYS_SOCKET 41
#define SYS_CONNECT 42
#define SYS_NANOSLEEP 35
#define SYS_GETPID 39
#define SYS_RECVMSG 47

/* The three structs `recvmsg` needs, and the control-message macros that go with
 * them. This file has no headers -- it declares what it uses and reaches the
 * kernel through syscall() -- so these are the kernel's own layouts. */
struct mosaic_iovec {
    void *base;
    ulong len;
};

struct mosaic_msghdr {
    void *name;
    uint32 name_len;
    struct mosaic_iovec *iov;
    ulong iov_len;
    void *control;
    ulong control_len;
    int flags;
};

struct mosaic_cmsghdr {
    ulong len;
    int level;
    int type;
};


extern long syscall(long, ...);
extern char *getenv(const char *);
extern int strcmp(const char *, const char *);
extern char *strcpy(char *, const char *);
extern char *strcat(char *, const char *);
extern void *malloc(ulong);
extern void free(void *);

static int broker_fd = -1;
static char broker_path[256];
static int broker_lock = 0;
static int broker_write_lock = 0;
static int broker_wanted = -1;

/* Talking to the broker is on by default, because the broker is part of the
 * product: it owns the names more than one process can see. MOSAIC_BINDER_BROKER=0
 * turns it off, for a harness that has no broker to talk to.
 *
 * The environment is read here, on the first use, and not at load: a constructor
 * runs before Bionic's environment is readable, which is how an earlier version
 * of this concluded the client was disabled when it was not. */
static int broker_enabled(void) {
    if (broker_wanted < 0) {
        const char *v = getenv("MOSAIC_BINDER_BROKER");
        broker_wanted = (v && v[0] == '0') ? 0 : 1;
    }
    return broker_wanted;
}

static void lock_broker(void) {
    int spins = 0;
    while (__sync_lock_test_and_set(&broker_lock, 1)) {
        if (spins++ == 0) {
            say("android-binder: waiting for the broker lock\n");
            say_once();
        }
    }
}

static void unlock_broker(void) {
    __sync_lock_release(&broker_lock);
}

/* Frames are written under their own lock: the serving thread answers on this
 * socket while a caller may be waiting for its own answer, and those two must
 * not be serialised against each other. */
static void lock_writes(void) {
    while (__sync_lock_test_and_set(&broker_write_lock, 1)) {
    }
}

static void unlock_writes(void) {
    __sync_lock_release(&broker_write_lock);
}

/* Where the broker listens. The socket is a systemd user socket, so the runtime
 * directory is where it is; MOSAIC_BINDER_SOCKET overrides it for a harness. */
static const char *broker_socket_path(void) {
    if (broker_path[0]) return broker_path;
    const char *set = getenv("MOSAIC_BINDER_SOCKET");
    if (set && set[0]) {
        int i = 0;
        while (set[i] && i < (int)sizeof(broker_path) - 1) {
            broker_path[i] = set[i];
            i++;
        }
        broker_path[i] = 0;
        return broker_path;
    }
    const char *runtime = getenv("XDG_RUNTIME_DIR");
    if (runtime && runtime[0]) {
        int i = 0;
        const char *suffix = "/mosaic/broker.sock";
        while (runtime[i] && i < (int)sizeof(broker_path) - 32) {
            broker_path[i] = runtime[i];
            i++;
        }
        for (int k = 0; suffix[k]; k++) broker_path[i++] = suffix[k];
        broker_path[i] = 0;
    }
    return broker_path[0] ? broker_path : 0;
}

static int broker_connect(void) {
    if (!broker_enabled()) return -1;
    if (broker_fd >= 0) return broker_fd;
    const char *path = broker_socket_path();
    if (!path) return -1;

    say("android-binder: broker path ");
    say(path);
    say("\n");
    say_once();
    int fd = (int)syscall(SYS_SOCKET, 1 /* AF_UNIX */, 1 /* SOCK_STREAM */, 0);
    if (fd < 0) {
        say("android-binder: no socket\n");
        say_once();
        return -1;
    }

    /* struct sockaddr_un: a family and a path, the path starting at offset 2. */
    unsigned char addr[110];
    for (int i = 0; i < 110; i++) addr[i] = 0;
    addr[0] = 1; /* AF_UNIX */
    addr[1] = 0;
    int i = 0;
    while (path[i] && i < 106) {
        addr[2 + i] = (unsigned char)path[i];
        i++;
    }
    long connected = syscall(SYS_CONNECT, fd, addr, 2 + i + 1);
    if (connected != 0) {
        say("android-binder: connect to the broker failed (");
        say_dec(connected);
        say(")\n");
        say_once();
        syscall(SYS_CLOSE, fd);
        return -1;
    }
    if (write(fd, MOSAIC_WIRE_MAGIC, 4) != 4) {
        syscall(SYS_CLOSE, fd);
        return -1;
    }
    broker_fd = fd;
    return fd;
}

static void put_u32(unsigned char *p, uint32 v) {
    p[0] = (unsigned char)(v >> 24);
    p[1] = (unsigned char)(v >> 16);
    p[2] = (unsigned char)(v >> 8);
    p[3] = (unsigned char)v;
}

static uint32 get_u32(const unsigned char *p) {
    return ((uint32)p[0] << 24) | ((uint32)p[1] << 16) | ((uint32)p[2] << 8) | (uint32)p[3];
}

static void put_u64(unsigned char *p, ulong v) {
    put_u32(p, (uint32)(v >> 32));
    put_u32(p + 4, (uint32)v);
}

static ulong get_u64(const unsigned char *p) {
    return ((ulong)get_u32(p) << 32) | (ulong)get_u32(p + 4);
}

/* Write every byte, whatever the kernel returns. */
static int write_all(int fd, const unsigned char *data, ulong size) {
    ulong done = 0;
    while (done < size) {
        long n = write(fd, data + done, size - done);
        if (n <= 0) return -1;
        done += (ulong)n;
    }
    return 0;
}

static int read_all(int fd, unsigned char *data, ulong size) {
    ulong done = 0;
    while (done < size) {
        long n = syscall(SYS_READ, fd, data + done, size - done);
        if (n <= 0) return -1;
        done += (ulong)n;
    }
    return 0;
}

static int broker_send(uint32 kind, uint32 a, uint32 b, uint32 c, ulong node,
                       const unsigned char *data, ulong size) {
    int fd = broker_connect();
    if (fd < 0) return -1;
    if (size > MAX_BROKER_FRAME) return -1;

    unsigned char header[32];
    header[0] = (unsigned char)kind;
    header[1] = MOSAIC_WIRE_VERSION;
    header[2] = 0;
    header[3] = 0;
    put_u32(header + 4, a);
    put_u32(header + 8, b);
    put_u32(header + 12, c);
    put_u64(header + 16, node);
    put_u32(header + 24, (uint32)size);
    put_u32(header + 28, 0);

    /* A frame is written whole or not at all, which needs a lock of its own.
     *
     * It used to be the caller's request/answer lock, which was a deadlock
     * waiting to happen: the thread that serves *incoming* transactions answers
     * on this same socket, and it had to take that lock to send -- while a caller
     * holding it waited for an answer the reader thread had to deliver. Serving a
     * transaction and making one at the same time is exactly what happens when
     * one framework process calls a service another one hosts, so the lock is
     * here, around the bytes, and the request/answer lock covers only the
     * send-and-wait pairs that need a single answer slot. */
    lock_writes();
    int result = write_all(fd, header, 32);
    if (result == 0 && size > 0) result = write_all(fd, data, size);
    unlock_writes();
    if (result != 0) {
        say("android-binder: sending to the broker failed (kind ");
        say_dec((long)kind);
        say(")\n");
        say_once();
    }
    return result;
}

/* Read one frame. `data` is the caller's buffer and `size` its capacity on the
 * way in, the body's length on the way out. */
static int broker_recv(uint32 *kind, uint32 *a, uint32 *b, uint32 *c, ulong *node,
                       unsigned char *data, ulong *size, int *fds, uint32 *fd_count) {
    int fd = broker_fd;
    if (fd < 0) return -1;
    if (fd_count) *fd_count = 0;
    unsigned char header[32];
    /* The header is read with recvmsg, not read, and that is not a detail: a
     * descriptor is ancillary data on the *first* byte of the frame, so a plain
     * read of the header takes it and throws it away. What that looked like was an
     * answer whose descriptors never arrived, a count that did not match, and a
     * reader thread that gave up -- after which nothing was ever answered again. */
    unsigned char control[CMSG_SPACE_BYTES];
    struct mosaic_iovec header_iov;
    header_iov.base = header;
    header_iov.len = sizeof(header);
    struct mosaic_msghdr header_msg;
    header_msg.name = 0;
    header_msg.name_len = 0;
    header_msg.iov = &header_iov;
    header_msg.iov_len = 1;
    header_msg.control = control;
    header_msg.control_len = sizeof(control);
    header_msg.flags = 0;
    long got_header = syscall(SYS_RECVMSG, fd, &header_msg, 0);
    if (got_header != (long)sizeof(header)) return -1;
    uint32 header_fds = 0;
    {
        ulong at = 0;
        while (at + sizeof(struct mosaic_cmsghdr) <= header_msg.control_len) {
            struct mosaic_cmsghdr *cmsg = (struct mosaic_cmsghdr *)(control + at);
            if (cmsg->len < sizeof(struct mosaic_cmsghdr)) break;
            if (cmsg->level == SOL_SOCKET && cmsg->type == SCM_RIGHTS) {
                ulong payload = cmsg->len - sizeof(struct mosaic_cmsghdr);
                int *numbers = (int *)((unsigned char *)cmsg + sizeof(struct mosaic_cmsghdr));
                for (ulong i = 0; i + sizeof(int) <= payload && header_fds < MAX_BROKER_FDS;
                     i += sizeof(int)) {
                    if (fds) fds[header_fds] = numbers[i / sizeof(int)];
                    header_fds++;
                }
            }
            at += (cmsg->len + 7) & ~(ulong)7;
        }
    }
    if (header[1] != MOSAIC_WIRE_VERSION) return -1;

    *kind = header[0];
    *a = get_u32(header + 4);
    *b = get_u32(header + 8);
    *c = get_u32(header + 12);
    *node = get_u64(header + 16);
    ulong length = get_u32(header + 24);
    uint32 wanted = get_u32(header + 28);
    if (wanted > MAX_BROKER_FDS) return -1;
    if (length > MAX_BROKER_FRAME || length > *size) return -1;
    /* The body and the descriptors arrive together, and a descriptor cannot be
     * read out of the stream afterwards: it comes as an ancillary message on the
     * same recvmsg that carries the first bytes of the body. So the body is read
     * with recvmsg rather than read(), which is also why an answer with
     * descriptors could not be read at all before this. */
    if (length > 0 && read_all(fd, data, length) != 0) return -1;
    /* The descriptors came with the header; what the frame claims has to be what
     * arrived, or the answer is not this answer. */
    if (header_fds != wanted) return -1;
    if (fd_count) *fd_count = header_fds;
    *size = length;
    return 0;
}

static int service_name_of(const unsigned char *data, ulong size, char *out, int cap) {
    return name_after_token(data, size, out, cap) != 0;
}

/* Take the object argument with libbinder's own reader.
 *
 * The object table records where each object *ends*, and a binder object is 24
 * bytes, so its start is 24 before the first recorded offset. Reading it through
 * libbinder rather than out of the bytes means the type decides what the fields
 * mean, and -- the point of doing it this way -- readStrongBinder takes a
 * reference, which is left in place: the registry holds the service from here on.
 * That is what stops a remembered object from going stale and taking the
 * framework's getService down with it when it is handed back.
 *
 * This replaced a hand parse of the flat_binder_object whose pointer was stored
 * with no reference at all.
 *
 * readStrongBinder returns an sp<IBinder> by value, and sp has a user-declared
 * destructor, so the Itanium ABI returns it through a hidden first pointer:
 * calling it as though it returned a pointer in rax is what made the first
 * attempt at this crash. */
/* Where the object is, found rather than computed.
 *
 * Three attempts to work the position out from the name's length were wrong for
 * the even-length names -- the padding between a string16 and the object that
 * follows it is not the same in every request, and the object table's offsets do
 * not agree with any single prefix length either. So the object is found by its
 * own type word instead: a binder object starts with BINDER_TYPE_BINDER or
 * BINDER_TYPE_HANDLE over 24 bytes, which is a thing to search for.
 *
 * The bytes are a request from this process's own service manager call, and the
 * first object in it is the service being registered, so the first match is the
 * one. A match that is not an object would need the type word to appear by
 * accident in the arguments, which is what the type check in the parse below is
 * for: get it wrong and nothing is registered, rather than something wrong.
 */
static const unsigned char *find_object(const unsigned char *data, ulong size, const unsigned char *from) {
    if (!data) return 0;
    const unsigned char *end = data + size;
    const unsigned char *p = from ? from : data;
    for (; p + 24 <= end; p += 4) {
        uint32 type = u32_at(p);
        if (type != BINDER_TYPE_BINDER && type != BINDER_TYPE_HANDLE) continue;
        /* A type word alone is not enough: the arguments can contain it by
         * accident, and one that did was handed back as a service and took the
         * framework down inside flatten_binder. The rest of an object has to look
         * like one too -- flags that are binder flags, a cookie, and for a local
         * object the weak reference table alongside it. */
        if (u32_at(p + 4) > 0x1ff) continue;      /* FLAT_BINDER_FLAG_* fit here */
        ulong cookie = 0;
        for (int i = 0; i < 8; i++) cookie |= ((ulong)p[16 + i]) << (8 * i);
        if (!cookie) continue;
        if (type == BINDER_TYPE_BINDER) {
            ulong weakrefs = 0;
            for (int i = 0; i < 8; i++) weakrefs |= ((ulong)p[8 + i]) << (8 * i);
            if (!weakrefs) continue;
        }
        return p;
    }
    return 0;
}

/* Take the object argument, by libbinder's reader where that works and by the
 * type word where it does not.
 *
 * readStrongBinder is preferred because it takes a reference, and the registry
 * then owns the service from registration on. It returns nothing for the Java
 * requests this exists for, so the fallback reads the object directly: type at 0,
 * cookie at 16, the layout Parcel::unflattenBinder's own code uses. The fallback
 * holds no reference, which is a thing to fix here; the framework's own services
 * live as long as the process, so it is the AIDL half that matters for it.
 */
static void *object_at(void *parcel, const unsigned char *data, ulong size,
                       const unsigned char *after_name, unsigned long *weakrefs_out) {
    const unsigned char *found = find_object(data, size, after_name);
    if (weakrefs_out) *weakrefs_out = 0;
    if (found && weakrefs_out) {
        unsigned long w = 0;
        for (int i = 0; i < 8; i++) w |= ((unsigned long)found[8 + i]) << (8 * i);
        *weakrefs_out = w;
    }

    /* The reader first: it is the one that takes a reference, the way a real
     * binder node does on registration, so the registry owns the service from
     * registration on -- and a service the framework registers from a temporary,
     * `platform_compat` among them, stays alive because of it. The position to
     * read from is the one find_object found; computing it from the name's length
     * was wrong for even-length names and the object table's offsets did not agree
     * either, which is why this reader was tried at guessed positions, came back
     * empty, and was left off. */
    if (parcel && found && parcel_set_position && parcel_read_strong && parcel_data_position) {
        ulong saved = parcel_data_position(parcel);
        if ((ulong)(found - data) + 24 <= size) {
            parcel_set_position(parcel, (ulong)(found - data));
            /* The sp lands here and is deliberately not destroyed: its reference
             * is the registry's. */
            unsigned long held[2] = {0, 0};
            parcel_read_strong(held, parcel);
            parcel_set_position(parcel, saved);
            if (held[0]) {
                static int said = 0;
                if (said < 3) {
                    said++;
                    say("android-binder:   object taken by readStrongBinder, reference held\n");
                    say_once();
                }
                return (void *)held[0];
            }
        }
        parcel_set_position(parcel, saved);
    }

    if (!found) return 0;
    const unsigned char *object = found;

    /* Which of the two pointer fields is the IBinder.
     *
     * The format has `binder` at 8 and `cookie` at 16, and for a local object one
     * of them is the BBinder and the other its weak reference table. Which is
     * which was assumed rather than checked, and the assumption was that cookie is
     * the object -- so every Java registration was handed back the *other* one,
     * which is why libbinder's reader crashed calling a virtual method on it.
     *
     * Both are now taken as candidates and the one that looks like an IBinder wins:
     * an IBinder's first word is its vtable, which is a pointer into a mapped
     * library, and a weak reference table's first word is not. That check is a
     * read of the candidate's first word, which is what a wrong guess would fault
     * on -- so it is done on both and the winner is chosen without trusting either. */
    unsigned long fields[2];
    for (int f = 0; f < 2; f++) {
        unsigned long v = 0;
        for (int i = 0; i < 8; i++) v |= ((unsigned long)object[8 + f * 8 + i]) << (8 * i);
        fields[f] = v;
    }

    static int reported = 0;
    if (reported < 4) {
        reported++;
        say("android-binder:   object at ");
        say_dec((long)(object - data));
        for (int f = 0; f < 2; f++) {
            say(f ? " cookie=0x" : " binder=0x");
            for (int shift = 60; shift >= 0; shift -= 4) {
                static const char hex[] = "0123456789abcdef";
                char digit[2];
                digit[0] = hex[(fields[f] >> shift) & 0xf];
                digit[1] = 0;
                say(digit);
            }
            unsigned long w0 = 0;
            if (fields[f] >= 0x10000 && fields[f] < 0x800000000000UL) {
                w0 = *(const unsigned long *)fields[f];
            }
            say(" w0=0x");
            for (int shift = 60; shift >= 0; shift -= 4) {
                static const char hex[] = "0123456789abcdef";
                char digit[2];
                digit[0] = hex[(w0 >> shift) & 0xf];
                digit[1] = 0;
                say(digit);
            }
        }
        say("\n");
        say_once();
    }

    /* Which of the two fields is the IBinder, read off libbinder rather than
     * guessed.
     *
     * A flat_binder_object is type at 0, flags at 4, and two pointers at 8 and 16.
     * For a local object, Parcel::flattenBinder() -- the function on the other side
     * of the hand-back -- calls RefBase::getWeakRefs() and stores the result at
     * `%rsp+0x8`, then stores the BBinder at `%rsp+0x10`. So offset 8 is the weak
     * reference table and offset 16 is the BBinder. This code used to hand back
     * offset 8, and flattenBinder died on it: it reads the object's first word as a
     * vtable and calls localBinder() through it, and the weak reference table's
     * first word is its refcounts -- {strong=0, weak=1} is 0x100000000, not a code
     * pointer. That is the SIGSEGV at Parcel::flattenBinder+52 that stopped
     * StartActivityManager.
     *
     * Offset 16 is therefore preferred; offset 8 is a fallback only. Which
     * candidate is an object is decided by reading its first word and asking
     * whether it looks like a vtable -- a userspace pointer. Both are pointers
     * libbinder itself wrote into the parcel, so reading them is safe, and the
     * size test rejects the small integers a handle or an fd would leave here
     * before anything is dereferenced. */
    unsigned long type = u32_at(object);
    if (type == BINDER_TYPE_HANDLE) {
        /* A handle, not an object: the field at 8 is a 32-bit index into the
         * *sender's* handle table. There is nothing to hand back for one of these:
         * a proxy would need a handle in *this* process's table, and the broker is
         * the thing that allocates those. */
        static int said = 0;
        if (said < 3) {
            said++;
            say("android-binder:   object is a handle, not a local binder; not handing it back\n");
            say_once();
        }
        return 0;
    }
    for (int f = 1; f >= 0; f--) {
        unsigned long candidate = fields[f];
        if (candidate < 0x10000 || candidate >= 0x800000000000UL) continue;
        unsigned long first = *(const unsigned long *)candidate;
        if (first >= 0x10000 && first < 0x800000000000UL) return (void *)candidate;
    }
    return 0;
}


/* Whether a remembered pointer is still an IBinder.
 *
 * `Parcel::flattenBinder` calls `binder->localBinder()` through the object's
 * vtable at byte 0x60 -- read off the disassembly, `mov (%rdi),%rax; call
 * *0x60(%rax)` -- and then calls `setParceled()` on what comes back. So the
 * question "is this still an IBinder" is exactly "does the entry at 0x60 resolve
 * to a localBinder".
 *
 * Asking which library that entry points into was not enough. `memtrack.proxy`
 * is registered from native code whose reference is dropped, and its freed memory
 * later held a vtable whose 0x60 entry *was* in libbinder -- so the check passed,
 * `localBinder()` returned garbage (0x48, a small integer, not a pointer), and
 * `setParceled()` wrote through it: SIGSEGV in `BBinder::setParceled`, which is
 * how StartActivityManager died. Requiring the resolved symbol to be named
 * `localBinder` rejects that object, and the service is answered absent instead,
 * which is what a caller of a service that no longer exists should hear.
 *
 * The vtable entry is read and resolved, never called, and dladdr only inspects
 * the address. */
extern char *strstr(const char *, const char *);

typedef struct {
    const char *dli_fname;
    void *dli_fbase;
    const char *dli_sname;
    void *dli_saddr;
} dl_info_t;
extern int dladdr(const void *, dl_info_t *);

static int looks_like_ibinder(void *object) {
    if (!object) return 0;
    unsigned long address = (unsigned long)object;
    if (address < 0x10000 || address >= 0x800000000000UL) return 0;
    unsigned long vtable = *(const unsigned long *)object;
    if (vtable < 0x10000 || vtable >= 0x800000000000UL) return 0;
    void *slot = ((void **)vtable)[0x60 / 8];
    if (!slot) return 0;
    dl_info_t info;
    __builtin_memset(&info, 0, sizeof info);
    int resolved = dladdr(slot, &info);
    int ok = resolved && info.dli_sname && strstr(info.dli_sname, "localBinder") != 0;
    /* What the slot holds, once per distinct object: the hand-back's safety turns
     * on it, and reading it back beats assuming. */
    static void *reported[6];
    static int reported_count = 0;
    int known = 0;
    for (int i = 0; i < reported_count; i++) {
        if (reported[i] == object) known = 1;
    }
    if (!known && reported_count < 6) {
        reported[reported_count++] = object;
        say("android-binder:   vtable slot 0x60 of ");
        say_dec((long)object);
        say(" -> ");
        say(info.dli_sname ? info.dli_sname : "(no symbol)");
        say(ok ? " (a local binder)\n" : " (not a local binder)\n");
        say_once();
    }
    return ok;
}

/* A trace written straight to a file, for questions the log pipeline cannot
 * answer: say() goes through a line budget and stderr, and the framework's own
 * output comes through logd, so their order cannot be compared. This does not
 * share either. */
#define SYS_WRITE 1
#define SYS_OPENAT 257
static int trace_fd = -2;
static int trace_wanted(void) {
    if (trace_fd == -2) {
        const char *on = getenv("MOSAIC_BINDER_TRACE");
        if (on && on[0] == '1') {
            trace_fd = (int)syscall(SYS_OPENAT, -100, "/tmp/mosaic-sm-trace.log",
                                    0x441 /* O_WRONLY|O_CREAT|O_APPEND */, 0644);
        } else {
            trace_fd = -1;
        }
    }
    return trace_fd >= 0;
}
static void trace_num(const char *what, long a, long b) {
    if (!trace_wanted()) return;
    char buf[160];
    long n = 0;
    for (const char *p = what; p && *p && n < 60; p++) buf[n++] = *p;
    buf[n++] = ' ';
    if (a < 0) { buf[n++] = '-'; a = -a; }
    char digits[24];
    int d = 0;
    if (a == 0) digits[d++] = '0';
    while (a > 0) { digits[d++] = (char)('0' + a % 10); a /= 10; }
    while (d > 0) buf[n++] = digits[--d];
    buf[n++] = ' ';
    if (b < 0) { buf[n++] = '-'; b = -b; }
    d = 0;
    if (b == 0) digits[d++] = '0';
    while (b > 0) { digits[d++] = (char)('0' + b % 10); b /= 10; }
    while (d > 0) buf[n++] = digits[--d];
    buf[n++] = '\n';
    syscall(SYS_WRITE, trace_fd, buf, n);
}

static void trace_pair(const char *what, const char *name) {
    if (!trace_wanted()) return;
    char buf[256];
    long n = 0;
    for (const char *p = what; p && *p && n < 200; p++) buf[n++] = *p;
    buf[n++] = ' ';
    for (const char *p = name ? name : "(null)"; *p && n < 240; p++) buf[n++] = *p;
    buf[n++] = '\n';
    syscall(SYS_WRITE, trace_fd, buf, n);
}

static void broker_export(const char *name, ulong node);
static ulong remember_object(void *object, unsigned long cookie);

static void remember(const char *name, void *object, unsigned long cookie) {
    ulong node = 0;
    int listed = 0;
    if (!name || !object) return;
    lock_registry();
    for (int i = 0; i < service_count; i++) {
        if (services[i].name[0] && length(services[i].name) == length(name)) {
            int same = 1;
            for (ulong k = 0; k < length(name); k++) {
                if (services[i].name[k] != name[k]) {
                    same = 0;
                    break;
                }
            }
            if (same) {
                services[i].object = object;
                services[i].cookie = cookie;
                unlock_registry();
                return;
            }
        }
    }
    if (service_count < MAX_SERVICES) {
        int n = 0;
        while (name[n] && n < NAME_MAX - 1) {
            services[service_count].name[n] = name[n];
            n++;
        }
        services[service_count].name[n] = 0;
        services[service_count].object = object;
        services[service_count].cookie = cookie;
        /* A node id carries this process's pid in its high half, which is how
         * the broker stops one process from publishing another's object. */
        services[service_count].node = ((ulong)syscall(SYS_GETPID) << 32) | (ulong)(service_count + 1);
        node = services[service_count].node;
        service_count++;
        listed = 1;
    }
    unlock_registry();
    if (listed) broker_export(name, node);
}

/* The object, and the weak reference table that goes beside it in a hand-back.
 *
 * A flat_binder_object carries both, and the hand-back writes one directly now
 * rather than calling Parcel::writeStrongBinder. That call reaches
 * IPCThreadState::self(), which re-enters binder initialisation from inside a
 * transaction and deadlocks on ART's arena pool on the NDK checkService path --
 * the hang that replaced the SIGSEGV. Parcel::writeObject does not.
 */
static void *lookup_full(const char *name, unsigned long *weakrefs_out) {
    if (weakrefs_out) *weakrefs_out = 0;
    if (!name) return 0;
    lock_registry();
    for (int i = 0; i < service_count; i++) {
        if (length(services[i].name) != length(name)) continue;
        int same = 1;
        for (ulong k = 0; k < length(name); k++) {
            if (services[i].name[k] != name[k]) {
                same = 0;
                break;
            }
        }
        if (same) {
            void *object = services[i].object;
            if (weakrefs_out) *weakrefs_out = services[i].cookie;
            unlock_registry();
            return object;
        }
    }
    unlock_registry();
    return 0;
}

static void *lookup(const char *name) {
    return lookup_full(name, 0);
}

static int registered_count(void) {
    lock_registry();
    int count = service_count;
    unlock_registry();
    return count;
}

/* The registry, for the driver-level shim in probe.so. */
void mosaic_binder_remember(const char *name, void *object) {
    remember(name, object, 0);
}

void *mosaic_binder_lookup(const char *name) {
    return lookup(name);
}

/* ---- using the socket ----------------------------------------------------- */



#define SYS_GETPID 39
#define SYS_RECVMSG 47

struct timespec {
    long tv_sec;
    long tv_nsec;
};

/* One request is outstanding at a time: this side makes them synchronously, and
 * the reader thread below fills the slot when the answer arrives. */
static int response_ready = 0;
/* What the current caller is waiting for: the kind of answer *and* the request it
 * answers. One caller at a time -- the request/answer lock is held across the send
 * and the wait -- but a broker that gives up on a slow owner can still deliver that
 * owner's answer later, and a late answer of the right kind is not this call's
 * answer. Matching on the kind alone is what let one be taken for the other. */
static uint32 awaiting_kind = 0;
static uint32 awaiting_id = 0;
/* Names a request. Under the same lock as the wait, so no two live requests share
 * one. */
static uint32 next_request_id = 1;
static uint32 response_kind = 0;
static uint32 response_id = 0;
static uint32 response_a = 0;
static uint32 response_b = 0;
static uint32 response_c = 0;
static ulong response_length = 0;
static ulong response_node = 0;
static unsigned char *response_data = 0;
static ulong response_capacity = 0;
/* Descriptors an answer carried. They arrive with the frame, and the side that
 * writes the answer into the caller's Parcel is the one that has to put their
 * numbers in it. */
static int response_fds[MAX_BROKER_FDS];
static uint32 response_fd_count = 0;

/* The binder objects an answer carries, as libbinder's reply path wants them.
 * Written beside the answer buffer, under the same assumption the rest of this
 * file makes: one transaction to the broker is in flight at a time. */
#define MAX_REPLY_OBJECTS 64
/* As many objects as one request may pass. The cap is this side's, not binder's:
 * a request with more is refused rather than silently truncated, because a
 * truncated list would leave an object in the data that the callee reads as a
 * number. */
#define MAX_ARGUMENT_OBJECTS 64
static unsigned long reply_objects[MAX_REPLY_OBJECTS];

/* An answer's data stops before its trailing object offsets: four bytes each,
 * big-endian, which is what the broker appends so a reader that knows nothing of
 * objects still reads the same data. */
static ulong answer_data_size(ulong size, uint32 count) {
    if (count > 0 && size >= (ulong)count * 4) return size - (ulong)count * 4;
    return size;
}

static ulong answer_offset(const unsigned char *answer, ulong size, uint32 index) {
    const unsigned char *word = answer + size + index * 4;
    return ((ulong)word[0] << 24) | ((ulong)word[1] << 16) | ((ulong)word[2] << 8) | word[3];
}

static uint32 reply_object_count(uint32 count) {
    return count > MAX_REPLY_OBJECTS ? MAX_REPLY_OBJECTS : count;
}

/* The objects among a request's arguments, as the refs the broker needs: twelve
 * bytes each, an offset then a node, both big-endian.
 *
 * An object the caller *owns* is named by its node -- the callee cannot be handed
 * a pointer into this process, and this is the side that can export it. One the
 * caller merely holds a handle to needs no naming: the broker has the caller's
 * table, so a node of zero says "the four bytes at that offset are a handle, look
 * it up there". That distinction is the whole of what the broker needs.
 *
 * Returns how many refs were written, or -1 if the buffer is too small. */
static int collect_argument_objects(const unsigned char *data, ulong size, const unsigned long *offsets,
                                    ulong count, unsigned char *out, uint32 capacity) {
    uint32 written = 0;
    for (ulong i = 0; i < count; i++) {
        ulong at = offsets[i];
        if (at + 24 > size) continue;
        uint32 kind = 0;
        __builtin_memcpy(&kind, data + at, 4);
        ulong node = 0;
        if (kind == BINDER_TYPE_BINDER || kind == BINDER_TYPE_WEAK_BINDER) {
            /* A local object: `binder` is the pointer this process knows it by, and
             * the same pointer the registry remembers it under. */
            unsigned long object = 0;
            unsigned long cookie = 0;
            __builtin_memcpy(&object, data + at + 8, 8);
            __builtin_memcpy(&cookie, data + at + 16, 8);
            node = remember_object((void *)object, cookie);
            if (node == 0) continue;
        }
        if (written >= capacity) return -1;
        put_u32(out + written * 12, (uint32)at);
        put_u64(out + written * 12 + 4, node);
        written++;
    }
    return (int)written;
}

/* The offsets of an answer's objects, converted for `copy_out`. */
static const unsigned long *answer_objects(const unsigned char *answer, ulong size, uint32 count) {
    count = reply_object_count(count);
    for (uint32 i = 0; i < count; i++) {
        reply_objects[i] = answer_offset(answer, size, i);
    }
    return reply_objects;
}

/* Write an answer into the caller's Parcel.
 *
 * An answer may carry binder objects -- a display token, a wake lock -- and an
 * object is not just bytes: the reader looks it up in the parcel's object table,
 * and one that is missing there is refused rather than handed over. So each one
 * is written through libbinder's own writer, which is what puts it in the table. */
static void write_broker_answer(void *reply, const unsigned char *answer, ulong size, uint32 count) {
    ulong data_size = answer_data_size(size, count);
    count = reply_object_count(count);
    ulong at = 0;
    uint32 fd_used = 0;
    for (uint32 i = 0; i < count; i++) {
        ulong offset = answer_offset(answer, data_size, i);
        if (offset < at || offset + 28 > data_size) break;
        if (offset > at && parcel_write_bytes) parcel_write_bytes(reply, answer + at, offset - at);
        flat_object_t object;
        __builtin_memcpy(&object, answer + offset, sizeof(object));
        int stability = 0;
        __builtin_memcpy(&stability, answer + offset + 24, 4);
        if (object.type == BINDER_TYPE_FD) {
            /* A descriptor: the number in this word has to be one this process can
             * open, and the descriptors came with the answer. */
            if (fd_used < response_fd_count) {
                write_object_into(reply, BINDER_TYPE_FD, (ulong)response_fds[fd_used], 0, stability);
                fd_used++;
            }
        } else {
            write_object_into(reply, object.type, object.binder, object.cookie, stability);
        }
        at = offset + 28;
    }
    if (at < data_size && parcel_write_bytes) parcel_write_bytes(reply, answer + at, data_size - at);
}

/* Put this process's descriptor numbers into the descriptor words of an answer.
 *
 * The answer comes from another process, where the same descriptors have other
 * numbers; the ones that came with the frame are this process's, and the word in
 * the caller's Parcel has to name one of those. Written in place: the answer
 * buffer belongs to this call. */
static void write_answer_fds(unsigned char *answer, ulong size, uint32 count) {
    count = reply_object_count(count);
    uint32 used = 0;
    for (uint32 i = 0; i < count; i++) {
        ulong offset = answer_offset(answer, size, i);
        if (offset + 28 > size) break;
        uint32 type = 0;
        __builtin_memcpy(&type, answer + offset, 4);
        if (type != BINDER_TYPE_FD) continue;
        if (used >= response_fd_count) break;
        ulong number = (ulong)response_fds[used++];
        __builtin_memcpy(answer + offset + 8, &number, 8);
    }
}

static int reader_started = 0;

static void *broker_reader(void *arg);

static void start_reader(void) {
    if (reader_started) return;
    if (broker_connect() < 0) return;
    unsigned long thread = 0;
    /* pthread_create comes from libc, which the process has even though this
     * library was built without one. */
    if (pthread_create(&thread, 0, broker_reader, 0) == 0) {
        reader_started = 1;
    } else {
        say("android-binder: the reader thread would not start\n");
        say_once();
    }
}

/* Wait for the answer to whatever was just sent, and take it.
 *
 * The kind is checked rather than ignored. One slot serves every caller, and the
 * reader thread fills it with whatever arrives -- so a *late* answer, to a request
 * that already gave up, used to be taken by the next caller as its own and its
 * fields read as that caller's. A lookup that took a transaction's reply read the
 * reply's status as a handle, and handed the framework a handle to an object that
 * does not exist. */
static int await_response(uint32 want_kind, uint32 want_id, unsigned char **data, ulong *size, uint32 *a, uint32 *b, uint32 *c) {
    /* Longer than the broker's own patience with a slow owner
     * (`FORWARD_TIMEOUT`, thirty seconds in src/binder/transport.rs), because a
     * caller that gives up first turns a slow answer into a missing service: the
     * broker may still deliver that answer, and by then this side has moved on to
     * another request whose kind matches. Five seconds here against thirty there
     * was exactly that asymmetry. */
    for (int i = 0; i < 35000; i++) { /* thirty-five seconds */
        if (__sync_bool_compare_and_swap(&response_ready, 1, 0)) {
            if (response_kind != want_kind || response_id != want_id) {
                /* Not this call's answer -- a late one, for a request that already
                 * gave up. Leaving it consumed and waiting again is right: the
                 * broker answers what was asked, and this is something nobody is
                 * waiting for any more. */
                say("android-binder: dropped an answer that was not awaited\n");
                say_once();
                continue;
            }
            *data = response_data;
            *size = response_length;
            *a = response_a;
            *b = response_b;
            *c = response_c;
            return 0;
        }
        struct timespec step = {0, 1000000}; /* one millisecond */
        syscall(SYS_NANOSLEEP, &step, 0);
    }
    return -1;
}

/* Publish a name for a node this process owns.
 *
 * No request/answer lock here: nothing is awaited. It used to take one, which
 * meant a registration from the serving thread could block behind a caller's
 * outstanding transaction. */
static void broker_export(const char *name, ulong node) {
    start_reader();
    if (broker_fd < 0) return;
    ulong size = name ? length(name) : 0;
    if (broker_send(KIND_EXPORT, 0, 0, 0, node, (const unsigned char *)name, size) == 0) {
        say("android-binder: published ");
        say(name ? name : "(an object handed over as an argument)");
        say("\n");
        say_once();
    }
}

/* The node for an object this process owns, remembered without a name.
 *
 * An object passed *into* a transaction -- a callback a caller wants called back
 * -- has no name: nobody looks it up, the callee is handed it. It still needs a
 * node, because the broker addresses objects by node, and the same object passed
 * twice has to keep one identity. Found by pointer, which is what the caller's
 * Parcel holds for a local binder.
 */
static ulong remember_object(void *object, unsigned long cookie) {
    if (!object) return 0;
    ulong node = 0;
    int listed = 0;
    lock_registry();
    for (int i = 0; i < service_count; i++) {
        if (services[i].object == object) {
            unlock_registry();
            return services[i].node;
        }
    }
    if (service_count < MAX_SERVICES) {
        services[service_count].name[0] = 0;
        services[service_count].object = object;
        services[service_count].cookie = cookie;
        services[service_count].node = ((ulong)syscall(SYS_GETPID) << 32) | (ulong)(service_count + 1);
        node = services[service_count].node;
        service_count++;
        listed = 1;
    }
    unlock_registry();
    if (listed) broker_export(0, node);
    return node;
}

/* Ask the broker for a name. Returns a handle in this process's table, or
 * NO_HANDLE. */
static uint32 broker_lookup(const char *name, ulong *node, uint32 *owner) {
    if (!name) return NO_HANDLE;
    start_reader();
    if (broker_fd < 0) return NO_HANDLE;
    lock_broker();
    uint32 id = next_request_id++;
    awaiting_kind = KIND_FOUND;
    awaiting_id = id;
    if (broker_send(KIND_LOOKUP, id, 0, 0, 0, (const unsigned char *)name, length(name)) != 0) {
        awaiting_kind = 0;
        awaiting_id = 0;
        unlock_broker();
        return NO_HANDLE;
    }
    unsigned char *data = 0;
    ulong size = 0;
    uint32 a = 0, b = 0, c = 0;
    int waited = await_response(KIND_FOUND, id, &data, &size, &a, &b, &c);
    awaiting_kind = 0;
    awaiting_id = 0;
    unlock_broker();
    if (waited != 0) return NO_HANDLE;
    if (node) *node = response_node;
    if (owner) *owner = b;
    return a;
}

/* Send a transaction to an object another process owns and take its answer.
 *
 * `objects` are the refs the arguments carry (twelve bytes each, see
 * `collect_argument_objects`), appended to the body so that a transaction with
 * none is byte for byte what it always was; the count rides in the upper half of
 * the header's flags word, which is otherwise two bits wide. */
static int broker_transact(uint32 handle, uint32 code, uint32 flags, const unsigned char *data,
                           ulong size, const unsigned char *objects, uint32 object_count,
                           unsigned char **reply, ulong *reply_size, uint32 *status,
                           uint32 *object_count_out);

/* Serve one transaction another process sent to an object here. */
static void broker_serve(uint32 id, ulong node, uint32 code, uint32 flags, uint32 object_count,
                         const unsigned char *data, ulong size);

static void *broker_reader(void *arg) {
    (void)arg;
    if (!response_data) {
        response_capacity = 64 * 1024;
        response_data = (unsigned char *)malloc(response_capacity);
    }
    if (!response_data) return 0;
    say("android-binder: reader thread running\n");
    say_once();
    for (;;) {
        uint32 kind = 0, a = 0, b = 0, c = 0;
        ulong node = 0;
        ulong size = response_capacity;
        uint32 fd_count = 0;
        if (broker_recv(&kind, &a, &b, &c, &node, response_data, &size, response_fds,
                        &fd_count) != 0) {
            break;
        }
        if (kind == KIND_INCOMING) {
            say("android-binder: the broker sent a transaction\n");
            say_once();
            /* `a` is the caller's request id, which the answer echoes. The object
             * count rides in the upper half of `c`, which is otherwise the
             * caller's flags. */
            broker_serve(a, node, b, c & 0xffff, c >> 16, response_data, size);
            continue;
        }
        if (kind == KIND_DEAD) continue;
        /* Only an answer someone is waiting for goes into the slot. A late one --
         * to a request that has already given up -- would otherwise be read as the
         * next caller's answer, with its fields meaning something else. */
        {
            /* Which request this answers, per kind: the reply carries it in `b`,
             * the found in `c`. */
            uint32 id = (kind == KIND_FOUND) ? c : b;
            if (kind != awaiting_kind || id != awaiting_id) {
                say("android-binder: the broker sent an answer nobody waits for\n");
                say_once();
                continue;
            }
            response_id = id;
        }
        response_kind = kind;
        response_a = a;
        response_b = b;
        response_c = c;
        response_node = node;
        response_length = size;
        response_fd_count = fd_count;
        __sync_lock_test_and_set(&response_ready, 1);
    }
    return 0;
}

/* The object a node names, if this process owns it. */
static void *object_for_node(ulong node) {
    lock_registry();
    void *object = 0;
    for (int i = 0; i < service_count; i++) {
        if (services[i].node == node) {
            object = services[i].object;
            break;
        }
    }
    unlock_registry();
    return object;
}

/* ---- joining the Java world -------------------------------------------- */

/* Serving a *Java* binder means entering libandroid_runtime's JavaBBinder, and
 * that asks ART for the calling thread's JNIEnv:
 *
 *   JavaBinder: Binder thread started or Java binder used, but env null. Attach JVM?
 *
 * followed by liblog's fatal assert inside JavaBBinder::onTransact. A thread this
 * shim created is not one ART knows, so a transaction from another process to a
 * framework service -- the case the broker exists for -- took the whole framework
 * down instead of being served. A driver's binder threads attach to the VM for
 * the same reason; this does what they do, through the JNI invocation API libart
 * already exports. The thread is never detached: it lives as long as the process.
 *
 * JavaVM is a pointer to a table of function pointers, and the layout is fixed by
 * the JNI specification: three reserved words, then DestroyJavaVM, then
 * AttachCurrentThread. Nothing else about the struct is touched. */

typedef struct JavaVM_ JavaVM;

typedef struct {
    void *reserved[3];
    int (*DestroyJavaVM)(JavaVM *);
    int (*AttachCurrentThread)(JavaVM *, void **, void *);
} jni_invoke_interface_t;

struct JavaVM_ {
    const jni_invoke_interface_t *functions;
};

typedef int (*get_created_vms_fn)(JavaVM **, int, int *);

static __thread int thread_attached = 0;
static __thread int attach_attempted = 0;

static void attach_to_jvm(void) {
    if (thread_attached || attach_attempted) return;
    attach_attempted = 1;
    void *art = dlopen("libart.so", RTLD_NOW);
    if (!art) {
        say("android-binder: no libart.so, so no VM to attach the serving thread to\n");
        say_once();
        return;
    }
    get_created_vms_fn get_vms = (get_created_vms_fn)dlsym(art, "JNI_GetCreatedJavaVMs");
    if (!get_vms) {
        say("android-binder: JNI_GetCreatedJavaVMs is not exported\n");
        say_once();
        return;
    }
    JavaVM *vm = 0;
    int count = 0;
    if (get_vms(&vm, 1, &count) != 0 || count < 1 || !vm || !vm->functions) {
        say("android-binder: no Java VM has been created\n");
        say_once();
        return;
    }
    void *env = 0;
    if (vm->functions->AttachCurrentThread(vm, &env, 0) == 0) {
        thread_attached = 1;
        say("android-binder: serving thread attached to the Java VM\n");
        say_once();
    } else {
        say("android-binder: attaching the serving thread to the Java VM failed\n");
        say_once();
    }
}

/* Serve one transaction another process sent to an object here.
 *
 * The request arrives as the parcel bytes the sender wrote, so the object is
 * called the way a local call would call it: a Parcel is built over those bytes
 * and BBinder::transact is entered by symbol, which then dispatches to the
 * object's own onTransact. The answer is read back out the same way it was
 * written, objects and all. */
static void broker_serve(uint32 id, ulong node, uint32 code, uint32 flags, uint32 object_count,
                         const unsigned char *data, ulong size) {
    attach_to_jvm();
    void *object = object_for_node(node);
    int status = -1;
    unsigned char *out = 0;
    ulong out_size = 0;
    uint32 out_objects = 0;

    /* The object offsets ride after the data, so the data stops four bytes short
     * of the body for each one. */
    ulong data_size = answer_data_size(size, object_count);
    object_count = reply_object_count(object_count);

    if (object && parcel_ctor && parcel_dtor && binder_transact && parcel_write_bytes) {
        void *request = malloc(PARCEL_BYTES);
        void *reply = malloc(PARCEL_BYTES);
        if (request && reply) {
            parcel_ctor(request);
            /* The bytes the sender wrote are copied into a fresh Parcel rather
             * than referenced in place. ipcSetDataReference is the other way to
             * do it, and it is what this used first, and it did not return: the
             * call was never dispatched and the caller waited until the broker
             * gave up on the connection.
             *
             * The objects among the arguments are written through libbinder's own
             * writer as the copy is made, which is what puts them in the parcel's
             * object table: the object word alone is not enough, because a reader
             * looks an object up in that table and one that is missing there is
             * refused. The handles in those words are this process's own -- the
             * broker wrote them for this side, not for the caller -- so a callee
             * that calls one back reaches the right object. */
            ulong at = 0;
            for (uint32 i = 0; i < object_count; i++) {
                ulong offset = answer_offset(data, data_size, i);
                if (offset < at || offset + 28 > data_size) break;
                if (offset > at) parcel_write_bytes(request, data + at, offset - at);
                flat_object_t argument;
                __builtin_memcpy(&argument, data + offset, sizeof(argument));
                int stability = 0;
                __builtin_memcpy(&stability, data + offset + 24, 4);
                /* An object of this process's own, coming back to it: a handle to
                 * it would mean calling it through the broker, which for an object
                 * it owns is a loop rather than a call. The node the broker left in
                 * the cookie says whether it is ours, and if it is, what goes into
                 * the parcel is the local object, which is what a driver writes. */
                void *own = argument.type == BINDER_TYPE_HANDLE ? object_for_node(argument.cookie) : 0;
                if (own) {
                    write_object_into(request, BINDER_TYPE_BINDER, (ulong)own, argument.cookie, stability);
                } else {
                    write_object_into(request, argument.type, argument.binder, argument.cookie, stability);
                }
                at = offset + 28;
            }
            if (at < data_size) parcel_write_bytes(request, data + at, data_size - at);
            parcel_ctor(reply);
            status = binder_transact(object, code, request, reply, flags);

            const unsigned char *bytes = parcel_ipc_data ? parcel_ipc_data(reply) : 0;
            ulong length = parcel_ipc_data_size ? parcel_ipc_data_size(reply) : 0;
            /* An answer may hand back objects of its own, and they go the same way
             * the caller's did: appended to the body, with the count in the
             * header. */
            const unsigned long *objects = parcel_ipc_objects ? parcel_ipc_objects(reply) : 0;
            ulong object_total = parcel_ipc_objects_count ? parcel_ipc_objects_count(reply) : 0;
            if (objects && object_total > MAX_REPLY_OBJECTS) object_total = MAX_REPLY_OBJECTS;
            ulong tail = objects ? object_total * 4 : 0;
            if (bytes && length + tail > 0) {
                out = (unsigned char *)malloc(length + tail);
                if (out) {
                    if (length > 0) __builtin_memcpy(out, bytes, length);
                    for (ulong i = 0; i < object_total; i++) {
                        put_u32(out + length + i * 4, (uint32)objects[i]);
                    }
                    out_size = length + tail;
                    out_objects = (uint32)object_total;
                }
            }
            parcel_dtor(reply);
            parcel_dtor(request);
        }
        free(reply);
        free(request);
    }
    /* One line per call served, which is the record that a transaction crossed:
     * the caller's side says it got an answer, and this says the owner ran it. */
    say("android-binder: served node ");
    say_dec((long)node);
    say(" code ");
    say_dec((long)code);
    say(status == 0 ? " ok\n" : " failed\n");
    say_once();

    if (broker_fd >= 0) {
        /* `b` carries the caller's id back, so the answer is matched to the
         * request that asked for it. No lock here: `broker_send` takes the write
         * lock itself, and taking it again around it is a deadlock -- the owner
         * spins on its own lock and the broker gives up on it thirty seconds
         * later. */
        broker_send(KIND_INCOMING_REPLY, (uint32)status, id, out_objects, node, out, out_size);
    }
    free(out);
}

/* Send a transaction to an object another process owns and wait for its answer. */
static int broker_transact(uint32 handle, uint32 code, uint32 flags, const unsigned char *data,
                           ulong size, const unsigned char *objects, uint32 object_count,
                           unsigned char **reply, ulong *reply_size, uint32 *status,
                           uint32 *object_count_out) {
    start_reader();
    if (broker_fd < 0) return -1;
    unsigned char *composed = 0;
    if (object_count > 0) {
        ulong body = size + (ulong)object_count * 12;
        if (body > MAX_BROKER_FRAME) return -1;
        composed = (unsigned char *)malloc(body);
        if (!composed) return -1;
        if (size > 0) __builtin_memcpy(composed, data, size);
        __builtin_memcpy(composed + size, objects, (ulong)object_count * 12);
        data = composed;
        size = body;
    }
    lock_broker();
    uint32 id = next_request_id++;
    awaiting_kind = ((flags & 1) != 0) ? 0 : KIND_REPLY;
    awaiting_id = id;
    if (broker_send(KIND_TRANSACTION, handle, code, flags | (object_count << 16), (ulong)id, data,
                    size) != 0) {
        awaiting_kind = 0;
        awaiting_id = 0;
        unlock_broker();
        free(composed);
        return -1;
    }
    free(composed);
    if ((flags & 1) != 0) { /* oneway owes no answer */
        awaiting_kind = 0;
        awaiting_id = 0;
        *object_count_out = 0;
        unlock_broker();
        return 0;
    }
    unsigned char *answer = 0;
    ulong answer_size = 0;
    uint32 a = 0, b = 0, c = 0;
    int waited = await_response(KIND_REPLY, id, &answer, &answer_size, &a, &b, &c);
    awaiting_kind = 0;
    awaiting_id = 0;
    unlock_broker();
    if (waited != 0) return -1;
    *status = (uint32)a;
    *reply = answer;
    *reply_size = answer_size;
    *object_count_out = c;
    return 0;
}

/* Run one IServiceManager transaction, writing the answer into `reply`.
 *
 * An absent thing should look absent: a call the framework makes about hardware
 * or a module it does not have is not a failure, so it gets a valid, empty
 * answer rather than an error. Answering with an error made PowerStatsService
 * throw a SecurityException out of onStart. */
static int service_manager(uint32 code, const unsigned char *data, ulong size, void *request_parcel,
                           void *reply) {
    resolve();
    trace_num("smcode", (long)code, (long)size);
    if (!parcel_write_int32) return 0;

    /* The two transactions *every* binder object answers, and the reason two
     * streams a boot could not decode were not garbage at all. From IBinder.h,
     * where the codes are B_PACK_CHARS:
     *
     *   PING_TRANSACTION      0x5f504e47  "are you alive?"  an empty reply is yes
     *   INTERFACE_TRANSACTION 0x5f4e5446  "what are you?"   the interface token
     *
     * Neither carries an interface token, which is why the token check below
     * rejected them, and both are the protocol rather than any interface: the
     * first is what ProcessState::getStrongProxyForHandle sends to a handle
     * before it hands out a proxy for it, and the second is what a generated
     * proxy asks before it will use the object it holds. Answering the second
     * with nothing makes the proxy report UNKNOWN_TRANSACTION, and the error
     * path generated for that is what crashed the battery statistics thread
     * with the interface descriptor read out of a null. */
    if (code == TRANSACTION_PING) {
        say("android-binder: ping (alive)\n");
        say_once();
        return 1;
    }
    if (code == TRANSACTION_INTERFACE) {
        write_string16_into(reply, SERVICE_MANAGER_TOKEN);
        say("android-binder: interface is ");
        say(SERVICE_MANAGER_TOKEN);
        say("\n");
        say_once();
        return 1;
    }

    if (!after_token(data, size, SERVICE_MANAGER_TOKEN)) return 0;

    char name[NAME_MAX];
    int have_name = data && service_name_of(data, size, name, NAME_MAX);
    {
        static int dumped_requests = 0;
        if (dumped_requests < 3 && data && size) {
            dumped_requests++;
            say("android-binder: request code ");
            say_dec((long)code);
            say(" [");
            for (ulong i = 0; i < size && i < 120; i++) {
                static const char hex[] = "0123456789abcdef";
                char pair[3];
                pair[0] = hex[(data[i] >> 4) & 0xf];
                pair[1] = hex[data[i] & 0xf];
                pair[2] = ' ';
                write(2, pair, 3);
            }
            say("] size=");
            say_dec((long)size);
            say(" name=\"");
            say(name);
            say("\"\n");
        }
    }
    if (!have_name) {
        static int unreadable_dumps = 0;
        if (unreadable_dumps < 3 && data) {
            unreadable_dumps++;
            say("android-binder: code ");
            say_dec((long)code);
            say(" request is unreadable [");
            for (ulong i = 0; i < size && i < 48; i++) {
                static const char hex[] = "0123456789abcdef";
                char pair[3];
                pair[0] = hex[(data[i] >> 4) & 0xf];
                pair[1] = hex[data[i] & 0xf];
                pair[2] = ' ';
                write(2, pair, 3);
            }
            say("] size=");
            say_dec((long)size);
            say("\n");
        }
    }

    if (code == TRANSACTION_ADD_SERVICE) {
        if (have_name) {
            const unsigned char *after_name = name_after_token(data, size, name, NAME_MAX);
            unsigned long weakrefs = 0;
            void *object = object_at(request_parcel, data, size, after_name, &weakrefs);
            if (object) {
                remember(name, object, weakrefs);
                trace_pair("add", name);
                say("android-binder: registered ");
                say(name);
                say("\n");
                say_once();
                /* The 48 bytes around the object, so its layout can be read
                 * rather than assumed: three guesses at where it starts have been
                 * wrong, and the one that matched the type word still hands back
                 * something the reader cannot call. */
                static int shown = 0;
                if (shown < 2 && after_name) {
                    shown++;
                    const unsigned char *from = after_name - 8 < data ? data : after_name - 8;
                    const unsigned char *end = data + size;
                    say("android-binder:   around the object:");
                    for (const unsigned char *q = from; q < end && q < after_name + 40; q++) {
                        static const char hex[] = "0123456789abcdef";
                        char pair[3];
                        pair[0] = hex[(*q >> 4) & 0xf];
                        pair[1] = hex[*q & 0xf];
                        pair[2] = ' ';
                        write(2, pair, 3);
                    }
                    say("\n");
                    say_once();
                }
            } else {
                say("android-binder: addService ");
                say(name);
                say(" carried no readable binder object\n");
                say_once();
                /* Where is the object, then? The bytes after the name, the object
                 * table, and what each candidate position holds. Bounded to the
                 * first three so a boot does not fill with this. */
                static int dumped = 0;
                if (dumped < 3) {
                    dumped++;
                    const unsigned char *end = data + size;
                    ulong count = parcel_ipc_objects_count ? parcel_ipc_objects_count(request_parcel) : 0;
                    const unsigned long *offsets = parcel_ipc_objects ? parcel_ipc_objects(request_parcel) : 0;
                    say("android-binder:   size=");
                    say_dec((long)size);
                    say(" after-name at ");
                    say_dec(after_name ? (long)(after_name - data) : -1);
                    say(" objects=");
                    say_dec((long)count);
                    if (offsets && count) {
                        say(" first=");
                        say_dec((long)offsets[0]);
                    }
                    say(" bytes:");
                    for (const unsigned char *q = after_name; q && q < end && q < after_name + 40; q++) {
                        static const char hex[] = "0123456789abcdef";
                        char pair[3];
                        pair[0] = hex[(*q >> 4) & 0xf];
                        pair[1] = hex[*q & 0xf];
                        pair[2] = ' ';
                        write(2, pair, 3);
                    }
                    say("\n");
                    say_once();
                }
            }
        }
        parcel_write_int32(reply, 0);
        return 1;
    }

    if (code == TRANSACTION_GET_SERVICE || code == TRANSACTION_CHECK_SERVICE) {
        void *object = have_name ? lookup(name) : 0;
        uint32 remote = NO_HANDLE;
        if (!object && have_name) {
            /* Not this process's. The broker is the one authority over names more
             * than one process can see, and what it answers with is a handle in
             * *this* process's table -- so the caller gets something it can call,
             * and every transaction on it comes back here to be forwarded.
             *
             * Recording the handle and answering "absent", which this did, is the
             * difference between a service another process published being
             * reachable and not. `suspend_control` is one of them: the framework
             * asks for it, is told nothing, and libandroid_servers aborts on the
             * null it was handed. */
            ulong node = 0;
            uint32 owner = 0;
            remote = broker_lookup(name, &node, &owner);
        }
        /* A lookup that finds something is worth a line every time; one that
         * finds nothing is asked for every service the framework does not have,
         * over and over, and would bury everything else. */
        static int misses = 0;
        if (object || remote != NO_HANDLE || (misses++ % 20) == 0) {
            say("android-binder: ");
            say(code == TRANSACTION_GET_SERVICE ? "getService " : "checkService ");
            say(have_name ? name : "(unreadable)");
            if (object) {
                say(" found\n");
            } else if (remote != NO_HANDLE) {
                say(" found in the broker, handle ");
                say_dec((long)remote);
                say("\n");
            } else {
                say(" not found\n");
            }
            say_once();
        }
        trace_pair(object ? "found" : (remote != NO_HANDLE ? "remote" : "miss"),
                   have_name ? name : "(unreadable)");
        parcel_write_int32(reply, 0);
        /* Three answers, and all three are an object word: a local one, a handle
         * for one another process owns, or the null libbinder writes when there
         * is none. Writing *nothing* for the null is what made a lookup the shim
         * answered "not found" arrive as a failed transaction instead. */
        int wrote = 0;
        if (object) {
            say("android-binder: handing back ");
            say(have_name ? name : "(unnamed)");
            say(" as 0x");
            for (int shift = 60; shift >= 0; shift -= 4) {
                static const char hex[] = "0123456789abcdef";
                char digit[2];
                digit[0] = hex[((unsigned long)object >> shift) & 0xf];
                digit[1] = 0;
                say(digit);
            }
            /* The object has to still be one. A service the framework registers
             * from a temporary -- `platform_compat` is one -- loses the Java wrapper
             * the native pointer belongs to when the temporary is collected, and the
             * pointer this registry kept then dangles: its first word stops being a
             * vtable. Handing that to libbinder is the SIGSEGV inside
             * Parcel::flattenBinder that stopped StartActivityManager, so it is
             * checked here and an object that no longer looks alive is answered
             * absent rather than passed on. */
            int alive = looks_like_ibinder(object);
            if (mosaic_alloc_trace_mark) mosaic_alloc_trace_mark("handback-local", object);
            say("\n");
            say_once();
            if (!alive) {
                say("android-binder:   ");
                say(have_name ? name : "(unnamed)");
                say(" no longer looks like an object; answering absent\n");
                say_once();
            } else if (parcel_write_binder) {
                /* The pointer in the first word: the layout that hands libbinder
                 * an object rather than a null. */
                unsigned long value[2] = {(unsigned long)object, 0};
                wrote = parcel_write_binder(reply, value);
                static int shown = 0;
                if (shown < 12) {
                    shown++;
                    say("android-binder:   writeStrongBinder(");
                    say(have_name ? name : "?");
                    say(") -> ");
                    say_dec((long)wrote);
                    say("\n");
                    say_once();
                }
            }
        } else if (remote != NO_HANDLE) {
            if (mosaic_alloc_trace_mark) mosaic_alloc_trace_mark("handback-handle", (const void *)(ulong)remote);
            write_handle_into(reply, remote, have_name ? name : 0);
            wrote = 1;
            say("android-binder: handing back ");
            say(have_name ? name : "(unnamed)");
            say(" as a handle to the broker's object\n");
            say_once();
        }
        if (!wrote) write_null_into(reply);
        return 1;
    }

    /* Every other IServiceManager call gets a valid, empty answer: a String[] is
     * a count followed by the strings, a boolean is an int32, and a void call
     * needs only the exception code. */
    if (code == TRANSACTION_LIST_SERVICES) {
        say("android-binder: listServices (");
        say_dec(registered_count());
        say(" registered)\n");
        say_once();
        parcel_write_int32(reply, 0); /* exception */
        parcel_write_int32(reply, 0); /* an empty array */
        return 1;
    }
    if (code == TRANSACTION_IS_DECLARED) {
        /* Nothing declares interfaces on this device: the framework's HAL and
         * apex lookups ask here, and "not declared" is the true answer, not a
         * failure. */
        parcel_write_int32(reply, 0); /* exception */
        parcel_write_int32(reply, 0); /* false */
        return 1;
    }
    if (code == TRANSACTION_GET_DECLARED_INSTANCES) {
        parcel_write_int32(reply, 0); /* exception */
        parcel_write_int32(reply, 0); /* an empty String[] */
        return 1;
    }
    if (code == TRANSACTION_UPDATABLE_VIA_APEX) {
        parcel_write_int32(reply, 0);  /* exception */
        parcel_write_int32(reply, 0);  /* absent: the nullable string is null */
        return 1;
    }
    if (code == TRANSACTION_GET_CONNECTION_INFO) {
        parcel_write_int32(reply, 0); /* exception */
        parcel_write_int32(reply, 0); /* an empty ConnectionInfo[] */
        return 1;
    }
    if (code == TRANSACTION_GET_SERVICE_DEBUG_INFO) {
        parcel_write_int32(reply, 0); /* exception */
        parcel_write_int32(reply, 0); /* an empty ServiceDebugInfo[] */
        return 1;
    }
    if (code == TRANSACTION_REGISTER_FOR_NOTIFICATIONS
        || code == TRANSACTION_UNREGISTER_FOR_NOTIFICATIONS) {
        /* "No", and honestly so: nothing here can call back into the framework
         * when a name appears. A caller that needs to wait asks again instead --
         * ServiceManagerShim::waitForService retries once a second -- whereas
         * answering "yes" and never calling would leave it waiting on a callback
         * that does not come. */
        parcel_write_int32(reply, 0); /* exception */
        parcel_write_int32(reply, 0); /* false */
        return 1;
    }
    if (code == TRANSACTION_REGISTER_CLIENT_CALLBACK
        || code == TRANSACTION_TRY_UNREGISTER_SERVICE) {
        parcel_write_int32(reply, 0); /* exception; these two return nothing */
        return 1;
    }
    /* Anything else: an exception code and a zero, which is what a reader is
     * about to ask for. A lone exception code leaves every reader short of its
     * value and is read as NOT_ENOUGH_DATA. */
    parcel_write_int32(reply, 0);
    parcel_write_int32(reply, 0);
    say("android-binder: answered code ");
    say_dec((long)code);
    say(" with an empty reply\n");
    say_once();
    return 1;
}

/* The API door. libbinder exports IPCThreadState::transact, which everything
 * funnels through and which carries the handle as an argument -- the better hook
 * of the two, because BpBinder::transact keeps the handle as a member at an
 * offset that is not fixed through IBinder's virtual inheritance from RefBase.
 *
 * Only the service manager is answered here. A transaction to any other handle
 * is a service in another process, which is the broker's to carry. */
/* The buffers of a reply handed to libbinder as a *data reference*: freeing them
 * is all this does. The objects inside them are not released here, and that is
 * the point -- see `handle_transaction`. */
static void release_reply_buffers(void *parcel, const unsigned char *data, ulong data_size,
                                  const unsigned long *objects, ulong objects_size) {
    (void)parcel;
    (void)data_size;
    (void)objects_size;
    free((void *)data);
    free((void *)objects);
}

static int handle_transaction(int handle, uint32 code, const void *data, void *reply, uint32 flags) {
    (void)handle;
    (void)flags;
    resolve();
    trace_num("door", (long)code, (long)handle);

    if (!reply) {
        /* A oneway transaction owes no answer. */
        return 0;
    }
    if (!parcel_data || !parcel_data_size || !parcel_write_int32) return 0;

    /* The answer is built in a Parcel of this side's own, and the caller is given
     * a *data reference* to it -- which is the shape a driver's reply arrives in,
     * and the shape it has to be, because the two differ in who owns the objects
     * inside them. `Parcel::freeDataNoInit` calls the release function of a Parcel
     * whose data it does not own, and releases every object of one it does. Written
     * straight into the caller's Parcel (which owns its own data), the answer's
     * object was released by the caller's own Parcel once the call had returned --
     * under the reference the caller had just been handed. The framework's
     * `BpBinder` for a service the broker hosts died that way, its address was
     * reused, and the proxy built on it ended up pointing at itself
     * (`docs/remaining-work.md`, "What the object itself says").
     *
     * The Parcel is deliberately not destroyed: its object table is what holds the
     * object for the caller, the way a driver's node reference does on a device. It
     * is one small allocation per service-manager call. */
    void *own_reply = malloc(PARCEL_BYTES);
    if (!own_reply) return 0;
    parcel_ctor(own_reply);
    if (service_manager(code, parcel_data(data), parcel_data_size(data), (void *)data, own_reply)) {
        if (parcel_set_reference) {
            parcel_set_reference(
                reply, parcel_ipc_data(own_reply), parcel_ipc_data_size(own_reply),
                parcel_ipc_objects ? parcel_ipc_objects(own_reply) : 0,
                parcel_ipc_objects_count ? parcel_ipc_objects_count(own_reply) : 0,
                release_reply_buffers);
        }
        /* The reference reset the caller's read position; nothing more to do. */
        return 0;
    }
    parcel_dtor(own_reply);
    free(own_reply);

    {
        /* Not the service manager: an object reached through a handle. If it is
         * one the broker gave this process, the broker carries the call to
         * whoever owns it. */
        int answered = 0;
        /* Handle 0 is the service manager, which `service_manager` above is the
         * one to answer; a call that reached this point with handle 0 is one whose
         * interface token is not the service manager's, not a call to an object in
         * another process. Forwarding it as one sent the broker a transaction it
         * refused -- and a refused transaction used to close the connection. */
        if (handle != 0 && parcel_write_bytes) {
            unsigned char *answer = 0;
            ulong answer_size = 0;
            uint32 status = 0;
            uint32 object_count = 0;
            unsigned char refs[MAX_ARGUMENT_OBJECTS * 12];
            uint32 ref_count = 0;
            const unsigned long *argument_offsets =
                parcel_ipc_objects ? parcel_ipc_objects(data) : 0;
            ulong argument_count =
                parcel_ipc_objects_count ? parcel_ipc_objects_count(data) : 0;
            if (argument_offsets && argument_count > 0) {
                int collected = collect_argument_objects(parcel_data(data), parcel_data_size(data),
                                                        argument_offsets, argument_count, refs,
                                                        MAX_ARGUMENT_OBJECTS);
                if (collected > 0) ref_count = (uint32)collected;
            }
            if (broker_transact((uint32)handle, code, flags, parcel_data(data),
                                parcel_data_size(data), refs, ref_count, &answer, &answer_size,
                                &status, &object_count) == 0) {
                if (answer && answer_size) write_broker_answer(reply, answer, answer_size, object_count);
                say("android-binder: forwarded code ");
                say_dec((long)code);
                say(" to the broker\n");
                say_once();
                answered = 1;
            }
        }
        if (!answered) {
            /* Either not the service manager, or a request whose interface token
             * could not be found. Answering an empty, successful reply is what this
             * did before, and failing instead stopped the framework earlier than it
             * used to: a call it can read an empty answer from is better served than
             * one it sees fail. */
            say("android-binder: no local object for code ");
            say_dec((long)code);
            say("\n");
            say_once();
            if (parcel_write_int32) parcel_write_int32(reply, 0);
        }
    }
    /* Writing the answer straight into the caller's Parcel leaves its read
     * position at the end of what was written. A reply from a real driver arrives
     * through Parcel::ipcSetDataReference, which resets that position to zero
     * before the generated proxy reads it -- readException() first, then
     * readStrongBinder(). Without the same reset the reader starts past the data
     * and every hand-back reads as null, so a lookup the shim answers `found`
     * still reaches the framework as a ServiceNotFoundException. */
    if (parcel_set_position) parcel_set_position(reply, 0);
    return 0;
}

int _ZN7android14IPCThreadState7transactEiRKNS_6ParcelEPS1_j(
    void *self, int handle, uint32 code, const void *data, void *reply, uint32 flags) {
    (void)self;
    return handle_transaction(handle, code, data, reply, flags);
}

int _ZN7android8BpBinder8transactEjRKNS_6ParcelEPS1_j(
    void *self, uint32 code, const void *data, void *reply, uint32 flags) {
    /* The handle this proxy stands for. `BpBinder::transact` reads it at offset
     * 0x10 and hands it to IPCThreadState::transact -- read off its disassembly,
     * `mov 0x10(%r12),%esi` right before the call -- so this door needs the same
     * number to route the call to the right object.
     *
     * Assuming zero, which this did, made every proxy's call look like a service
     * manager call: the objects handed back for services the broker hosts were
     * reachable by name and then answered as if they were the service manager, so
     * a reply for `registerCallback` was an exception code and nothing else. */
    uint32 handle = 0;
    if (self) {
        __builtin_memcpy(&handle, (const char *)self + BINDER_HANDLE_OFFSET, 4);
        /* A proxy for the service manager is handle 0; anything else has to be a
         * plausible number, or this is not a BpBinder and the service manager is
         * the safer assumption. */
        if (handle != 0 && handle > 0xffff) handle = 0;
    }
    return handle_transaction((int)handle, code, data, reply, flags);
}

/* The NDK door.
 *
 * libbinder_ndk defines these as plain C functions and other libraries call
 * them, which matters: a preload cannot interpose a C++ method that libbinder
 * calls inside itself, because Android builds with -fno-semantic-interposition
 * and those calls bind locally. Registering has to *remember*: an earlier version
 * acknowledged and discarded, which left memtrack.proxy registered and
 * unreachable. An AIBinder is an android::IBinder, so the same registry serves
 * both doors. */
/* Registration has to keep the object alive, or it is answered absent later.
 *
 * `memtrack.proxy` is registered through this door and the caller's reference
 * goes away with the caller: the registry then holds a pointer into freed
 * memory, `looks_like_ibinder` sees that it is no longer an IBinder, and the
 * service is reported absent rather than handed over -- the one
 * "no longer looks like an object" line in a boot. AIBinder_incStrong is the
 * NDK's own way to take a reference. It is deliberately never released: the
 * registry owns the service from registration on, which is what the Java door's
 * readStrongBinder already does. */
typedef void (*inc_strong_fn)(void *);

static inc_strong_fn aidl_inc_strong = 0;

static void hold_aidl_reference(void *binder) {
    if (!aidl_inc_strong) {
        void *ndk = dlopen("libbinder_ndk.so", RTLD_NOW);
        if (!ndk) {
            say("android-binder: no libbinder_ndk.so to hold a reference with\n");
            say_once();
            return;
        }
        aidl_inc_strong = (inc_strong_fn)dlsym(ndk, "AIBinder_incStrong");
    }
    if (aidl_inc_strong) aidl_inc_strong(binder);
}

int AServiceManager_addService(void *binder, const char *instance) {
    if (!instance) return 0; /* STATUS_OK */
    if (binder && looks_like_ibinder(binder)) {
        /* Only take a reference on something that is still an IBinder. Calling
         * incStrong on a pointer whose object is already gone writes refcounts
         * into memory the allocator has handed to something else, and the object
         * is remembered either way: the check is what decides whether it is
         * remembered as a service or as absent. */
        hold_aidl_reference(binder);
        remember(instance, binder, 0);
    }
    say("android-binder: AIDL register ");
    say(instance);
    say(binder ? "\n" : " (null)\n");
    say_once();
    return 0; /* STATUS_OK */
}

void *AServiceManager_getService(const char *instance) {
    void *binder = instance ? lookup(instance) : 0;
    say("android-binder: AIDL lookup ");
    say(instance ? instance : "(null)");
    say(binder ? " found\n" : " not found\n");
    say_once();
    return binder;
}

/* Take an answer out of a Parcel, or out of the broker's reply, into the two
 * buffers the driver door hands to libbinder. Both are libbinder's format --
 * bytes plus the offsets of the binder objects among them -- and the framework
 * frees what it is given, so the answer is copied: the Parcel itself outlives
 * only this call. */
static int copy_out(const unsigned char *data, ulong size, const unsigned long *objects, ulong count,
                    unsigned char **out_data, ulong *out_size, unsigned long **out_objects,
                    ulong *out_objects_count) {
    unsigned char *copy = (unsigned char *)malloc(size ? size : 8);
    unsigned long *object_copy = 0;
    if (count) object_copy = (unsigned long *)malloc(count * 8);
    if (!copy || (count && !object_copy)) {
        free(copy);
        free(object_copy);
        return 0;
    }
    if (size) __builtin_memcpy(copy, data, size);
    if (count) __builtin_memcpy(object_copy, objects, count * 8);
    *out_data = copy;
    *out_size = size;
    *out_objects = object_copy;
    *out_objects_count = count;
    return 1;
}

/* The driver door's reply builder.
 *
 * The handle decides where the call goes, and the door has it: target.handle is
 * 0 for the service manager, which this shim answers itself, and anything else
 * is an object in another process, which is the broker's to carry -- the same
 * thing the API door does with the handle it is handed. Without this the driver
 * door answered every handle with EX_SERVICE_SPECIFIC, which is why a service
 * the broker hosts would be found by name and then fail on the first call: the
 * Java and AIDL paths reach the driver, not the API. */
int mosaic_binder_reply(uint32 handle, uint32 code, const unsigned char *request, ulong request_size,
                        const unsigned long *argument_offsets, ulong argument_count,
                        unsigned char **out_data, ulong *out_size,
                        unsigned long **out_objects, ulong *out_objects_count) {
    resolve();

    if (handle != 0) {
        unsigned char *answer = 0;
        ulong answer_size = 0;
        uint32 status = 0;
        uint32 object_count = 0;
        unsigned char refs[MAX_ARGUMENT_OBJECTS * 12];
        uint32 ref_count = 0;
        if (argument_offsets && argument_count > 0 && request) {
            int collected = collect_argument_objects(request, request_size, argument_offsets,
                                                    argument_count, refs, MAX_ARGUMENT_OBJECTS);
            if (collected > 0) ref_count = (uint32)collected;
        }
        if (broker_transact(handle, code, 0, request ? request : (const unsigned char *)"", request_size,
                            refs, ref_count, &answer, &answer_size, &status, &object_count) == 0) {
            ulong size = answer_data_size(answer_size, object_count);
            write_answer_fds(answer, size, object_count);
            return copy_out(answer, size, answer_objects(answer, size, object_count),
                            reply_object_count(object_count), out_data, out_size, out_objects,
                            out_objects_count);
        }
        /* Nobody answered, so the caller gets a failed transaction rather than an
         * empty one: EX_TRANSACTION_FAILED is what the framework reports as a
         * remote exception instead of reading nothing as a value. */
        int failed = -129;
        return copy_out((const unsigned char *)&failed, 4, 0, 0, out_data, out_size, out_objects,
                        out_objects_count);
    }

    if (!parcel_ctor || !parcel_write_int32 || !parcel_ipc_data) return 0;

    void *reply = malloc(PARCEL_BYTES);
    if (!reply) return 0;
    parcel_ctor(reply);

    service_manager(code, request, request_size, 0, reply);

    const unsigned char *data = parcel_ipc_data(reply);
    ulong size = parcel_ipc_data_size(reply);
    const unsigned long *objects = parcel_ipc_objects ? parcel_ipc_objects(reply) : 0;
    ulong count = parcel_ipc_objects_count ? parcel_ipc_objects_count(reply) : 0;

    int copied = copy_out(data, size, objects, count, out_data, out_size, out_objects,
                          out_objects_count);

    parcel_dtor(reply);
    free(reply);
    return copied;
}
