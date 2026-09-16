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
#define SYM_DATA "_ZNK7android6Parcel4dataEv"
#define SYM_DATA_SIZE "_ZNK7android6Parcel8dataSizeEv"
#define SYM_PARCEL_CTOR "_ZN7android6ParcelC1Ev"
#define SYM_PARCEL_DTOR "_ZN7android6ParcelD1Ev"
#define SYM_IPC_DATA "_ZNK7android6Parcel7ipcDataEv"
#define SYM_IPC_DATA_SIZE "_ZNK7android6Parcel11ipcDataSizeEv"
#define SYM_IPC_OBJECTS "_ZNK7android6Parcel10ipcObjectsEv"
#define SYM_IPC_OBJECTS_COUNT "_ZNK7android6Parcel15ipcObjectsCountEv"

/* A Parcel is a few hundred bytes and its layout is libbinder's business, so it
 * is allocated with room to spare and initialised by its own constructor. */
#define PARCEL_BYTES 1024

#define TRANSACTION_GET_SERVICE 1
#define TRANSACTION_CHECK_SERVICE 2
#define TRANSACTION_ADD_SERVICE 3
#define TRANSACTION_LIST_SERVICES 4
#define TRANSACTION_IS_DECLARED 5

/* From binder.h. */
#define BINDER_TYPE_BINDER 0x73 /* 's' */

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
typedef const unsigned char *(*parcel_data_fn)(const void *);
typedef ulong (*parcel_size_fn)(const void *);
typedef void (*parcel_ctor_fn)(void *);
typedef void (*parcel_dtor_fn)(void *);
typedef const unsigned long *(*parcel_objects_fn)(const void *);
typedef ulong (*parcel_count_fn)(const void *);

static write_int32_fn parcel_write_int32;
static write_binder_fn parcel_write_binder;
static parcel_data_fn parcel_data;
static parcel_size_fn parcel_data_size;
static parcel_ctor_fn parcel_ctor;
static parcel_dtor_fn parcel_dtor;
static parcel_data_fn parcel_ipc_data;
static parcel_size_fn parcel_ipc_data_size;
static parcel_objects_fn parcel_ipc_objects;
static parcel_count_fn parcel_ipc_objects_count;

static void resolve(void) {
    if (parcel_write_int32) return;
    void *binder = dlopen("libbinder.so", RTLD_NOW);
    if (!binder) {
        say("android-binder: cannot open libbinder.so\n");
        return;
    }
    parcel_write_int32 = (write_int32_fn)dlsym(binder, SYM_WRITE_INT32);
    parcel_write_binder = (write_binder_fn)dlsym(binder, SYM_WRITE_BINDER);
    parcel_data = (parcel_data_fn)dlsym(binder, SYM_DATA);
    parcel_data_size = (parcel_size_fn)dlsym(binder, SYM_DATA_SIZE);
    parcel_ctor = (parcel_ctor_fn)dlsym(binder, SYM_PARCEL_CTOR);
    parcel_dtor = (parcel_dtor_fn)dlsym(binder, SYM_PARCEL_DTOR);
    parcel_ipc_data = (parcel_data_fn)dlsym(binder, SYM_IPC_DATA);
    parcel_ipc_data_size = (parcel_size_fn)dlsym(binder, SYM_IPC_DATA_SIZE);
    parcel_ipc_objects = (parcel_objects_fn)dlsym(binder, SYM_IPC_OBJECTS);
    parcel_ipc_objects_count = (parcel_count_fn)dlsym(binder, SYM_IPC_OBJECTS_COUNT);
}

static uint32 u32_at(const unsigned char *p) {
    uint32 v;
    __builtin_memcpy(&v, p, 4);
    return v;
}

static ulong u64_at(const unsigned char *p) {
    ulong v;
    __builtin_memcpy(&v, p, 8);
    return v;
}

/* A Parcel string16 is a character count, the UTF-16 data, and -- when the count
 * is odd -- four bytes of padding, because writeString16 pads with an int32. */
static const unsigned char *skip_string16(const unsigned char *p, const unsigned char *end) {
    if (!p || p + 4 > end) return 0;
    uint32 chars = u32_at(p);
    const unsigned char *q = p + 4 + (ulong)chars * 2;
    if (q > end) return 0;
    if (chars & 1) q += 4;
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

static int service_name_of(const unsigned char *data, ulong size, char *out, int cap) {
    return name_after_token(data, size, out, cap) != 0;
}

static void remember(const char *name, void *object, unsigned long cookie) {
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
        service_count++;
    }
    unlock_registry();
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

/* Run one IServiceManager transaction, writing the answer into `reply`.
 *
 * An absent thing should look absent: a call the framework makes about hardware
 * or a module it does not have is not a failure, so it gets a valid, empty
 * answer rather than an error. Answering with an error made PowerStatsService
 * throw a SecurityException out of onStart. */
static int service_manager(uint32 code, const unsigned char *data, ulong size, void *reply) {
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
            const unsigned char *end = data + size;
            const unsigned char *p = name_after_token(data, size, name, NAME_MAX);
            if (p && p + 24 <= end) {
                uint32 type = u32_at(p);
                unsigned long cookie = u64_at(p + 16);
                if (type == BINDER_TYPE_BINDER) {
                    /* The object is the cookie for a local binder; the other
                     * field is its weak reference table, which is not an IBinder
                     * and must not be handed back as one. */
                    remember(name, (void *)cookie, cookie);
                    say("android-binder: registered ");
                    say(name);
                    say("\n");
                    say_once();
                } else {
                    say("android-binder: ");
                    say(name);
                    say(" is not a local binder (type ");
                    say_dec((long)type);
                    say(")\n");
                    say_once();
                }
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
        if (parcel_write_binder) {
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
    if (!service_manager(code, parcel_data(data), parcel_data_size(data), reply)) {
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

    service_manager(code, request, request_size, reply);

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
