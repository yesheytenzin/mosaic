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

/* setpriority is called for both positive and negative priorities on this path,
 * and the caller turns any failure into a SecurityException. */
int setpriority_always(int which, int who, int priority) {
    (void)which;
    (void)who;
    (void)priority;
    return 0;
}

/* The framework also sets scheduling parameters and affinity for its own
 * threads, which a desktop process is not permitted to do either. */
int pthread_setschedparam(void *thread, int policy, const void *param) {
    (void)thread;
    (void)policy;
    (void)param;
    return 0;
}

int sched_setaffinity(int pid, unsigned long cpusetsize, const void *mask) {
    (void)pid;
    (void)cpusetsize;
    (void)mask;
    return 0;
}

/* libcutils does the actual work, and does it in a way that a libc-level
 * interceptor does not see -- its own setpriority is a direct syscall. These are
 * the entry points libandroid_runtime calls, so they are the ones to answer. */
int androidSetThreadPriority(int tid, int priority) {
    (void)tid;
    (void)priority;
    return 0;
}

int set_sched_policy(int tid, int policy) {
    (void)tid;
    (void)policy;
    return 0;
}

/* Process.setThreadGroup ends up here, in libprocessgroup, with a vector of
 * profile names that needs a cgroup the desktop does not have. The arguments are
 * not touched, only the answer. */
int _ZN12TaskProfiles15SetTaskProfilesEiRKNSt3__16vectorINS0_12basic_stringIcNS0_11char_traitsIcEENS0_9allocatorIcEEEENS5_IS7_EEEEb(int tid, const void *profiles, int use_fd_cache) {
    (void)tid;
    (void)profiles;
    (void)use_fd_cache;
    return 1 /* true */;
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
