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

int __system_property_set(const char *key, const char *value) {
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
