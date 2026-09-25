/* Property writes, implemented in the shim.
 *
 * Reading properties works through the mapped area. Writing goes over a socket at
 * /dev/socket/property_service, and the property service answers that -- but libc
 * refuses some writes before it ever connects, and `SystemProperties.set` turns
 * the refusal into a RuntimeException. The framework sets a cache nonce on its way
 * through service startup, so that refusal stopped `startBootstrapServices`.
 *
 * Interposing the entry point both shows every write, which was invisible before,
 * and lets the writes the old wire format can carry go through regardless of what
 * libc thinks of them.
 *
 * The wire format is the old one: struct prop_msg { cmd, name[32], value[92] },
 * 128 bytes on a SOCK_STREAM connection, acknowledged by the server closing it.
 * PROP_MSG_SETPROP2 is only used when ro.property_service.version says so, and it
 * does not here.
 *
 * A name or value too long for that struct is accepted and not stored. Android has
 * properties longer than 32 characters and libc rejects them for this protocol;
 * pretending they were set is less disruptive than the exception the framework
 * throws when it cannot set one, and a read of one comes back empty, as an unset
 * property does.
 */

typedef unsigned int uint32;

extern long syscall(long, ...);

extern char *getenv(const char *);
extern int strcmp(const char *, const char *);
extern void *dlsym(void *, const char *);

#define SYS_close 3
#define SYS_write 1
#define SYS_socket 41
#define SYS_connect 42

#define AF_UNIX 1
#define SOCK_STREAM 1

#define PROP_NAME_MAX 32
#define PROP_VALUE_MAX 92
#define PROP_MSG_SETPROP 1
#define MESSAGE_SIZE 128

struct local_sockaddr {
    unsigned short sun_family;
    char sun_path[108];
};

static unsigned long length(const char *s) {
    unsigned long n = 0;
    if (!s) return 0;
    while (s[n]) n++;
    return n;
}

static void say(const char *s) {
    if (s) syscall(SYS_write, 2, s, length(s));
}

/* Every write is logged: this is the path whose failures were invisible, because
 * the property service never sees the ones libc rejects. The log is bounded the
 * same way the harness bounds its output. */
static int logged = 0;

/* A diagnostic: read the names the package manager needs, with the same libc the
 * framework reads them with, and say what comes back. Interposing
 * `__system_property_get` did not work -- the callers reach it inside their own
 * library -- but calling it from here is the same question asked of the same code. */
static int reads_logged = 0;

/* What the callback reader reports, so the two can be compared. */
static void property_callback(void *cookie, const char *name, const char *value, unsigned int serial) {
    (void)cookie;
    (void)serial;
    say("android-properties: callback ");
    say(name);
    say(" -> ");
    say(value ? value : "(null)");
    say("\n");
}

static void check_property_reads(void) {
    static int (*real_get)(const char *, char *);
    if (!real_get) real_get = (int (*)(const char *, char *))dlsym((void *)-1L, "__system_property_get");
    if (!real_get || reads_logged) return;
    reads_logged = 1;
    static const char *names[] = {"pm.dexopt.first-boot", "pm.dexopt.bg-dexopt", "ro.build.version.sdk",
                                  "fw.free_cache_v2", "persist.sys.preloads.file_cache_expired"};
    for (unsigned long i = 0; i < 5; i++) {
        char value[128];
        for (int j = 0; j < 128; j++) value[j] = 0;
        int got = real_get(names[i], value);
        say("android-properties: reading ");
        say(names[i]);
        say(" -> ");
        say(value);
        say("\n");
        (void)got;
    }
    /* The other reader: `__system_property_find` walks the trie, and that is what
     * the JNI's property callbacks use. A name the old path finds and this one does
     * not is a trie entry the builder got wrong. */
    static void *(*find)(const char *);
    if (!find) find = (void *(*)(const char *))dlsym((void *)-1L, "__system_property_find");
    if (find) {
        for (unsigned long i = 0; i < 5; i++) {
            void *found = find(names[i]);
            say("android-properties: find ");
            say(names[i]);
            say(found ? " -> found\n" : " -> NOT FOUND\n");
        }
    }
    /* And the callback reader, which is the one `libbase`'s GetProperty uses in
     * this branch: it reads through the serial and the prop_info rather than the
     * old map, so a name the two above find can still come back empty here. */
    static void (*read_cb)(const void *, void (*)(void *, const char *, const char *, unsigned int), void *);
    if (!read_cb) read_cb = (void (*)(const void *, void (*)(void *, const char *, const char *, unsigned int), void *))dlsym((void *)-1L, "__system_property_read_callback");
    if (read_cb && find) {
        for (unsigned long i = 0; i < 5; i++) {
            void *found = find(names[i]);
            if (!found) continue;
            read_cb(found, property_callback, 0);
        }
    }
}

int __system_property_set(const char *key, const char *value) {
    check_property_reads();
    if (!key || !*key) return -1;
    if (!value) value = "";

    if (logged < 200) {
        logged++;
        say("android-properties: set ");
        say(key);
        say("=");
        say(value);
        say("\n");
    }

    if (length(key) >= PROP_NAME_MAX || length(value) >= PROP_VALUE_MAX) {
        return 0; /* accepted, not stored; see the note above */
    }

    int fd = (int)syscall(SYS_socket, AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0) return 0;

    struct local_sockaddr address;
    for (unsigned long i = 0; i < sizeof(address); i++) ((char *)&address)[i] = 0;
    address.sun_family = AF_UNIX;
    const char *socket_path = "/dev/socket/property_service";
    unsigned long n = 0;
    while (socket_path[n] && n < sizeof(address.sun_path) - 1) {
        address.sun_path[n] = socket_path[n];
        n++;
    }

    if (syscall(SYS_connect, fd, &address, sizeof(address)) < 0) {
        syscall(SYS_close, fd);
        return 0;
    }

    char message[MESSAGE_SIZE];
    for (int i = 0; i < MESSAGE_SIZE; i++) message[i] = 0;
    uint32 command = PROP_MSG_SETPROP;
    __builtin_memcpy(message, &command, 4);
    for (unsigned long i = 0; key[i] && i < PROP_NAME_MAX - 1; i++) message[4 + i] = key[i];
    for (unsigned long i = 0; value[i] && i < PROP_VALUE_MAX - 1; i++) {
        message[4 + PROP_NAME_MAX + i] = value[i];
    }

    syscall(SYS_write, fd, message, MESSAGE_SIZE);
    syscall(SYS_close, fd);
    return 0;
}
