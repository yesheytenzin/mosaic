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
#define SYM_SET_REFERENCE "_ZN7android6Parcel19ipcSetDataReferenceEPKhmPKymPFvPS0_S2_mS4_mE"
#define SYM_BINDER_TRANSACT "_ZN7android7BBinder8transactEjRKNS_6ParcelEPS1_j"
extern int pthread_create(unsigned long *, const void *, void *(*)(void *), void *);

/* A Parcel is a few hundred bytes and its layout is libbinder's business, so it
 * is allocated with room to spare and initialised by its own constructor. */
#define PARCEL_BYTES 1024

#define TRANSACTION_GET_SERVICE 1
#define TRANSACTION_CHECK_SERVICE 2
#define TRANSACTION_ADD_SERVICE 3
#define TRANSACTION_LIST_SERVICES 4
#define TRANSACTION_IS_DECLARED 5

/* The object types libbinder actually writes, read out of Parcel::unflattenBinder:
 *
 *   0x73682a85  BINDER_TYPE_HANDLE -- a remote object
 *   0x73622a85  BINDER_TYPE_BINDER -- a local one
 *
 * The 0x73 this used to carry is the *old* ASCII encoding and matches nothing,
 * so every addService was read as "not a local binder" and remembered nothing --
 * which is why the framework's own services were never found. */
#define BINDER_TYPE_BINDER 0x73622a85u
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
typedef int (*parcel_position_fn)(const void *, ulong);
typedef ulong (*parcel_where_fn)(const void *);
/* readStrongBinder returns an sp<IBinder> by value. sp has a user-declared
 * destructor, so the Itanium ABI returns it through a hidden first pointer --
 * calling it as if it returned a pointer in rax is what made the first attempt
 * at this crash. */
typedef void (*parcel_read_fn)(void *out, const void *self);
typedef void (*parcel_set_reference_fn)(void *self, const unsigned char *data, ulong size,
                                        const ulong *objects, ulong count, void *release);
typedef int (*binder_transact_fn)(void *self, uint32 code, const void *data, void *reply,
                                  uint32 flags);

static write_int32_fn parcel_write_int32;
static write_binder_fn parcel_write_binder;
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

#define SYS_READ 0
#define SYS_CLOSE 3
#define SYS_SOCKET 41
#define SYS_CONNECT 42
#define SYS_NANOSLEEP 35
#define SYS_GETPID 39

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
static int broker_wanted = -1;

/* Talking to the broker is off unless asked for.
 *
 * It is written and it builds, and it is not finished: the first run published
 * nothing and the reason is not yet found. Because a lookup that misses locally
 * would then wait on a socket for every service the framework does not have --
 * hundreds of them, five seconds each -- leaving it on by default would turn an
 * unverified path into a broken boot. MOSAIC_BINDER_BROKER=1 turns it on. */
static int broker_enabled(void) {
    if (broker_wanted < 0) {
        const char *v = getenv("MOSAIC_BINDER_BROKER");
        broker_wanted = (v && v[0] == '1') ? 1 : 0;
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

    /* The caller holds the lock across the whole request and its answer, so
     * that nothing else can interleave a frame. Taking it here as well is a
     * deadlock, not a belt and braces: every caller had already taken it. */
    int result = write_all(fd, header, 32);
    if (result == 0 && size > 0) result = write_all(fd, data, size);
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
                       unsigned char *data, ulong *size) {
    int fd = broker_fd;
    if (fd < 0) return -1;
    unsigned char header[32];
    if (read_all(fd, header, 32) != 0) return -1;
    if (header[1] != MOSAIC_WIRE_VERSION) return -1;

    *kind = header[0];
    *a = get_u32(header + 4);
    *b = get_u32(header + 8);
    *c = get_u32(header + 12);
    *node = get_u64(header + 16);
    ulong length = get_u32(header + 24);
    if (get_u32(header + 28) != 0) return -1; /* descriptors are not used yet */
    if (length > MAX_BROKER_FRAME || length > *size) return -1;
    if (length > 0 && read_all(fd, data, length) != 0) return -1;
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
static void *object_at(void *parcel) {
    if (!parcel || !parcel_set_position || !parcel_read_strong) return 0;
    if (!parcel_ipc_objects || !parcel_ipc_objects_count) return 0;
    ulong count = parcel_ipc_objects_count(parcel);
    if (count == 0) return 0;
    const ulong *offsets = parcel_ipc_objects(parcel);
    if (!offsets || offsets[0] < 24) return 0;

    ulong saved = parcel_data_position ? parcel_data_position(parcel) : 0;
    if (parcel_set_position(parcel, offsets[0] - 24) != 0) {
        parcel_set_position(parcel, saved);
        return 0;
    }
    /* The sp lands here and is deliberately not destroyed: its reference is the
     * registry's. */
    unsigned long held[2] = {0, 0};
    parcel_read_strong(held, parcel);
    parcel_set_position(parcel, saved);
    return (void *)held[0];
}

static void broker_export(const char *name, ulong node);

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

static void *lookup(const char *name) {
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
            unlock_registry();
            return object;
        }
    }
    unlock_registry();
    return 0;
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

struct timespec {
    long tv_sec;
    long tv_nsec;
};

/* One request is outstanding at a time: this side makes them synchronously, and
 * the reader thread below fills the slot when the answer arrives. */
static int response_ready = 0;
static uint32 response_kind = 0;
static uint32 response_a = 0;
static uint32 response_b = 0;
static uint32 response_c = 0;
static ulong response_length = 0;
static ulong response_node = 0;
static unsigned char *response_data = 0;
static ulong response_capacity = 0;

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

/* Wait for the answer to whatever was just sent, and take it. */
static int await_response(uint32 want_kind, unsigned char **data, ulong *size, uint32 *a, uint32 *b, uint32 *c) {
    for (int i = 0; i < 5000; i++) { /* five seconds */
        if (__sync_bool_compare_and_swap(&response_ready, 1, 0)) {
            *data = response_data;
            *size = response_length;
            *a = response_a;
            *b = response_b;
            *c = response_c;
            (void)want_kind;
            return 0;
        }
        struct timespec step = {0, 1000000}; /* one millisecond */
        syscall(SYS_NANOSLEEP, &step, 0);
    }
    return -1;
}

/* Publish a name for a node this process owns. */
static void broker_export(const char *name, ulong node) {
    if (!name) return;
    start_reader();
    if (broker_fd < 0) return;
    lock_broker();
    if (broker_send(KIND_EXPORT, 0, 0, 0, node, (const unsigned char *)name, length(name)) == 0) {
        say("android-binder: published ");
        say(name);
        say("\n");
        say_once();
    }
    unlock_broker();
}

/* Ask the broker for a name. Returns a handle in this process's table, or
 * NO_HANDLE. */
static uint32 broker_lookup(const char *name, ulong *node, uint32 *owner) {
    if (!name) return NO_HANDLE;
    start_reader();
    if (broker_fd < 0) return NO_HANDLE;
    lock_broker();
    if (broker_send(KIND_LOOKUP, 0, 0, 0, 0, (const unsigned char *)name, length(name)) != 0) {
        unlock_broker();
        return NO_HANDLE;
    }
    say("broker-self-check: asked\n");
    unsigned char *data = 0;
    ulong size = 0;
    uint32 a = 0, b = 0, c = 0;
    int waited = await_response(KIND_FOUND, &data, &size, &a, &b, &c);
    unlock_broker();
    say("broker-self-check: the wait ended\n");
    if (waited != 0) return NO_HANDLE;
    if (node) *node = response_node;
    if (owner) *owner = b;
    return a;
}

/* Send a transaction to an object another process owns and take its answer. */
static int broker_transact(uint32 handle, uint32 code, uint32 flags, const unsigned char *data,
                           ulong size, unsigned char **reply, ulong *reply_size, uint32 *status);

/* Serve one transaction another process sent to an object here. */
static void broker_serve(uint32 from, ulong node, uint32 code, uint32 flags,
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
        if (broker_recv(&kind, &a, &b, &c, &node, response_data, &size) != 0) break;
        if (kind == KIND_INCOMING) {
            say("android-binder: the broker sent a transaction\n");
            say_once();
            broker_serve(a, node, b, c, response_data, size);
            continue;
        }
        if (kind == KIND_DEAD) continue;
        /* Anything else is an answer to something this side asked. */
        response_kind = kind;
        response_a = a;
        response_b = b;
        response_c = c;
        response_node = node;
        response_length = size;
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

/* Serve a transaction another process sent to an object here.
 *
 * The request arrives as the parcel bytes the sender wrote, so the object is
 * called the way a local call would call it: a Parcel is built over those bytes
 * and BBinder::transact is entered by symbol, which then dispatches to the
 * object's own onTransact. The answer is read back out the same way it was
 * written, objects and all. */
static void broker_serve(uint32 from, ulong node, uint32 code, uint32 flags,
                         const unsigned char *data, ulong size) {
    (void)from;
    void *object = object_for_node(node);
    int status = -1;
    unsigned char *out = 0;
    ulong out_size = 0;

    if (object && parcel_ctor && parcel_dtor && binder_transact) {
        void *request = malloc(PARCEL_BYTES);
        void *reply = malloc(PARCEL_BYTES);
        if (request && reply) {
            parcel_ctor(request);
            if (parcel_set_reference) {
                parcel_set_reference(request, data, size, 0, 0, 0);
                parcel_ctor(reply);
                status = binder_transact(object, code, request, reply, flags);
                const unsigned char *bytes = parcel_ipc_data ? parcel_ipc_data(reply) : 0;
                ulong length = parcel_ipc_data_size ? parcel_ipc_data_size(reply) : 0;
                if (bytes && length) {
                    out = (unsigned char *)malloc(length ? length : 8);
                    if (out) {
                        __builtin_memcpy(out, bytes, length);
                        out_size = length;
                    }
                }
                parcel_dtor(reply);
            }
            parcel_dtor(request);
        }
        free(reply);
        free(request);
    }
    say("android-binder: served node ");
    say_dec((long)node);
    say(" code ");
    say_dec((long)code);
    say(status == 0 ? " ok\n" : " failed\n");
    say_once();

    lock_broker();
    if (broker_fd >= 0) {
        broker_send(KIND_INCOMING_REPLY, (uint32)status, 0, 0, node, out, out_size);
    }
    unlock_broker();
    free(out);
}

/* Send a transaction to an object another process owns and wait for its answer. */
static int broker_transact(uint32 handle, uint32 code, uint32 flags, const unsigned char *data,
                           ulong size, unsigned char **reply, ulong *reply_size, uint32 *status) {
    start_reader();
    if (broker_fd < 0) return -1;
    lock_broker();
    if (broker_send(KIND_TRANSACTION, handle, code, flags, 0, data, size) != 0) {
        unlock_broker();
        return -1;
    }
    if ((flags & 1) != 0) { /* oneway owes no answer */
        unlock_broker();
        return 0;
    }
    unsigned char *answer = 0;
    ulong answer_size = 0;
    uint32 a = 0, b = 0, c = 0;
    int waited = await_response(KIND_REPLY, &answer, &answer_size, &a, &b, &c);
    unlock_broker();
    if (waited != 0) return -1;
    *status = (uint32)a;
    *reply = answer;
    *reply_size = answer_size;
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
    if (!parcel_write_int32) return 0;
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
            void *object = object_at(request_parcel);
            if (object) {
                remember(name, object, 0);
                say("android-binder: registered ");
                say(name);
                say("\n");
                say_once();
            } else {
                say("android-binder: addService ");
                say(name);
                say(" carried no readable binder object\n");
                say_once();
            }
        }
        parcel_write_int32(reply, 0);
        return 1;
    }

    if (code == TRANSACTION_GET_SERVICE || code == TRANSACTION_CHECK_SERVICE) {
        void *object = have_name ? lookup(name) : 0;
        if (!object && have_name) {
            /* Not this process's: another one may have published it. Until the
             * remote object can be handed back as a handle, this records what
             * the broker said and reports the service absent. */
            ulong node = 0;
            uint32 owner = 0;
            uint32 handle = broker_lookup(name, &node, &owner);
            if (handle != NO_HANDLE) {
                say("android-binder: ");
                say(name);
                say(" is in the broker (handle ");
                say_dec((long)handle);
                say(")\n");
                say_once();
            }
        }
        /* A lookup that finds something is worth a line every time; one that
         * finds nothing is asked for every service the framework does not have,
         * over and over, and would bury everything else. */
        static int misses = 0;
        if (object || (misses++ % 20) == 0) {
            say("android-binder: ");
            say(code == TRANSACTION_GET_SERVICE ? "getService " : "checkService ");
            say(have_name ? name : "(unreadable)");
            say(object ? " found\n" : " not found\n");
            say_once();
        }
        parcel_write_int32(reply, 0);
        /* A null object is written by writing nothing: libbinder's own
         * writeStrongBinder dereferences the pointer it is given, so handing it
         * null for a service that does not exist is a SIGSEGV inside the
         * framework's getService. A remembered object is never null -- the
         * registry holds a reference to it -- so this is only the absent case. */
        if (object && parcel_write_binder) {
            unsigned long value[2] = {(unsigned long)object, 0};
            parcel_write_binder(reply, value);
        }
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
        parcel_write_int32(reply, 0); /* exception */
        parcel_write_int32(reply, 0); /* false: nothing is declared */
        return 1;
    }
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
static int handle_transaction(int handle, uint32 code, const void *data, void *reply, uint32 flags) {
    (void)handle;
    (void)flags;
    resolve();

    if (!reply) {
        /* A oneway transaction owes no answer. */
        return 0;
    }
    if (!parcel_data || !parcel_data_size || !parcel_write_int32) return 0;
    if (!service_manager(code, parcel_data(data), parcel_data_size(data), (void *)data, reply)) {
        /* Not the service manager: an object reached through a handle. If it is
         * one the broker gave this process, the broker carries the call to
         * whoever owns it. */
        if (parcel_write_bytes) {
            unsigned char *answer = 0;
            ulong answer_size = 0;
            uint32 status = 0;
            if (broker_transact((uint32)handle, code, flags, parcel_data(data),
                                parcel_data_size(data), &answer, &answer_size, &status) == 0) {
                if (answer && answer_size) parcel_write_bytes(reply, answer, answer_size);
                say("android-binder: forwarded code ");
                say_dec((long)code);
                say(" to the broker\n");
                say_once();
                return 0;
            }
        }
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
    return 0;
}

int _ZN7android14IPCThreadState7transactEiRKNS_6ParcelEPS1_j(
    void *self, int handle, uint32 code, const void *data, void *reply, uint32 flags) {
    (void)self;
    return handle_transaction(handle, code, data, reply, flags);
}

int _ZN7android8BpBinder8transactEjRKNS_6ParcelEPS1_j(
    void *self, uint32 code, const void *data, void *reply, uint32 flags) {
    (void)self;
    return handle_transaction(0, code, data, reply, flags);
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
int AServiceManager_addService(void *binder, const char *instance) {
    if (!instance) return 0; /* STATUS_OK */
    if (binder) remember(instance, binder, 0);
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

/* The driver door's reply builder.
 *
 * The answer to a transaction is a parcel: some bytes, plus an array of offsets
 * for the binder objects inside them. Both are libbinder's format, so a Parcel is
 * constructed here and its own writers fill it; the image is then copied out,
 * because the framework frees what it is handed and the Parcel itself outlives
 * only this call. */
int mosaic_binder_reply(uint32 code, const unsigned char *request, ulong request_size,
                        unsigned char **out_data, ulong *out_size,
                        unsigned long **out_objects, ulong *out_objects_count) {
    resolve();
    if (!parcel_ctor || !parcel_write_int32 || !parcel_ipc_data) return 0;

    void *reply = malloc(PARCEL_BYTES);
    if (!reply) return 0;
    parcel_ctor(reply);

    service_manager(code, request, request_size, 0, reply);

    const unsigned char *data = parcel_ipc_data(reply);
    ulong size = parcel_ipc_data_size(reply);
    const unsigned long *objects = parcel_ipc_objects ? parcel_ipc_objects(reply) : 0;
    ulong count = parcel_ipc_objects_count ? parcel_ipc_objects_count(reply) : 0;

    unsigned char *copy = malloc(size ? size : 8);
    unsigned long *object_copy = 0;
    if (count) object_copy = (unsigned long *)malloc(count * 8);
    if (!copy || (count && !object_copy)) {
        free(copy);
        free(object_copy);
        parcel_dtor(reply);
        free(reply);
        return 0;
    }
    __builtin_memcpy(copy, data, size);
    if (count) __builtin_memcpy(object_copy, objects, count * 8);

    parcel_dtor(reply);
    free(reply);

    *out_data = copy;
    *out_size = size;
    *out_objects = object_copy;
    *out_objects_count = count;
    return 1;
}
