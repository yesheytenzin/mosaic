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
 * reaches binder through libandroid_runtime, so this is the first call that
 * matters and it can be interposed.
 *
 * What it answers: handle 0 is the service manager, and a call to it asking for a
 * service this system does not have is answered the way the service manager would
 * answer for an absent service -- a zero exception code and a null binder, which
 * is a valid reply that the generated AIDL proxy turns into a null return rather
 * than a failure.
 *
 * What it does not answer yet: anything else. Those return an error so callers
 * fail in Java where the framework reports them, rather than waiting forever for
 * a reply that will not come.
 *
 * This is the shape a real implementation grows into: the service registry and
 * the routes to real services belong behind this function, and the driver
 * protocol stays out of it.
 */

typedef unsigned int uint32;

extern void *dlsym(void *, const char *);
extern void *dlopen(const char *, int);
extern long write(int, const void *, unsigned long);

#define RTLD_NOW 2
#define RTLD_NEXT ((void *)-1L)

#define SYM_BP_TRANSACT "_ZN7android8BpBinder8transactEjRKNS_6ParcelEPS1_j"
#define SYM_WRITE_INT32 "_ZN7android6Parcel10writeInt32Ei"
#define SYM_WRITE_BINDER "_ZN7android6Parcel17writeStrongBinderERKNS_2spINS_7IBinderEEE"

/* The binder transaction codes the service manager understands. */
#define TRANSACTION_GET_SERVICE 1
#define TRANSACTION_CHECK_SERVICE 2
#define TRANSACTION_ADD_SERVICE 3

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

static write_int32_fn parcel_write_int32;
static write_binder_fn parcel_write_binder;

static void resolve(void) {
    if (parcel_write_int32) return;
    void *binder = dlopen("libbinder.so", RTLD_NOW);
    if (!binder) {
        say("android-binder: cannot open libbinder.so\n");
        return;
    }
    parcel_write_int32 = (write_int32_fn)dlsym(binder, SYM_WRITE_INT32);
    parcel_write_binder = (write_binder_fn)dlsym(binder, SYM_WRITE_BINDER);
    if (!parcel_write_int32 || !parcel_write_binder) {
        say("android-binder: Parcel writers not found\n");
    }
}

/* BpBinder keeps the handle as a member; the object starts with the vtables. */
static int bp_handle(const void *self) {
    return *(const int *)((const char *)self + 8);
}

/* The mangled name is the symbol libbinder exports, so this definition is what a
 * caller outside libbinder binds to. */
int _ZN7android8BpBinder8transactEjRKNS_6ParcelEPS1_j(
    void *self, uint32 code, const void *data, void *reply, uint32 flags) {
    resolve();

    /* BpBinder derives from IBinder which derives virtually from RefBase, so the
     * handle is not simply after the first vtable pointer. Show the candidates
     * for the first few calls, since the service manager's call is checkService
     * and its handle must be zero. */
    static int shown = 0;
    if (shown < 4) {
        shown++;
        const unsigned char *raw = (const unsigned char *)self;
        say("android-binder: candidates:");
        for (int off = 8; off <= 40; off += 4) {
            unsigned int v;
            __builtin_memcpy(&v, raw + off, 4);
            say(" ");
            say_dec(off);
            say("=");
            say_dec((long)v);
        }
        say("\n");
    }

    int handle = bp_handle(self);
    say("android-binder: transact handle=");
    say_dec(handle);
    say(" code=");
    say_dec((long)code);
    say(" flags=");
    say_dec((long)flags);
    say("\n");

    /* No service exists yet, so every target here is the service manager: the
     * only binder the framework can reach is handle 0. That avoids needing the
     * handle field, whose offset is not obvious through BpBinder's virtual
     * inheritance, until there is a second binder to tell it apart from. */
    (void)handle;

    /* Registering a service succeeds. The registry itself is the next step: the
     * framework is starting its own services and will look them up again, and
     * then the name-to-binder table has to exist. Answering addService is what
     * lets it get that far. */
    if (reply && code == TRANSACTION_ADD_SERVICE) {
        if (parcel_write_int32) parcel_write_int32(reply, 0);
        say("android-binder: accepted a service registration\n");
        return 0;
    }

    if (reply && (code == TRANSACTION_GET_SERVICE || code == TRANSACTION_CHECK_SERVICE)) {
        /* An absent service, as the service manager would report it. */
        if (parcel_write_int32) parcel_write_int32(reply, 0 /* no exception */);
        if (parcel_write_binder) {
            /* sp<IBinder> with a null pointer; the extra words are slack in case
             * this build's sp is wider than one pointer. */
            void *null_binder[2] = {0, 0};
            parcel_write_binder(reply, null_binder);
        }
        say("android-binder: answered \"no such service\"\n");
        return 0; /* NO_ERROR */
    }

    /* Everything else fails in Java rather than waiting for a reply. */
    return -1;
}
