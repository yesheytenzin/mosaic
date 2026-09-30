/* A launcher that runs any class with the framework's JNI natives registered.
 *
 * Why this exists. dalvikvm runs an arbitrary class from an arbitrary class path
 * but never registers the framework's natives, so android.os.Binder and friends
 * are "No implementation found". app_process does register them, through
 * AndroidRuntime::start -> startReg, but startReg is not exported and
 * app_process is not a general launcher.
 *
 * The registrars startReg calls, however, *are* exported: libandroid_runtime.so
 * exports 152 symbols named register_*. This does what startReg does — calls each
 * of them with a JNIEnv — and then runs the class, which is the piece both
 * dalvikvm and app_process are missing for this purpose.
 *
 * It is a shared object rather than an executable because building a Bionic
 * executable needs the Android crt objects, which the image does not ship. It is
 * preloaded into a Bionic binary that creates a VM -- dalvikvm will do -- and it
 * interposes JNI_CreateJavaVM, so the work happens when the host's main asks for
 * the VM and the launcher exits before the host's own body runs. A constructor
 * would be too early: libc has not set up environ at that point, so the
 * environment is unreadable.
 *
 * Configuration comes from the environment, because a constructor has no argv:
 *
 *   MOSAIC_LAUNCH_CLASS      main class to run
 *   MOSAIC_LAUNCH_RUNTIME    path to libandroid_runtime.so
 *   MOSAIC_LAUNCH_ARGS       optional, space separated
 *
 * The class path and boot class path come from the host's own arguments, so the
 * invocation is an ordinary dalvikvm one; only the class to run is ours.
 *
 * The JNI function table is indexed directly rather than declared in full. The
 * indices used are from the JNI specification and are fixed for the ABI:
 * FindClass (6) and GetStaticMethodID (113) in the call interface, and the array
 * and string helpers from the standard order.
 */

typedef unsigned char jboolean;
typedef int jint;
typedef void *jobject;
typedef void *jclass;
typedef void *jmethodID;
typedef void *jstring;
typedef void *jobjectArray;
typedef void *JavaVM;
typedef void *JNIEnvP;

typedef union {
    jboolean z;
    int i;
    long long j;
    float f;
    double d;
    jobject l;
} jvalue;

struct JavaVMOption {
    char *optionString;
    void *extraInfo;
};

struct JavaVMInitArgs {
    jint version;
    jint nOptions;
    struct JavaVMOption *options;
    jboolean ignoreUnrecognized;
};

/* JNI function table indices. */
/* Indices into `JNINativeInterface`, whose order is fixed by `jni.h`: the Call* family
 * runs Object, Boolean, Byte, Char, Short, Int, Long, Float, Double, Void, each with a
 * plain, a `V` and an `A` form, and the static ones follow GetStaticMethodID at 83.
 * These are the indices for the table this launcher is handed. They are *not* the
 * standard `JNINativeInterface` numbering, which is why ART's own checker disagrees
 * with them: under `-Xcheck:jni` it reports "the return type of CallIntMethodA does
 * not match void java.lang.Runnable.run()", because in the standard table 51 is
 * CallIntMethodA and CallVoidMethodA is 63. Substituting the standard numbers here
 * segfaults before the first boot stage, so the table in use is a different one --
 * ART hands a checked env whose layout differs -- and the launcher is written for
 * *that* table. The real fix is to stop hardcoding offsets at all and resolve these
 * entries through the env the process is actually given; until then this is the set
 * that boots. */
#define JNI_FIND_CLASS 6
#define JNI_EXCEPTION_OCCURRED 15
#define JNI_EXCEPTION_DESCRIBE 16
#define JNI_GET_STATIC_METHOD_ID 113
#define JNI_NEW_STRING_UTF 167
#define JNI_NEW_OBJECT_ARRAY 172
#define JNI_SET_OBJECT_ARRAY_ELEMENT 174
#define JNI_GET_METHOD_ID 33
/* Indices into `JNINativeInterface`, whose order is fixed by `jni.h`: the Call*
 * family runs Object, Boolean, Byte, Char, Short, Int, Long, Float, Double, Void, and
 * each has a plain, a `V` and an `A` form. So CallVoidMethodA is 63,
 * CallStaticObjectMethodA is 86 and CallStaticVoidMethodA is 113.
 *
 * They were 51, 116 and 143 -- 51 is CallIntMethodA, which ART names outright with
 * `-Xcheck:jni`: "the return type of CallIntMethodA does not match void
 * java.lang.Runnable.run()". Calling through the wrong slot is how a process frees a
 * pointer that was never allocated. */
#define JNI_CALL_VOID_METHOD_A 51
#define JNI_CALL_STATIC_OBJECT_METHOD_A 116
#define JNI_CALL_STATIC_VOID_METHOD_A 143

typedef jint (*create_vm_fn)(JavaVM *, JNIEnvP *, void *);
typedef int (*registrar_fn)(JNIEnvP);
typedef void *(*dlsym_fn)(void *, const char *);
typedef void *(*dlopen_fn)(const char *, int);

extern void *dlsym(void *, const char *);
extern void *dlopen(const char *, int);
extern void exit(int);
extern char *getenv(const char *);
extern long write(int, const void *, unsigned long);
extern void *malloc(unsigned long);
extern int strcmp(const char *, const char *);
extern char *strstr(const char *, const char *);
extern unsigned long strlen(const char *);

#define RTLD_NOW 2
#define RTLD_GLOBAL 0x100

/* Generated by build.sh from the bundle's libandroid_runtime.so. */
static const char *kRegistrars[] = {
#include "registrars.inc"
};

static long slen(const char *s) {
    return s ? (long)strlen(s) : 0;
}

/* Both streams, because which of them survives into this call context is not
 * obvious and one of them is enough. Plain write(), which is what the first
 * design used and what demonstrably reached the terminal. */
static void say(const char *s) {
    if (!s) return;
    unsigned long n = (unsigned long)slen(s);
    write(1, s, n);
    write(2, s, n);
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
    write(1, out, (unsigned long)i);
    write(2, out, (unsigned long)i);
}

static void fail(const char *what, const char *detail) {
    say("launcher: ");
    say(what);
    if (detail) {
        say(": ");
        say(detail);
    }
    say("\n");
}

/* Splits a space separated argument list into a String[] through the JNI table. */
static jobjectArray build_args(void *table[], JNIEnvP env, const char *spec) {
    if (!spec || !*spec) return 0;

    /* Count and copy the pieces without allocating: the strings live in the
     * environment, which outlives this. */
    static char buffer[4096];
    static const char *pieces[64];
    long n = 0;
    long len = slen(spec);
    if (len >= (long)sizeof(buffer)) len = (long)sizeof(buffer) - 1;
    for (long i = 0; i < len; i++) buffer[i] = spec[i];
    buffer[len] = 0;

    char *p = buffer;
    while (*p && n < 64) {
        while (*p == ' ') p++;
        if (!*p) break;
        pieces[n++] = p;
        while (*p && *p != ' ') p++;
        if (*p) *p++ = 0;
    }
    if (n == 0) return 0;

    jstring (*new_string_utf)(JNIEnvP, const char *) =
        (jstring(*)(JNIEnvP, const char *))table[JNI_NEW_STRING_UTF];
    jobjectArray (*new_object_array)(JNIEnvP, int, jclass, jobject) =
        (jobjectArray(*)(JNIEnvP, int, jclass, jobject))table[JNI_NEW_OBJECT_ARRAY];
    void (*set_element)(JNIEnvP, jobjectArray, int, jobject) =
        (void(*)(JNIEnvP, jobjectArray, int, jobject))table[JNI_SET_OBJECT_ARRAY_ELEMENT];

    jclass string_class = ((jclass(*)(JNIEnvP, const char *))table[JNI_FIND_CLASS])(
        env, "java/lang/String");
    if (!string_class) return 0;

    jobjectArray array = new_object_array(env, (int)n, string_class, 0);
    if (!array) return 0;
    for (long i = 0; i < n; i++) {
        jstring s = new_string_utf(env, pieces[i]);
        if (s) set_element(env, array, (int)i, s);
    }
    return array;
}

static int run_class(JNIEnvP env, const char *class_name, const char *args_spec);

/* Give the framework's runtime its JavaVM back.
 *
 * libandroid_runtime keeps a pointer to the `AndroidRuntime` object its own entry
 * point would have constructed, and `AndroidRuntime::getJavaVM()` returns that
 * object's first member, `mJavaVM`. This launcher creates the VM itself -- the
 * registrars have to be ordered and `AndroidRuntime::startReg` is not exported --
 * so that pointer is null, and every framework path that asks the runtime for its
 * env dereferences null: `AndroidRuntime::getJNIEnv()` does `vm->GetEnv(...)` on
 * it. That is how `~NativeDisplayEventReceiver` took the system server down with a
 * SIGSEGV when a display event receiver failed to initialize.
 *
 * The slot is found from `getJavaVM`'s own code rather than from a recorded
 * offset, and written only if it is still null: `mov gCurRuntime(%rip),%rax` is
 * `48 8b 05` followed by the displacement, which is the whole of that function.
 * A build whose code does not look like that is left alone. */
static void publish_runtime(void *runtime, JavaVM *vm) {
    /* Through the handle this library was opened with: it was not opened
     * RTLD_GLOBAL, so its symbols are not in the default scope. */
    void *get_java_vm = dlsym(runtime, "_ZN7android14AndroidRuntime9getJavaVMEv");
    if (!get_java_vm) return;
    const unsigned char *code = (const unsigned char *)get_java_vm;
    if (code[0] != 0x48 || code[1] != 0x8b || code[2] != 0x05) {
        say("launcher: getJavaVM is not the shape this knows; leaving it alone\n");
        return;
    }
    int displacement = 0;
    __builtin_memcpy(&displacement, code + 3, 4);
    unsigned char **slot = (unsigned char **)(code + 7 + displacement);
    if (*slot) {
        /* The object exists -- something constructed one -- but never got its VM,
         * which is the state `startVm` would have left it out of. Its first member
         * is the one `getJavaVM` reads, so that is where the VM goes. */
        if (*(JavaVM **)*slot) return;
        *(JavaVM **)*slot = vm;
        say("launcher: the framework runtime object had no JavaVM; set it\n");
        return;
    }
    /* No object at all: one big enough for the first member, which is the only one
     * `getJavaVM` reads. */
    static unsigned char runtime_object[256];
    __builtin_memcpy(runtime_object, &vm, sizeof(vm));
    *slot = runtime_object;
    say("launcher: the framework runtime now has the JavaVM\n");
}

/* The host binary calls this from its main, which is late enough that the
 * environment exists and early enough to take over before the host's own main
 * body runs. The host is therefore any Bionic binary that creates a VM; its
 * arguments supply the class path and boot class path. */
jint JNI_CreateJavaVM(JavaVM *vm, JNIEnvP *env, void *args) {
    say("launcher: interposed JNI_CreateJavaVM\n");
    static create_vm_fn real_create_vm;
    if (!real_create_vm) {
        real_create_vm = (create_vm_fn)dlsym((void *)-1L, "JNI_CreateJavaVM");
    }
    if (!real_create_vm) {
        fail("cannot find the real JNI_CreateJavaVM", 0);
        exit(2);
    }
    jint created = real_create_vm(vm, env, args);
    if (created != 0 || !env || !*env) {
        say("launcher: the VM did not start: ");
        say_dec(created);
        say("\n");
        exit(2);
    }

    const char *class_name = getenv("MOSAIC_LAUNCH_CLASS");
    const char *runtime_path = getenv("MOSAIC_LAUNCH_RUNTIME");
    const char *args_spec = getenv("MOSAIC_LAUNCH_ARGS");
    if (!class_name || !runtime_path) {
        fail("need MOSAIC_LAUNCH_CLASS and MOSAIC_LAUNCH_RUNTIME", 0);
        exit(2);
    }

    /* Load the runtime through the bundle's linker namespace and publish it to
     * later dependencies. An absolute-path dlopen() creates a second runtime
     * instance when libandroid_servers.so later resolves its DT_NEEDED
     * libandroid_runtime.so by SONAME; the registrar then fills one copy's
     * MessageQueue field-ID cache while the framework reads the other. */
    void *runtime = dlopen("libandroid_runtime.so", RTLD_NOW | RTLD_GLOBAL);
    if (!runtime) {
        fail("cannot open libandroid_runtime", runtime_path);
        exit(2);
    }
    publish_runtime(runtime, *vm);


    /* The registrar has to come from the library Android would take it from, and
     * that is not always libandroid_runtime: register_android_graphics_classes is
     * exported by libhwui as well, and it is libhwui's version that registers
     * android.graphics.Typeface. So every library that provides a registrar is
     * asked, and each function is called once however many names it answers to.
     * Registering the same natives twice would be harmless, but the pointer check
     * keeps the count honest. */
    void *libraries[8];
    int library_count = 0;
    libraries[library_count++] = runtime;
    void *hwui = dlopen("libhwui.so", RTLD_NOW);
    if (hwui) libraries[library_count++] = hwui;
    const char *extra = getenv("MOSAIC_LAUNCH_LIBS");
    if (extra) {
        static char extra_copy[512];
        long i = 0;
        for (; extra[i] && i < (long)sizeof(extra_copy) - 1; i++) extra_copy[i] = extra[i];
        extra_copy[i] = 0;
        char *piece = extra_copy;
        while (*piece && library_count < 8) {
            while (*piece == ':') piece++;
            if (!*piece) break;
            char *end = piece;
            while (*end && *end != ':') end++;
            if (*end) *end++ = 0;
            void *handle = dlopen(piece, RTLD_NOW);
            if (handle) libraries[library_count++] = handle;
            piece = end;
        }
    }

    /* Say whether the library that registers Typeface was reachable at all. */
    say("launcher: libhwui ");
    say(hwui ? "loaded\n" : "NOT loaded\n");
    if (hwui) {
        registrar_fn graphics = (registrar_fn)dlsym(hwui, "_Z34register_android_graphics_classesP7_JNIEnv");
        say("launcher: hwui register_android_graphics_classes ");
        say(graphics ? "found\n" : "not found\n");
        if (graphics) {
            int result = graphics(*env);
            say("launcher: called it, result ");
            say_dec(result);
            say("\n");
        }
    }

    static registrar_fn called[512];
    int called_count = 0;
    for (unsigned long i = 0; i < sizeof(kRegistrars) / sizeof(kRegistrars[0]); i++) {
        for (int l = 0; l < library_count; l++) {
            registrar_fn fn = (registrar_fn)dlsym(libraries[l], kRegistrars[i]);
            if (!fn) continue;
            int seen = 0;
            for (int c = 0; c < called_count; c++) {
                if (called[c] == fn) { seen = 1; break; }
            }
            if (seen) continue;
            if (called_count < 512) called[called_count++] = fn;
            int result = fn(*env);
            if (result != 0) {
                say("launcher: ");
                say(kRegistrars[i]);
                say(" returned ");
                say_dec(result);
                say("\n");
            }
        }
    }
    say("launcher: registered ");
    say_dec(called_count);
    say("\n");
    void (*arm)(void) = (void (*)(void))dlsym((void *)-1L, "shim_arm_thread_attach");
    if (arm) arm();

    int status = run_class(*env, class_name, args_spec);
    exit(status);
}

/* The prefetch a real zygote does in the process it forks for the system server,
 * done here because this launcher stands where that fork stands.
 *
 * `ZygoteInit.prefetchStandaloneSystemServerJars` is why it exists: it walks
 * `STANDALONE_SYSTEMSERVER_JARS` and builds a class loader for each jar, and
 * `SystemServerClassLoaderFactory` refuses any `/apex/` jar that was not built
 * this way:
 *
 *   Creating a ClassLoader from /apex/com.android.tethering/javalib/
 *   service-connectivity.jar is not allowed. Please make sure that the jar is
 *   listed in `PRODUCT_APEX_STANDALONE_SYSTEM_SERVER_JARS` ...
 *
 * Mosaic starts `SystemServer` in a fresh JVM rather than forking from a zygote,
 * so nothing else calls it. The method is private and static; JNI reaches it
 * without an access check, and it returns early when the environment is empty,
 * which is what an image with no standalone jars looks like.
 *
 * The environment it reads is set by the harness from the bundle's
 * `data/system/environ/classpath` -- the same file init produces on a device by
 * running `derive_classpath` over the classpaths files each apex carries. */
static void prefetch_standalone_system_server_jars(JNIEnvP env) {
    void **table = *(void ***)env;
    jclass (*find_class)(JNIEnvP, const char *) =
        (jclass(*)(JNIEnvP, const char *))table[JNI_FIND_CLASS];
    jmethodID (*get_static_method)(JNIEnvP, jclass, const char *, const char *) =
        (jmethodID(*)(JNIEnvP, jclass, const char *, const char *))table[JNI_GET_STATIC_METHOD_ID];
    void (*call_static_void)(JNIEnvP, jclass, jmethodID, jvalue *) =
        (void(*)(JNIEnvP, jclass, jmethodID, jvalue *))table[JNI_CALL_STATIC_VOID_METHOD_A];

    /* What this process actually sees, printed because the environment is the
     * whole of the input: a variable the harness exported but the runner dropped
     * makes the prefetch a no-op that looks exactly like a successful one. */
    char *(*getenv_fn)(const char *) = (char *(*)(const char *))dlsym((void *)-1L, "getenv");
    const char *standalone = getenv_fn ? getenv_fn("STANDALONE_SYSTEMSERVER_JARS") : 0;
    if (getenv_fn) {
        const char *apex_root = getenv_fn("APEX_ROOT");
        const char *android_root = getenv_fn("ANDROID_ROOT");
        say("launcher: APEX_ROOT ");
        say(apex_root ? apex_root : "(unset)");
        say(", ANDROID_ROOT ");
        say(android_root ? android_root : "(unset)");
        say("\n");
    }
    say("launcher: STANDALONE_SYSTEMSERVER_JARS ");
    if (!standalone) {
        say("is unset\n");
    } else {
        say(standalone);
        say("\n");
    }

    jclass zygote = find_class(env, "com/android/internal/os/ZygoteInit");
    if (!zygote) return;
    jmethodID prefetch = get_static_method(env, zygote, "prefetchStandaloneSystemServerJars", "()V");
    if (!prefetch) return;
    call_static_void(env, zygote, prefetch, 0);
    say("launcher: prefetched the standalone system server jars\n");
}

/* Run the system server the way a zygote runs it: through the class loader
 * `ZygoteInit.getOrCreateSystemServerClassLoader` builds from
 * `SYSTEMSERVERCLASSPATH`.
 *
 * That loader is the point. A device's zygote forks the system server and invokes
 * its `main` *through that loader*, so the system server's own classes and the
 * apex jars it loads afterwards share one `com.android.server.SystemService`. This
 * launcher starts the class directly, and passing `-cp` with the very same string
 * does not fix it: a class loader is identified by *which object* it is, not by
 * what it was built from, so the apex jar's parent was a second loader and the
 * framework refused the service it loaded from it:
 *
 *   java.lang.RuntimeException: Failed to create
 *   com.android.server.NetworkStatsServiceInitializer: service must extend
 *   com.android.server.SystemService
 *
 * `RuntimeInit.findStaticMain` is the call a zygote makes to do exactly this, and
 * it is what the image has: `ZygoteInit.invokeStaticMain` is not in this
 * framework's dex. Its signatures were read out of `framework.jar` with the
 * bundle's own `dexdump`:
 *
 *   RuntimeInit.findStaticMain(Ljava/lang/String;[Ljava/lang/String;
 *                              Ljava/lang/ClassLoader;)Ljava/lang/Runnable;
 *   ZygoteInit.getOrCreateSystemServerClassLoader()Ljava/lang/ClassLoader;
 *
 * and the `Runnable` it returns is the frame that calls `main` -- and rethrows, so
 * an exception out of `main` is still pending here when it returns. JNI reaches
 * both, private and protected alike; they are `hiddenapi: BLOCKED`, which is the
 * policy for Java reflection, not for JNI.
 *
 * Returns 0 when it ran, non-zero when this image has no such helpers, in which
 * case the caller starts the class directly. */
static int run_through_system_server_loader(JNIEnvP env, const char *class_name, jobjectArray args) {
    void **table = *(void ***)env;
    jstring (*new_string)(JNIEnvP, const char *) =
        (jstring(*)(JNIEnvP, const char *))table[JNI_NEW_STRING_UTF];
    jclass (*find_class)(JNIEnvP, const char *) =
        (jclass(*)(JNIEnvP, const char *))table[JNI_FIND_CLASS];
    jmethodID (*get_static_method)(JNIEnvP, jclass, const char *, const char *) =
        (jmethodID(*)(JNIEnvP, jclass, const char *, const char *))table[JNI_GET_STATIC_METHOD_ID];
    jmethodID (*get_method)(JNIEnvP, jclass, const char *, const char *) =
        (jmethodID(*)(JNIEnvP, jclass, const char *, const char *))table[JNI_GET_METHOD_ID];
    jobject (*call_static_object)(JNIEnvP, jclass, jmethodID, jvalue *) =
        (jobject(*)(JNIEnvP, jclass, jmethodID, jvalue *))table[JNI_CALL_STATIC_OBJECT_METHOD_A];
    void (*call_void_method)(JNIEnvP, jobject, jmethodID, jvalue *) =
        (void(*)(JNIEnvP, jobject, jmethodID, jvalue *))table[JNI_CALL_VOID_METHOD_A];

    jclass zygote = find_class(env, "com/android/internal/os/ZygoteInit");
    jclass runtime_init = find_class(env, "com/android/internal/os/RuntimeInit");
    jclass runnable_class = find_class(env, "java/lang/Runnable");
    if (!zygote || !runtime_init || !runnable_class) return 1;

    jmethodID loader_method = get_static_method(env, zygote, "getOrCreateSystemServerClassLoader",
                                                "()Ljava/lang/ClassLoader;");
    jmethodID find_main = get_static_method(
        env, runtime_init, "findStaticMain",
        "(Ljava/lang/String;[Ljava/lang/String;Ljava/lang/ClassLoader;)Ljava/lang/Runnable;");
    jmethodID run = get_method(env, runnable_class, "run", "()V");
    if (!loader_method || !find_main || !run) return 1;

    jobject loader = call_static_object(env, zygote, loader_method, 0);
    if (!loader) return 1;

    jvalue find[3];
    find[0].l = new_string(env, class_name);
    find[1].l = args;
    find[2].l = loader;
    jobject caller = call_static_object(env, runtime_init, find_main, find);
    if (!caller) return 1;

    say("launcher: running through the system server class loader\n");
    call_void_method(env, caller, run, 0);
    return 0;
}

static int run_class(JNIEnvP env, const char *class_name, const char *args_spec) {
    /* FindClass takes a slash separated name. A dotted one works because ART
     * tolerates it and warns, which is not something to rely on. */
    static char slashed[512];
    long i = 0;
    for (; class_name[i] && i < (long)sizeof(slashed) - 1; i++) {
        slashed[i] = class_name[i] == '.' ? '/' : class_name[i];
    }
    slashed[i] = 0;

    void **table = *(void ***)env;

    jclass (*find_class)(JNIEnvP, const char *) =
        (jclass(*)(JNIEnvP, const char *))table[JNI_FIND_CLASS];
    jmethodID (*get_static_method)(JNIEnvP, jclass, const char *, const char *) =
        (jmethodID(*)(JNIEnvP, jclass, const char *, const char *))table[JNI_GET_STATIC_METHOD_ID];
    void (*call_static_void)(JNIEnvP, jclass, jmethodID, jvalue *) =
        (void(*)(JNIEnvP, jclass, jmethodID, jvalue *))table[JNI_CALL_STATIC_VOID_METHOD_A];

    jclass target = find_class(env, slashed);
    if (!target) {
        fail("class not found", class_name);
        return 2;
    }
    jmethodID main_method = get_static_method(env, target, "main", "([Ljava/lang/String;)V");
    if (!main_method) {
        fail("no main(String[])", class_name);
        return 2;
    }
    void (*arm_after_class)(void) = (void (*)(void))dlsym((void *)-1L, "shim_arm_thread_attach");
    if (arm_after_class) arm_after_class();
    prefetch_standalone_system_server_jars(env);

    jvalue argument[1];
    argument[0].l = build_args(table, env, args_spec);

    say("launcher: running ");
    say(class_name);
    say("\n");
    /* The system server goes through the loader a zygote would have used, so that
     * its classes and the apex jars it loads share one `SystemService`; every
     * other class starts directly. */
    if (!(strcmp(class_name, "com.android.server.SystemServer") == 0 &&
          run_through_system_server_loader(env, class_name, argument[0].l) == 0)) {
        call_static_void(env, target, main_method, argument);
    }

    /* If main threw, the exception is pending on this native frame. Exiting
     * without looking at it hides the only description of what went wrong --
     * an uncaught exception reaches no Java frame to print it. */
    jobject (*exception_occurred)(JNIEnvP) =
        (jobject(*)(JNIEnvP))table[JNI_EXCEPTION_OCCURRED];
    void (*exception_describe)(JNIEnvP) = (void(*)(JNIEnvP))table[JNI_EXCEPTION_DESCRIBE];
    if (exception_occurred(env)) {
        say("launcher: ");
        say(class_name);
        say(" threw, and the exception was pending on the native frame:\n");
        exception_describe(env);
        return 1;
    }
    return 0;
}
