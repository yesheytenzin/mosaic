/* Binder at the API level, instead of at the driver protocol.
 *
 * Four attempts to decode what libbinder puts in BINDER_WRITE_READ's write buffer
 * all failed, and the last one showed the code field holding the ASCII "GNP_".
 * The protocol does not have to be decoded: libbinder exports the level above it,
 * with declared arguments.
 *
 *   android::BpBinder::transact(unsigned int code, Parcel const& data,
 *                               Parcel* reply, unsigned int flags)
 *
 * A preloaded definition wins for callers outside libbinder, and the framework
 * reaches binder through libandroid_runtime, so this is the call that matters and
 * it can be interposed.
 *
 * What it implements is the service registry, which is what the framework needs
 * first: `addService` remembers the local binder it is handed, and
 * `getService`/`checkService` hand the same one back. While the framework and its
 * services are one process, a transaction to such a binder is an ordinary C++
 * call and never comes through here at all, so there is nothing to route yet -- a
 * second process is what makes routing, reference counting and descriptor passing
 * necessary (ADR-0005's broker over its socket).
 *
 * Reading the arguments means reading the caller's parcel: `Parcel::data()` and
 * `dataSize()` are exported, and an AIDL parcel is [interface token][name] as
 * string16s followed by the arguments, so the name and the flat_binder_object are
 * parseable without libbinder's own readers, whose C++ return types (sp<IBinder>
 * by value) are awkward to call from C.
 */

typedef unsigned int uint32;

extern void *dlsym(void *, const char *);
extern void *dlopen(const char *, int);
extern long write(int, const void *, unsigned long);

#define RTLD_NOW 2

#define SYM_WRITE_INT32 "_ZN7android6Parcel10writeInt32Ei"
#define SYM_WRITE_BINDER "_ZN7android6Parcel17writeStrongBinderERKNS_2spINS_7IBinderEEE"
#define SYM_DATA "_ZNK7android6Parcel4dataEv"
#define SYM_DATA_SIZE "_ZNK7android6Parcel8dataSizeEv"

#define TRANSACTION_GET_SERVICE 1
#define TRANSACTION_CHECK_SERVICE 2
#define TRANSACTION_ADD_SERVICE 3
#define TRANSACTION_LIST_SERVICES 4
#define TRANSACTION_IS_DECLARED 5

/* From binder.h. */
#define BINDER_TYPE_BINDER 0x73 /* 's' */

#define MAX_SERVICES 64
#define NAME_MAX 64

typedef struct {
    char name[NAME_MAX];
    void *binder;
    unsigned long cookie;
} service_t;

static service_t services[MAX_SERVICES];
static int service_count = 0;

static unsigned long length(const char *s) {
    unsigned long n = 0;
    if (!s) return 0;
    while (s[n]) n++;
    return n;
}

static void say(const char *s) {
    if (s) write(2, s, length(s));
}

static void say_dec(long value) {
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
    write(2, out, (unsigned long)i);
}

typedef int (*write_int32_fn)(void *, int);
typedef int (*write_binder_fn)(void *, const void *);
typedef const unsigned char *(*parcel_data_fn)(const void *);
typedef unsigned long (*parcel_size_fn)(const void *);

static write_int32_fn parcel_write_int32;
static write_binder_fn parcel_write_binder;
static parcel_data_fn parcel_data;
static parcel_size_fn parcel_data_size;

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
}

static unsigned int u32_at(const unsigned char *p) {
    unsigned int v;
    __builtin_memcpy(&v, p, 4);
    return v;
}

/* A Parcel string16 is a character count, the UTF-16 data, and -- when the count
 * is odd -- four bytes of padding, because writeString16 pads with an int32. */
static const unsigned char *skip_string16(const unsigned char *p, const unsigned char *end) {
    if (!p || p + 4 > end) return 0;
    unsigned int chars = u32_at(p);
    const unsigned char *q = p + 4 + (unsigned long)chars * 2;
    if (q > end) return 0;
    if (chars & 1) q += 4;
    return q <= end ? q : 0;
}

static int read_string16(const unsigned char *p, const unsigned char *end, char *out, int cap) {
    if (!p || p + 4 > end) return 0;
    unsigned int chars = u32_at(p);
    if (p + 4 + (unsigned long)chars * 2 > end) return 0;
    int n = 0;
    for (unsigned int i = 0; i < chars && n < cap - 1; i++) {
        unsigned short c;
        __builtin_memcpy(&c, p + 4 + (unsigned long)i * 2, 2);
        out[n++] = (c < 128) ? (char)c : '?';
    }
    out[n] = 0;
    return 1;
}

/* The name is the second string16 an IServiceManager call carries, after the
 * interface token. */
static int service_name_of(const void *data, char *out, int cap) {
    if (!parcel_data || !parcel_data_size) return 0;
    const unsigned char *p = parcel_data(data);
    const unsigned char *end = p + parcel_data_size(data);
    p = skip_string16(p, end);
    return read_string16(p, end, out, cap);
}

static void remember(const char *name, void *binder, unsigned long cookie) {
    for (int i = 0; i < service_count; i++) {
        if (services[i].name[0] && length(services[i].name) == length(name)) {
            int same = 1;
            for (unsigned long k = 0; k < length(name); k++) {
                if (services[i].name[k] != name[k]) { same = 0; break; }
            }
            if (same) {
                services[i].binder = binder;
                services[i].cookie = cookie;
                return;
            }
        }
    }
    if (service_count >= MAX_SERVICES) return;
    int n = 0;
    while (name[n] && n < NAME_MAX - 1) {
        services[service_count].name[n] = name[n];
        n++;
    }
    services[service_count].name[n] = 0;
    services[service_count].binder = binder;
    services[service_count].cookie = cookie;
    service_count++;
}

static void *lookup(const char *name) {
    for (int i = 0; i < service_count; i++) {
        if (length(services[i].name) != length(name)) continue;
        int same = 1;
        for (unsigned long k = 0; k < length(name); k++) {
            if (services[i].name[k] != name[k]) { same = 0; break; }
        }
        if (same) return services[i].binder;
    }
    return 0;
}

/* BpBinder keeps the handle as a member, but through IBinder's virtual inheritance
 * from RefBase its offset is not simply after the first vtable pointer. Nothing
 * here depends on it yet: while the framework and its services share a process,
 * the only binder anyone can reach is the service manager. */
static int bp_handle(const void *self) {
    return *(const int *)((const char *)self + 8);
}

/* The same handler serves both entry points. IPCThreadState::transact is the one
 * everything funnels through, including the NDK's, and it carries the handle as an
 * argument -- which BpBinder::transact does not, where the handle is a member at an
 * offset this does not know. So this is both the wider and the easier hook. */
static int handle_transaction(int handle, uint32 code, const void *data, void *reply, uint32 flags);

int _ZN7android14IPCThreadState7transactEiRKNS_6ParcelEPS1_j(
    void *self, int handle, uint32 code, const void *data, void *reply, uint32 flags) {
    (void)self;
    return handle_transaction(handle, code, data, reply, flags);
}

int _ZN7android8BpBinder8transactEjRKNS_6ParcelEPS1_j(
    void *self, uint32 code, const void *data, void *reply, uint32 flags) {
    return handle_transaction(bp_handle(self), code, data, reply, flags);
}

static int handle_transaction(int handle, uint32 code, const void *data, void *reply, uint32 flags) {
    resolve();

    if (!reply) {
        /* A oneway transaction: accept it and let the caller continue. */
        if (code == TRANSACTION_ADD_SERVICE && data) {
            char name[NAME_MAX];
            if (service_name_of(data, name, NAME_MAX)) {
                say("android-binder: oneway addService ");
                say(name);
                say("\n");
            }
        }
        return 0;
    }

    if (code == TRANSACTION_ADD_SERVICE) {
        char name[NAME_MAX];
        if (data && service_name_of(data, name, NAME_MAX)) {
            const unsigned char *p = parcel_data(data);
            const unsigned char *end = p + parcel_data_size(data);
            p = skip_string16(p, end);          /* interface token */
            p = skip_string16(p, end);          /* the name */
            if (p && p + 24 <= end) {
                unsigned int type = u32_at(p);
                unsigned long binder = 0, cookie = 0;
                __builtin_memcpy(&binder, p + 8, 8);
                __builtin_memcpy(&cookie, p + 16, 8);
                if (type == BINDER_TYPE_BINDER) {
                    remember(name, (void *)binder, cookie);
                    say("android-binder: registered ");
                    say(name);
                    say("\n");
                } else {
                    say("android-binder: ");
                    say(name);
                    say(" is not a local binder (type ");
                    say_dec((long)type);
                    say(")\n");
                }
            }
        }
        if (parcel_write_int32) parcel_write_int32(reply, 0);
        return 0;
    }

    if (code == TRANSACTION_GET_SERVICE || code == TRANSACTION_CHECK_SERVICE) {
        char name[NAME_MAX];
        void *binder = 0;
        if (data && service_name_of(data, name, NAME_MAX)) {
            binder = lookup(name);
            say("android-binder: ");
            say(code == TRANSACTION_GET_SERVICE ? "getService " : "checkService ");
            say(name);
            say(binder ? " found\n" : " not found\n");
        }
        if (parcel_write_int32) parcel_write_int32(reply, 0);
        if (parcel_write_binder) {
            unsigned long value[2] = {(unsigned long)binder, 0};
            parcel_write_binder(reply, value);
        }
        return 0;
    }

    /* Every other IServiceManager call gets a valid, empty answer instead of an
     * error. The framework asks whether hardware it does not have is declared
     * (isDeclared), lists services it will not find, and asks about modules it
     * does not have; none of those is a failure, and answering with an error made
     * PowerStatsService throw a SecurityException out of onStart. An absent thing
     * should look absent.
     *
     * The replies are per the AIDL: a String[] is a count followed by the
     * strings, a boolean is an int32, and a void call needs only the exception
     * code. */
    if (parcel_write_int32) parcel_write_int32(reply, 0);
    if (code == TRANSACTION_LIST_SERVICES && parcel_write_int32) {
        parcel_write_int32(reply, 0); /* an empty array */
    } else if (code == TRANSACTION_IS_DECLARED && parcel_write_int32) {
        parcel_write_int32(reply, 0); /* false: nothing is declared */
    }
    say("android-binder: answered code ");
    say_dec((long)code);
    say(" with an empty reply\n");
    return 0;
}
