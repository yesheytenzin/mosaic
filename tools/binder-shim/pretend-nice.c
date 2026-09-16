/* Pretend the thread-priority calls succeed.
 *
 * The Android framework raises its own thread priorities during startup --
 * android.os.Process.setThreadPriority with a negative value -- and an
 * unprivileged process may only lower its nice value within RLIMIT_NICE. On a
 * desktop RLIMIT_NICE is 0, and raising the hard limit needs privilege in the
 * initial user namespace, so not even root in a user namespace can do it here.
 * The framework therefore throws:
 *
 *   java.lang.SecurityException: No permission to set the priority of <tid>
 *     at android.os.Process.setThreadPriority(Native Method)
 *     at com.android.server.SystemServer.run(SystemServer.java:858)
 *
 * This preload accepts those calls and logs them, so a harness run can get past
 * a capability the harness cannot have. It is not a fix: the product needs the
 * broker's unit to carry LimitNICE (systemd) or an equivalent privileged step,
 * which is the shape of ADR-0008.
 *
 * Built by tools/build-native.sh alongside the other Bionic artifacts.
 */

extern long write(int, const void *, unsigned long);
extern void *dlsym(void *, const char *);

#define RTLD_NEXT ((void *)-1L)

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

/* PRIO_PROCESS is 0; -2 is THREAD_PRIORITY_FOREGROUND. */
int setpriority(int which, int who, int priority) {
    if (priority < 0) {
        say("pretend-nice: setpriority(");
        say_dec(which);
        say(", ");
        say_dec(who);
        say(", ");
        say_dec(priority);
        say(") accepted; the product needs LimitNICE\n");
        return 0;
    }
    typedef int (*real_fn)(int, int, int);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "setpriority");
    return real ? real(which, who, priority) : 0;
}

int getpriority(int which, int who) {
    (void)which;
    (void)who;
    /* The nicest priority, so callers that clamp do not try to raise it. */
    return 20;
}

/* The framework also uses scheduler policies for its own threads. */
int sched_setscheduler(int pid, int policy, const void *param) {
    (void)pid;
    (void)param;
    if (policy != 0 /* SCHED_OTHER */) {
        say("pretend-nice: sched_setscheduler accepted\n");
        return 0;
    }
    typedef int (*real_fn)(int, int, const void *);
    static real_fn real;
    if (!real) real = (real_fn)dlsym(RTLD_NEXT, "sched_setscheduler");
    return real ? real(pid, policy, param) : 0;
}
