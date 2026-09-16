/* Answer the calls that want Android's cgroup model, which a desktop has not got.
 *
 * This is not about the nice limit, and it is not a crutch for a harness. The
 * framework tells libprocessgroup which scheduling group a thread belongs to --
 * Process.setThreadGroup, which lands in TaskProfiles::SetTaskProfiles and
 * set_sched_policy -- and pins threads to the CPUs it read out of
 * /sys/devices/system/cpu. On a device both of those write into cgroups and cpu
 * sets that init created. Here there is nothing to write to, and libprocessgroup
 * answers with:
 *
 *   java.lang.SecurityException: No permission to modify given thread 27270
 *     at android.os.Process.setThreadGroup(Native Method)
 *     at com.android.server.UiThread.run(UiThread.java:44)
 *
 * which stops the run. These entry points accept the calls and log them.
 *
 * What this does *not* do is map Android's scheduling groups onto a host without
 * cgroups -- nice values, ioprio, or something else. It answers, and the answer
 * is a lie the framework can live with. Doing it properly is real work, and it is
 * work the product needs, because scheduling groups are how Android keeps audio
 * and display threads ahead of background work.
 *
 * Built by tools/build-native.sh alongside the other Bionic artifacts.
 */

extern long write(int, const void *, unsigned long);

static unsigned long length(const char *s) {
    unsigned long n = 0;
    if (!s) return 0;
    while (s[n]) n++;
    return n;
}

static void say(const char *s) {
    if (s) write(2, s, length(s));
}


/* Log the first of each, which is the only record of what is being answered
 * rather than mapped -- there is no cgroup to look at afterwards. */
static void answered(const char *what) {
    static const char *seen[4];
    static int count = 0;
    for (int i = 0; i < count; i++) {
        if (seen[i] == what) return;
    }
    if (count < 4) seen[count++] = what;
    say("pretend-cgroups: answering ");
    say(what);
    say("; the host has no cgroups to write to\n");
}

/* The framework pins its threads to the CPUs the device reports; the mask is
 * empty or names CPUs this machine does not have, and the call fails. */
int sched_setaffinity(int pid, unsigned long cpusetsize, const void *mask) {
    (void)pid;
    (void)cpusetsize;
    (void)mask;
    answered("sched_setaffinity");
    return 0;
}

/* libprocessgroup's own entry point, which writes /dev/cpuctl/<group>/tasks. */
int set_sched_policy(int tid, int policy) {
    (void)tid;
    (void)policy;
    answered("set_sched_policy");
    return 0;
}

/* Process.setThreadGroup ends up here with a vector of profile names that needs
 * a cgroup the desktop does not have. The arguments are not touched, only the
 * answer. */
int _ZN12TaskProfiles15SetTaskProfilesEiRKNSt3__16vectorINS0_12basic_stringIcNS0_11char_traitsIcEENS0_9allocatorIcEEEENS5_IS7_EEEEb(int tid, const void *profiles, int use_fd_cache) {
    (void)tid;
    (void)profiles;
    (void)use_fd_cache;
    answered("TaskProfiles::SetTaskProfiles");
    return 1 /* true */;
}
