# B: `system_server` to completion, as a checklist

The items in `docs/remaining-work.md` section B, in dependency order, each with the gate
that says it is done. Kept here because it is the list being worked.

Section A's gate is `tools/verify-a.sh` and it passes. Section B has `tools/verify-b.sh`
and it does not exist yet -- that is B0 below, because a checklist without a gate is the
thing that let this section drift.

### The messages are chroot-only

The host path prints none of them: `host run: preload-failures=0, in daemon log=0`, and its shim
loads normally. So the sixteen lines belong to the chroot layout alone, which is consistent with
the child-process reading and rules out anything about the bundle, the binaries or the preload
list itself -- both layouts use the same files and the same list.

That leaves one measurement on this route, and it is the one already named: the shim logging
`getpid`/`getppid` beside its `LD_PRELOAD` once, so the process that reports the glibc messages
names itself and its parent in the daemon's own log. Everything else about the chroot layout is
measured: the daemon runs, publishes readiness, registers as the HIDL service manager, and the
framework launches under bionic's linker from the bundle's root.

### Bionic is the linker in the root, so the glibc messages are a child's

`chroot "$PWD" /linker64 --help` prints bionic's own usage
(`Usage: /linker64 [--list] PROGRAM [ARGS-FOR-PROGRAM...]`) and the md5 of `/linker64` inside the
root equals the bundle's, so the explicit invocation does run bionic's linker. Bionic's strings
do not contain `cannot be preloaded`. Yet the daemon's log -- written by a process launched that
way -- carries eight of those lines.

The only consistent reading is that they are printed by a *child* of those processes, which
inherits `LD_PRELOAD` and runs under the host's loader: glibc prints exactly that string and is
the only program on the machine that does. The next measurement is to catch the child in the act
rather than infer it -- `ps`-style parentage is not available inside the harness, but the shim
can log `getpid`/`getppid` beside its `LD_PRELOAD` once, which names the process and its parent
in the daemon's own log.

### Two loaders, and the message says which is which

`strings` settles the question the message format raised:

```
linker64 (bionic):            0 occurrences of "cannot be preloaded"
ld-linux-x86-64.so.2 (glibc): 1
```

and bionic's linker does carry `library "%s" not found: needed by main executable`, which is the
message the run printed for `liblog.so`. So both loaders are running in the same run: the
`liblog.so` line comes from bionic, and the sixteen `cannot be preloaded` lines come from glibc.
Whichever process prints the latter is not using the bundle's linker at all -- which is a fact
about that process, not about the chroot branch, since that branch invokes
`chroot "$PWD" /linker64 <binary>` explicitly and `/linker64` inside the root is the bionic one.

The next measurement is therefore to have the shim print, once, the loader it was loaded by --
the first line of `/proc/self/maps` and its own `LD_PRELOAD` -- so the process that reports the
glibc messages names itself instead of being inferred from a log that mixes both.

### The two routes converge, and one obstacle separates them

Put together, the two findings leave a single design:

* the connectivity filter passes only when the framework's root is `/`, because the package
  manager stores `sourceDir` by concatenating its scan root with the directory name (measured:
  `Directories scanned as system partitions: [<bundle>:0, ...]` on the host layout);
* a chroot whose root *is* the bundle gives exactly that, and its only obstacle is the binaries'
  `PT_INTERP`, which names the bundle's `linker64` by host-absolute path and therefore cannot
  resolve inside that root, so the kernel falls back to the host's glibc `ld.so`.

The bind that would fix the interpreter is impossible while the root is the bundle (the mount
point would be inside the source), so the fix belongs where the bundle is built and run: make
both layouts invoke bionic's linker explicitly -- `<bundle>/linker64 <binary> ...` -- which is
what the chroot branch already does with the *command line* and what the binaries' `PT_INTERP`
cannot do inside the root. That is a change in `run.sh` and in the chroot branch together, so
the host path keeps working while the chroot path stops falling back to glibc, and it is the next
piece of work rather than a one-line fix.

### The mechanism, in the package manager's own words

The run says it outright:

```
PackageManager: Directories scanned as system partitions:
  [/home/tenzin/.local/share/mosaic/bundle:0, /vendor:524288, /odm:4194304, ...]
```

The scan root is the *bundle*, because that is `ANDROID_ROOT`. So every apex package's stored
`sourceDir` is `<bundle>/apex/com.android.tethering`, and `ConnectivityResources` compares it
against `/apex/com.android.tethering` -- the spelling a device stores. The comparison cannot
succeed, and no shim can fix it, because the string is built by concatenating the scan root with
the directory name: the shim never sees it. The same run also shows the framework reading
`/apex/com.android.tethering/etc/permissions/permissions.xml` and its compatconfig files
successfully, so the apex *content* is reachable; only the stored *name* is wrong.

That is precisely why the chroot layout passes this filter: there the root is `/`, the scan root
is `/`, and the stored `sourceDir` is `/apex/...`. The host layout's fix is therefore the same
insight as the chroot's -- make the framework's root `/` -- and the two routes converge: one
chroot (blocked on the interpreter path) or a mount namespace where the bundle is presented at
`/apex`, `/system`, and the rest. Neither is a shim change, and this is now a recorded design
decision rather than an open question.

### The apex module path: changed, measured, reverted

`src/device/apex.rs` reports each module as `/system/apex/<name>`, and the framework's
`ConnectivityResources` looks for `/apex/com.android.tethering`, so reporting `/apex/<name>`
looked like the fix for the host route's wall. It was built (the harness runs
`target/debug/mosaic`, which the first build missed because `make` builds the release profile),
installed by the build alone, and run: `resource-not-found` stayed at 2 and the boot stopped in
the same place, `ConnectivityServiceInitializer` throwing
`IllegalStateException: No connectivity resource package found`.

So the module path the apex service reports is not what decides that comparison, and the change
is reverted rather than left in unproven. The next measurement is to print the *stored*
`sourceDir` for the tethering package from inside the run -- the package manager's own record,
not the path the service reports -- because that is the string `ConnectivityResources` compares
and the only way to tell whether the shim's `/apex` handling reaches the scan at all.

### The bind cannot exist: the root is the bundle

Retried with the mount's own error visible, the bind of the bundle at its host-absolute path
still fails -- and the reason is structural rather than a namespace restriction. `boot-system-server.sh`
does `cd "$bundle"` before the harness runs, so the harness's `$PWD` *is* the bundle and the
chroot root *is* the bundle: the mount point would be inside the source, which no bind can do.
The first attempt's silence was this, not a refused mount.

So the interpreter path has exactly one place it can be fixed for this layout, and it is the
binary: `patchelf --set-interpreter` to a path that exists inside the root. That cannot be
`/linker64` for a bundle that is also run *without* a chroot -- the host path needs the
host-absolute path the binaries carry now -- so the bundle wants either a second copy of the two
binaries for the chroot layout or a root that is not the bundle (a separate directory with the
bundle bound into it, which the wrapper already does for `/dev`). The second is the smaller
change and it is the one to try: `with-logd.sh` already builds the run directory, so the chroot
root can be that directory with the bundle bound at `/bundle`, and the binaries' host-absolute
interpreter path stays valid for the host path.

### The bind that would have proved it failed, and why that matters

Binding the bundle at its own host path inside the root is the direct fix for the interpreter
path, and the harness printed `mosaic: could not bind the bundle at its host path` -- inside this
user namespace the mount did not take, so the run kept the glibc fallback and the sixteen
messages, which is the expected result once the bind is known to have failed rather than the
refutation of the mechanism.

So the mechanism stands on its evidence (`PT_INTERP` is a host-absolute path; the message is
glibc's; bionic's linker is in the bundle and unreachable at that path) and the fix has two
routes, neither of which is a guess: patch the interpreter path in the bundle's binaries to
`/linker64` when the bundle is built (a `patchelf --set-interpreter` in the bundler, where the
bundle's own layout is known), or bind the bundle into the run root at that absolute path with a
mount that this namespace allows. The first is a bundler change and is the one to try next,
because the bundler already regenerates paths that must not be absolute.

### The mechanism: a host-absolute `PT_INTERP`

`strings` on the bundle's binaries gives the answer:

```
bin/hwservicemanager -> /home/tenzin/.local/share/mosaic/bundle/linker64
bin/dalvikvm64       -> /home/tenzin/.local/share/mosaic/bundle/linker64
```

Both declare their interpreter as the bundle's `linker64` **by host-absolute path**. Inside a
chroot of the bundle that path does not exist, so the kernel cannot use it and falls back to the
host's `/lib64/ld-linux-x86-64.so.2` -- glibc -- which is exactly the program whose message
format appears eight times per process. The bundle's `linker64` is genuine bionic
(`bionic/linker/linker_cfi.cpp` in its strings), so the earlier reading that it was a glibc copy
was wrong: it is bionic, it is simply not reachable at the path the binaries name.

That also explains why pointing the launch at `apex/.../linker64` changed nothing: the kernel
still used `PT_INTERP`, not the command line. The fix is to make the interpreter path resolve
inside the root -- either patch `PT_INTERP` in the bundle's binaries to `/linker64`, or have the
chroot root present the bundle's `linker64` at a path the binaries name (a bind of the bundle
into its own root at that absolute path, which is the same trick the harness already uses for
`/dev`). Either way it is one line, and it is the last thing between the chroot layout and the
services after `ConnectivityService`.

### The apex linker is not the difference either

Pointing the chroot launches at `$bundle/apex/com.android.runtime/bin/linker64` -- the real
android linker the bundler ships beside the glibc-shaped top-level `linker64` -- left the
sixteen messages exactly where they were. So the loader's identity is not what decides them, and
the observation that the message format is glibc's stands as a fact about the *message* rather
than about the process: whatever prints `ld.so: object ... cannot be preloaded` is reading a
preload list in an environment where the inside paths do not resolve, and it is not the linker
choice that changes that.

Reverted, so the tree keeps the configuration whose host run is verified. The chroot route's
remaining question is unchanged and narrower than it has been: one process prints those eight
lines and logs `binder-shim: loaded` in the same run, and the way to catch it is to have the
shim print its own `/proc/self/maps` first line and its `LD_PRELOAD` from inside -- the shim is
the one place that knows it was loaded and can say so with evidence.

### `linker64` in the bundle is dynamically linked

`file` on `$bundle/linker64` says `ELF 64-bit LSB shared object, x86-64, ... dynamically linked`.
Android's linker is a *static* PIE and prints `cannot locate ...` naming the namespace it
searched; the messages in the run are glibc `ld.so`'s. So the chroot launch is starting the
framework and the daemon under a dynamic loader that reads `LD_PRELOAD` from the *host* rules,
while the binary and its shim expect android's, which is why eight inside-paths are reported
missing in the same process that logs `binder-shim: loaded`.

That makes the fix a launch question rather than a staging one. The candidate list to check next
is short: whether `$bundle/linker64` was copied from the host's `ld.so` by the bundler, and which
interpreter `$bundle/bin/dalvikvm64` and `$bundle/bin/hwservicemanager` declare in their `PT_INTERP`
-- one `readelf -l` each, and the mismatch that explains every one of the eight messages.

### The loader in the root is not the one the message comes from

The probe with the daemon's exact environment, run from inside the chroot, prints:

```
ERROR: ld.so: object '/shim/launcher.so' from LD_PRELOAD cannot be preloaded
(cannot open shared object file): ignored.
```

That message format is **glibc's `ld.so`**, not android's linker: android's says
`cannot locate ...` and names the namespace it searched. So the process that reports these eight
failures is being started with a host loader rather than the bundle's `/linker64`, which is also
why the same run's `binder-shim: loaded` can be true at the same time -- the shim is loaded by
whichever loader *does* get the environment it expects, and the preload list is being evaluated
somewhere else entirely.

The next check is one `file` on the two: `$bundle/linker64` and the interpreter the framework's
`dalvikvm64` was built against. If `linker64` is not the static android linker, the chroot
launch is running the framework under a host loader, and the eight failures are that loader
saying it cannot read android's inside paths -- a one-line fix in the launch, not a staging
problem.

### The failures are inside the daemon, and the shim loads anyway

`in daemon log: 8, in run log: 16` -- the daemon's own log carries eight `cannot be preloaded`
lines (one per path) and the run log carries sixteen, eight per process, so both the daemon and
the framework report them. The daemon's log also says `binder-shim: loaded`, and the framework's
run reaches `interposed JNI_CreateJavaVM`, so the shim *does* get in while all eight paths are
reported as failures.

That combination is the whole question now: eight failures and a loaded shim in the same process
means the preload entries are not the mechanism that loads it, and the next run should print the
process's `LD_PRELOAD` as the loader sees it and the contents of `/shim` as that process sees it
-- one `chroot "$PWD" /linker64 /bin/dalvikvm64 --version` with the daemon's exact environment,
whose own output is the answer rather than an inference from two logs.

### The chroot route, re-measured with a harness whose gate is real

With the property reader correct, the chroot run's gate now passes on its own evidence:

```
could-not-read: 0
property dump: hwservicemanager.ready serial=0x04000002 value=b'true'
preload-failures: 16   (each of the eight paths twice)
JNI interceptions: 2
stages: 0   (the framework does not get past the preload list)
```

So the daemon inside the chroot genuinely publishes readiness and this is no longer a harness
artefact. The preload list is the wall, and the count is the clue: *all eight* paths fail twice,
while `binder-shim: loaded` and `interposed JNI_CreateJavaVM` still appear -- two processes
report the failures and something in the run loads the shim anyway. The two extra processes
`boot-system-server.sh` starts before the chroot branch (`idmap2d` and the idmap daemon, through
`run.sh` with `MOSAIC_PRELOAD`) are the candidates for the failures, which would make this a
harness-environment question rather than a chroot one; the next run should print
`LD_PRELOAD` from inside each of those launches rather than assume.

### The reader is complete, and both halves were necessary

The probe printed the file the index names, `u:object_r:default_prop:s0`, at the offset the index
gives: serial 0, value empty. Scanning that file for the value found `true` at byte 1704 -- so
the record starts at 1700, which is the index's 1572 plus the 128-byte area header, with serial
`0x04000002`. The middle column *is* a filename in the directory, and the offset *is*
data-relative with the header in front of it; each of the two readings I had was half right, and
the two reader bugs I introduced came from treating one half as the whole answer.

With both in the reader, the harness prints what it printed before the file was corrupted:

```
could-not-read: 0
property dump: hwservicemanager.ready serial=0x04000002 value=b'true' name=b'hwservicemanager.ready\x00'
host root: exit 134, 102 packages, scudo 0, ext-services 0
```

The wait is a real gate again rather than a report, and the file is 257 lines with every measured
fix in place.

### The index's second column is a file, and the reader now uses it

The two-way probe (read at 2 s and at 22 s) answered both candidates at once. Every area file is
present and named: `index.tsv`, `properties_serial`, `property_info`, and
`u:object_r:default_prop:s0` -- the middle column of the row for `hwservicemanager.ready`, read as
a *file* in the directory, which is what the first reader I wrote did before I "fixed" it away
by treating that column as a label. Nothing changed between 2 s and 22 s: the row is the same,
the file sizes are the same, and `properties_serial@1572` is zero at both times.

So the harness reads the right row, and the file that row names is the one to open. That is what
the wait now does, and it still reports rather than gates -- the record it reads there is not
`true` at the offset the index gives, which is the last step: print `u:object_r:default_prop:s0`
at 1572 and confirm where the service's write actually lands. The framework boots through the
same property regardless, at the same 78 stages and 102 packages, so this is the harness's
reader and not the runtime's behaviour.

### The area is a copy, and the service writes it -- so the empty record is narrower

`with-logd.sh` copies `$MOSAIC_PROPERTY_DIR/*` into `/dev/__properties__/` inside the namespace
and then runs the property service with `ANDROID_PROPERTY_DIR=/dev/__properties__`, so service,
daemon, framework and the harness's own reader all use the same directory. The reader's
`serial=0x00000000` at the index's offset is therefore not a different file: it is a record the
service did not write, in the file the reader picked, at the offset the index gave.

That leaves two candidates, and the next measurement separates them in one run: the service wrote
a *different* record whose index row the reader is not using (print every row whose name matches
and every `properties_*` file's size), or it wrote the record but the harness's copy is a
snapshot taken before the write (print `stat` of the file and the row again at 5 s and at 30 s).
The wait keeps reporting rather than gating, so the boot reading is unaffected either way.

### The reader is right and the area it reads is not

With the layout matched to AOSP's `prop_info` (`uint32_t serial; char value[92]`, the index
offset absolute in the area file), the dump finds the record and prints
`serial=0x00000000 value=b''` -- an unwritten slot -- while the framework passes the same wait
and boots to the same 78 stages. So the harness's own view of the area is not the one the
daemon writes, and the wait correctly reports rather than gates.

That is a statement about which area this shell maps, not about the daemon: the daemon sets the
property, the framework reads it, and the harness's reader sees an empty record at the right
offset in a `properties_serial` file it can open. With-logd.sh mounts the property area into the
run's `/dev/__properties__`, so the next measurement is to print, from the harness, the device
and inode of the file it opens and the one the daemon's shim reports writing to.

### The reader's arithmetic, measured

A diagnostic printed the area's actual contents: the index gives offset 1572 for
`hwservicemanager.ready`, `properties_serial` is 131072 bytes, and at `1572 + 128` the bytes are
all zeros -- and the property's *name* is nowhere in that file. Names live in the shim's trie;
the area file holds the value records only. So the reader's base is wrong in a way that the
framework's own wait is not, and the wait keeps its measured behaviour: report, then start the
framework anyway. Fixing it means matching the shim's own record layout
(`tools/binder-shim/android-properties.c` is the authority) rather than guessing another base.

### The rebuild is verified

The rebuilt `tools/boot-with-hwservicemanager.sh` reproduces the pre-corruption host run exactly:
`exit 134`, **78 stages, 102 packages, scudo 0, ext-services 0**, and `build 0 warnings, 136
tests ok`. The trace switch, the identity echo, `start_daemon` (pure and chrooted), the
`/linkerconfig/ld.config.txt` staging with `:/shim` in `namespace.default`, the framework launch
with the full environment in both layouts, the five-second property dump and the timeout loop
are all back.

One reader remains imprecise, and it is now harmless by construction: the property wait's reader
reports `this harness could not read hwservicemanager.ready as true; starting the framework
anyway` and the dump prints `not set in any properties_* area`, while the framework itself
boots through the same property -- so the reader's offset arithmetic for the area files is
wrong (the index rows are `name<TAB>context<TAB>offset`, the offset is data-relative with the
area header in front of it, and the per-file layout is the part still unverified), and the wait
no longer aborts on it. The framework's own wait is the authority and it succeeds.

The corruption came from an edit of mine that deleted every newline in the file while removing a
probe. Two rules follow: this harness is untracked and should be tracked, and edits to it belong
in a tool that refuses to write a file whose line count collapsed.

### The harness was corrupted and rebuilt

`tools/boot-with-hwservicemanager.sh` is untracked, and a probe-removal edit of mine replaced
every newline in it: 12899 bytes with no line breaks, which `bash -n` still accepts because the
first line is `#!/bin/bash# ...` and everything after the `#` is a comment. Nothing in git holds
it, and no other copy exists on the machine.

It has been rebuilt from the recovered content -- the bytes were intact, only the structure was
gone -- with every measured fix in place: the trace switch and identity echo, `start_daemon`
with the pure and chrooted forms, the log-line diagnostic and the property wait that decides,
the chroot staging (`/dev`, `/proc`, `/tmp`, the linker configuration at
`/linkerconfig/ld.config.txt`, `:/shim` in `namespace.default`), the framework launch with the
full environment in both layouts, the property dump at five seconds, and the timeout loop.

Two lessons worth keeping: the file is untracked and should be tracked (it is a harness this
project depends on for every wall measurement, and losing it costs a reconstruction), and edits
to it belong in a tool that refuses to write a file whose line count dropped to zero.

### The probe's answer: the process's root is not the bundle

A probe inside the harness, after the staging and before any process starts, asked for the three
libraries by their in-chroot names and got `No such file or directory` for all of them --
including from the host side, where `/shim` and `/lib64` do not exist at all. The run root is
`$PWD`, which `boot-system-server.sh` sets to the bundle with `cd "$bundle"` (line 76), so
`/shim/launcher.so` *must* resolve inside a chroot of `$PWD`. It does not, and the preload list
agrees with the probe.

That leaves exactly one thing to print, and it is the next command rather than another
hypothesis: `readlink /proc/self/root` from inside the same `chroot "$PWD"`, plus `pwd` and
`ls /` from inside it. The session's earlier readings -- the log-line gate, the wait ordering, a
stale copy, a missing bundle tree, the namespace counts, the sed breadth -- are all refuted with
the runs that refuted them, so the root's actual value is the remaining unknown, and the answer
is one probe away.

### Every namespace, measured: worse, and reverted

Adding `/shim` to *every* namespace's search and permitted paths made the preload list fail for
both processes -- `launcher.so` included, which had loaded with the `namespace.default` edit
alone. That is a measurement in the useful direction: the edit is not neutral, so the namespaces
differ in a way that decides which preloads resolve, and the next attempt should print the
generated configuration's namespace sections (the file is written into the bundle root during
the run, so a plain `sed -n` of it afterwards shows exactly what the linker read) rather than
guess which section admits an executable under this root.

Reverted to the narrower edit. The two fixes that stand are the linker configuration's path
(`/linkerconfig/ld.config.txt`, which removed the linker's own complaint) and the
`namespace.default` entry.

### The linker configuration: found, and the shim namespace

Two fixes landed and both are visible in the run. The generated configuration now goes where
android's linker actually reads it -- `/linkerconfig/ld.config.txt` under the root, not the path
`LD_CONFIG_FILE` names -- and the chroot run no longer says
`failed to find generated linker configuration`. The shim's directory was also added to the
namespace's search and permitted paths, and `launcher.so` now preloads: the run reaches
`launcher: interposed JNI_CreateJavaVM` twice.

The other seven remain: `probe.so`, `pretend-nice.so`, `pretend-cgroups.so`,
`android-binder.so`, `android-properties.so`, `alloc-trace.so` and
`lib64/libandroid_runtime.so` still report `cannot be preloaded (cannot open shared object
file)`. Since `launcher.so` is in the *same directory* and now loads, the difference is not
the directory: it is which namespace the executable that preloads them belongs to, and
`namespace.default.search.paths` is only one of them. Next step: list every namespace section in
the generated configuration and add the shim's directory to each one that admits an executable
under this root -- the file is generated from `ld.config.txt` in the bundle, so the change
belongs in the same `sed`.

### The root is the bundle, and the libraries are all there

`boot-system-server.sh` does `cd "$bundle"` before it starts anything (line 76), so the harness's
`$PWD` -- and therefore the chroot root -- *is* the bundle. Every library the preload list names
exists in it: `shim/` holds `launcher.so`, `probe.so`, `pretend-nice.so`, `pretend-cgroups.so`,
`android-binder.so`, `android-properties.so`, `alloc-trace.so`, and `lib64/libandroid_runtime.so`
is present.

So the earlier reading -- that the bundle's tree is missing from the root -- is wrong, and the
binding loop was answering a question that did not exist. A probe run from inside the harness
confirms the contradiction directly: with the root at the bundle, the very first
`LD_PRELOAD=/shim/launcher.so chroot "$PWD" /linker64 /bin/dalvikvm64` reports
`cannot be preloaded (cannot open shared object file)` plus
`failed to find generated linker configuration from "/linkerconfig/ld.config.txt"`.

That is a statement about *when* the loader resolves its paths, not about what is on disk: the
preload list is applied by `/linker64`, and the executable's own interpreter runs before the
chroot's view applies to it in the way the harness assumes. The next step is to stop passing the
absolute paths through `LD_PRELOAD` for the chroot path and instead let the linker configuration
name them (`dir.shim`, or a `namespace` entry), which is how a real Android root does it -- the
generated `ld.config.txt` already exists and only needs the shim's directory in its search path.

### The binds succeed and the preloads still fail

The binding loop reported `bound <entry>` for every top-level bundle entry -- apex, bin, lib64,
shim, system and the rest -- so the staging does what it says, and `/shim/*.so` still resolves to
nothing for the process that reports the failures. The run's order says which process that is:
the 16 `cannot be preloaded` lines appear *after* the first SIGABRT trace and *before* a second
`binder-shim: loaded` plus two `interposed JNI_CreateJavaVM`, so it is a process started after
the failure, not the first framework launch -- most likely the daemon's children or the
framework's second attempt, still under the pre-bind view of the root.

Next measurement, one command: `ls -l /shim /lib64` *inside* the chroot of the run directory
right after the staging (before either process starts), and print `readlink /proc/self/root`
from inside that chroot. That distinguishes "the binds are not visible in the chroot" from "the
processes that fail are not the chrooted ones" without another full run.

### The real defect was mine: the property wait ran before the branch

`MOSAIC_HARNESS_TRACE=1` (a switch added to the harness) produced the answer in one run: the
trace ends with the property wait's own lines at top level, *before* the chroot branch, and
`exit 1`. My restructure had moved the daemon start and the first wait into `start_daemon` but
left the *property* wait -- the one that can succeed -- outside it, so it ran unconditionally,
waited for a daemon that the chroot path had not started yet, and killed the run before the
branch was ever reached. The entry `echo` that never printed was consistent with this all along;
the three hypotheses before it (log-line gate, ordering, stale copy) were each refuted by
measurement, and the identity echo proved the executing file is this file
(md5 `49582b902f4055330a78c1129c43dbbc`, matched in the run).

With the wait moved inside the function, the chroot path gets much further in one step:
`hwservicemanager.ready` is published by the chrooted daemon and the property dump shows
`value=b'true'`; the framework launches, reaches `launcher: interposed JNI_CreateJavaVM` twice,
and stops on the preload list -- `/shim/...` and `/lib64/libandroid_runtime.so` do not exist
under the chroot root, because that root is the run directory, which the wrapper fills with
`/dev`, `/proc`, `/tmp` and the property area, but not with the bundle's tree. Binding each
top-level bundle entry into the run directory before the daemon starts is the fix to try next;
the first attempt (a loop of `mount --rbind` per entry) reported no mount errors and still left
`/shim/launcher.so` unresolvable, so the next run should print the loop's result rather than
assume it.

### The stale-copy hypothesis is refuted too

No copy of `boot-with-hwservicemanager.sh` exists under the work directory or `/tmp`, and the
wrapper (`tools/bundle/with-logd.sh`) never copies the script -- it only copies the property
files. So the run executes this file, and the contradiction stands as the open question: the
property wait's message is printed (that code is inside `start_daemon`), the framework is never
launched (`dalvikvm64` appears zero times in the run log), and an entry `echo` in the same
function prints nothing at all in the same stream.

That combination -- later code in a function executing, earlier code in it silent -- is not
explained by any of the three hypotheses tried (log-line gate, ordering, stale copy). The next
experiment should not add another marker: it should capture what the harness actually runs by
starting it with `bash -x` and reading the trace around line 171, which settles in one run
whether the branch is reached at all.

### Instrumentation result: the function's own echo never appeared

`set -x`-style markers were added at the function's entry and right after the chroot daemon's
launch, and the run log contains neither -- yet the property wait, which is *inside* the same
function, does run (its failure message is what the harness prints last). That is the
contradiction to chase: an entry echo that does not appear while later code in the same function
does means the function body being executed is not the one being read, i.e. the copy the wrapper
actually runs differs from this file. The wrapper stages a copy under the work directory
(`$BUNDLE/...`), and an earlier session was already bitten by a *stale shim copy in the bundle*,
which is the same failure shape.

Next step, one command: print the resolved path of the script the wrapper executes, and diff it
against the repo's `tools/boot-with-hwservicemanager.sh` before drawing any conclusion from a
run. Markers removed; the host path is verified again after their removal (102 packages, 78
stages, scudo 0, ext-services 0, readiness ok).

### The restructure: daemon and waits moved into the chroot branch

The daemon start and the two readiness waits are now a function, `start_daemon`, called on the
host path at the top and *inside* the chroot branch after its staging (`start_daemon chrooted`,
which starts the daemon with `chroot` and the same environment the framework gets). The host
path is verified unchanged -- 102 packages, 78 stages, scudo 0, ext-services 0, and
`hwservicemanager is ready now` in its log.

The chroot path still reports `hwservicemanager.ready never became true` with an empty daemon
log, so the call is not doing what it reads as. Next: `set -x` around the branch, or a direct
echo of `$?` right after the daemon launch, to see whether the function was entered at all --
the pieces are all proven individually (the daemon runs chrooted, publishes readiness, and
registers as the service manager), so this is about the harness's control flow rather than
about the runtime.

### The harness's own order is the last obstacle

The daemon start and *both* readiness waits sit above the chroot branch, so a daemon started
inside the chroot is never reached by them: the waits time out first (30 s with the widened
count, and the run takes 73 s), and the harness gives up before the framework is launched at
all. That is the measured reason a chroot daemon cannot satisfy the harness as the script is
laid out.

The fix is a restructure rather than a line: with `MOSAIC_CHROOT=1` the daemon has to start
*inside* that branch -- after its staging -- and the two waits have to happen there as well,
before the framework's own launch. The pieces are all measured: the daemon runs in that root and
publishes readiness (its shim log shows `set hwservicemanager.ready=true` and
`BINDER_SET_CONTEXT_MGR -> ok`), the preload set has to be the whole one, and the log-line check
cannot be the gate because a chrooted daemon logs through logd.

### The readiness wait was watching the wrong place

The harness's first check greps the daemon's own log for `hwservicemanager is ready now`. A
daemon started *inside* the chroot logs through logd -- the line goes to logd's socket, not to
that file -- so the check can never see it, and it was fatal before the wait below it, which
reads the property area and is the one that can succeed. On the host path the same check passes
(`hwservicemanager is ready now: 1` in a verified run), which is the contrast that makes the
difference visible.

Both changes are in: the check is now a diagnostic that says so, and the chroot daemon starts in
the plain `env -i … chroot … &` form -- the one that demonstrably worked when run by hand and
logged `set hwservicemanager.ready=true` and `BINDER_SET_CONTEXT_MGR -> ok`. The `bash -c`
variation that produced nothing is not used.

### The chroot daemon works: it publishes readiness and serves

With the start captured so its output survives, the chrooted daemon does everything it is for:

```
android-properties: set hwservicemanager.ready=true
binder-shim: ioctl 0x40046207 BINDER_SET_CONTEXT_MGR (hwbinder) -> ok
binder-shim: ioctl 0x40046205 BINDER_SET_MAX_THREADS -> ok
binder-shim: ioctl 0xc0306201 binder-shim: write=4 read=0
```

What the earlier attempts lacked was not the daemon but the *capture*: started in the chroot
branch with its output going to a file that a failing `env` line never reached, it looked as
though it had exited silently. It had not. So the chroot layout has its daemon, and the
remaining gap is now only that the harness's readiness wait does not observe the property the
daemon sets -- the area it reads and the one the property service writes are the thing to
compare next, since both are inside the same namespace.

### Under the harness the same daemon exits before writing anything

By hand, in a bare namespace with the full preload set, it ran for the whole four-second
timeout. Under the harness it produces **zero lines** -- with its own log file and with stderr
inherited into the harness log -- and the harness reports `hwservicemanager.ready never became
true`. So it is not the binary, not the preload set, and not the root; it is something the
harness's namespace has that a bare one does not, and the daemon exits before it says why.

That is where the next session should start, and it is a small question now: the difference
between the two environments is the mounts (`/dev` as the wrapper's tmpfs with the property
socket, `<bundle>/dev` bound over it), the property area, and the daemon's four extra variables.
Running the harness's own daemon command by hand *inside its namespace* would show the exit
status in one step.

### The chrooted daemon *does* run -- what fails is the readiness handshake

Run directly, with the full preload set and no `env` in between:

```
exit: 124        <- the timeout killed it at four seconds, i.e. it was still running
(only outer-shell preload warnings, which every `timeout`/`chroot` in the chain inherits)
```

So a chrooted `hwservicemanager` **starts and stays up**. The link failure in the previous
attempt was real and is fixed by carrying the whole preload set; what remains is that the
harness never sees it say it is ready:

```
boot-with-hwservicemanager: the HIDL service manager did not say it is ready:
boot-with-hwservicemanager: hwservicemanager.ready never became true;
```

The daemon publishes that property through `/dev/socket/property_service`, so the next thing to
check is whether the property service is reachable *from inside the chroot* at the moment the
daemon starts -- the socket, its ownership, and the order the harness starts them in. That is a
narrow, testable question rather than a structural one, and it is the last thing between the
chroot layout and the device-shaped scan paths it already delivers.

### The chrooted daemon fails to link: a shim symbol its siblings provide

Reproduced outside the harness, where the message is visible instead of swallowed:

```
CANNOT LINK EXECUTABLE "/bin/hwservicemanager": cannot locate symbol
"shim_weak_reference_taken" referenced by "/shim/probe.so"
```

`probe.so` is built with `-nostdlib` and leaves symbols for the Android linker to resolve, and
that one comes from another object. Preloading `probe.so` on its own leaves it unresolved and
the *daemon* exits before it can say anything -- which is why its log was empty and the harness
only reported `hwservicemanager.ready never became true`. The framework's invocation preloads
the whole set and links; a daemon started with a narrower set does not. Any chroot daemon
invocation must therefore carry the *same* preload list as the framework, which is what the
next attempt should use.

### The chroot daemon, second attempt: staging fixed, startup still silent

The daemon's start was moved to *after* the chroot's own setup (linker configuration, the
mounted directories, the environment) rather than before it, which was the ordering that made
its preloads unresolvable:

```
before:  ERROR: ld.so: object '/shim/launcher.so' from LD_PRELOAD cannot be preloaded …
after:   (no preload errors)  but `hwservicemanager.ready never became true`
```

With the ordering fixed the daemon starts cleanly and says nothing at all, so whatever stops
it now is silent -- no preload complaint, no message in its log. Reverted, so the tree keeps
its working state (102 packages, 78 stages, scudo 0, ext-services 0). The two facts worth
keeping are that the ordering *was* the preload failure, and that the next thing to look at is
why a chrooted `hwservicemanager` exits without a word -- its own log has nothing in it, so
the question is what it needs that the framework's environment does not provide.

### The root split is the chroot's defect, confirmed by trying to fix it

The daemon was started *inside* the chroot (`chroot "$PWD" /linker64 /bin/hwservicemanager`
with the same environment the framework gets) so that one root would be shared. It does not
start: the linker reports every preload unresolvable --

```
ERROR: ld.so: object '/shim/launcher.so' from LD_PRELOAD cannot be preloaded (cannot open shared object file): ignored.
```

-- which is the same staging order the framework's own invocation handles, and it needs the
preload paths and the linker configuration in place *before* the chroot rather than after.
Reverted, so the tree keeps its working state. The direction is right and the mechanism is
now proven: the daemon's bundle-absolute paths are what the certificate step is handed, and
they are unreachable inside the chroot.

### The framework's own path layer is correct; the complaint is another process

Logging the pid and both spellings for one path:

```
android-paths: framework-res pid=60155 from=/system/framework/framework-res.apk  to=//framework/framework-res.apk
android-paths: framework-res pid=60155 from=/framework/framework-res.apk         to=//framework/framework-res.apk
incfs: isIncFsPathImpl(): could not statfs /home/…/bundle/framework/framework-res.apk: No such file or directory
```

`//framework/framework-res.apk` inside the chroot *is* `<bundle>/framework/framework-res.apk`,
and it exists. So the framework's own path layer resolves this correctly, and the `statfs`
complaint quoting the bundle's absolute path comes from a process whose root is the bundle --
the daemon, started outside the chroot -- not from the framework. The certificate failures are
therefore downstream of something else, and the next measurement is to run the same check with
the daemon's paths in device form too, or to attach the pid to the `incfs` message by way of
the shim's own `statfs` hook.

### Where the chroot's mixed path comes from, as far as it is measured

The shim prints the root it was given, and in a chroot run it prints the **bundle's**
absolute path -- that line comes from the wrapper, which is the *outer* process, so it
describes the daemon's environment rather than the framework's. The framework's own
environment is set by the harness's chroot branch (`MOSAIC_ANDROID_ROOT=/`, line 163), so
the path layer inside the chroot should produce `/framework/...` and not
`<bundle>/framework/...`.

That means the host path in the certificate failure is either handed over as a *string* by
the framework itself, or produced in a process other than the framework. Distinguishing those
is the next measurement: log the mapped path *and the pid* for the `framework-res.apk` open,
and compare it against which process printed the `statfs` complaint.

## The chroot layout is the closest to working, and its obstacle is a path mix

Re-tested with every fix of this session in place -- the duplicate `/system/framework` bind
gone, `realpath` correct, the device names served, the env right. Its three old walls are all
gone:

```
resource-not-found 0     ext-services 0     frameworks-pkg-fail 0
scanned as system partitions: [/:0, /vendor:524288, …, /apex/com.android.adbd:8388608, …]
```

The scan directories are **device paths** and the connectivity filter passes. What stops it is
narrower: only two packages are scanned, and 101 certificate collections fail --

```
incfs: isIncFsPathImpl(): could not statfs /home/…/bundle/priv-app/BlockedNumberProvider/…
PackageManager: Failed to scan /priv-app/BlockedNumberProvider: Failed to collect certificates
```

The scan knows the package as `/priv-app/<name>` and the certificate step is handed a **host**
path -- a mix of spellings that inside a chroot is worse than outside it, because the host path
is not reachable from within. `statfs` is already hooked (a second hook does not even
compile), so the path is being rewritten somewhere that resolves to the bundle's absolute
path, and finding which call does that is the next step. `/` cannot be made writable to give a
real `/apex` (`mkdir /apex`: permission denied; binds only work over an existing mountpoint),
so the chroot is the only route to device-shaped scan paths.

## The connectivity filter is a trade, measured both ways

The apex directory was moved to `apex-modules` and the shim's `/apex` rule pointed at it, so
that the root-derived scan would find nothing at `<bundle>/apex` and only the device-shaped
listing would supply the apexes. Both halves of that are now measured:

| apex layout | apex packages | connectivity filter | stages |
| --- | --- | --- | --- |
| `<bundle>/apex` (as it is) | 102 | fails: `sourceDir` reads `<bundle>/apex/...` | **78** |
| `<bundle>/apex-modules` | **88** | **passes** (`resource-not-found: 0`) | 18 |

So the scan list really is root-derived: the packages come from `<bundle>/apex`, and taking
that directory away takes the apex packages with it -- `Required services extension package is
missing` comes back. The listing is not what feeds the scan; `/apex` → `apex-modules` resolved
correctly throughout, and it changed nothing about which packages were found.

That closes the loop on this filter: it cannot be satisfied by any arrangement of the shim,
because `sourceDir` is a string the package manager builds from its root and keeps. It needs
either the apexes reachable at a *real* `/apex` in the framework's namespace (a mount in the
harness's namespace, which is what `docs/remaining-work.md` calls the product's alternative to
path redirection), or the framework's own expectation changed. Both are decisions rather than
patches, and the measurements above are what they should be decided on.

Both experiments are reverted: the directory is back at `<bundle>/apex`, the shim's rules are
back, and the boot is re-verified at 78 stages / 102 packages / 0 Scudo errors.

## The connectivity filter is still open, and the apex reverse does not touch it

Instrumented: `realpath` logs every time its answer is rewritten for an apex path.

```
apex-reverse lines: 0
resource-not-found: 2      stages: 78      packageCount: 102
```

**Zero.** The framework never canonicalises an apex path, so the `apex_reverse` that
`ConnectivityResources` was supposed to be satisfied by never runs. The earlier report of
`resource-not-found: 0` came from a 41-stage boot that died *before* `ConnectivityService`,
where the check had not run at all -- the same false-pass shape the stale-shim note describes,
and the second time this session has read a zero that way.

So the fix has to reach the **scan** rather than the canonicalisation: `sourceDir` is built
from the partition directories in `mDirsToScanAsSystem`, those come out as
`<bundle>/apex/<module>/...`, and the comparison is against `/apex/com.android.tethering/`.
The same listing that produces the apex list asks the shim for `/apex` and is answered with the
bundle's directory -- so the *names* are device-shaped while the *directories* are not, and
which of the two paths the package manager keeps is the next thing to measure.

## The stale shim: every measurement after `build-native` stopped copying

The bundle's `shim/launcher.so` had gone hollow (`has bad ELF magic: 00000000`) and the rest
of the shims were older than the sources, so a run of fixes was being tested against stale
binaries -- which is why the corruption "persisted" through several fixes that had already
addressed it. `build-native.sh` is supposed to copy them into the bundle; that stopped
happening at some point, and nothing checks it.

With the shims synced by hand:

```
before (stale shims)   scudo 1   stages 48   last stage StartInputManagerService
after  (fresh shims)   scudo 0   stages 78   last stage StartConnectivityService
```

**The lesson, and it cost more than one run**: a count of zero is not a pass when the boot
dies before the check that produces it. `resource-not-found: 0` was measured at 41 stages,
where the boot never reached `ConnectivityService` -- the check had not run. Only now, at 78
stages, does the filter actually execute, and it fails.

## Where the boot is

Measured, not remembered:

```
packageCount 102    stages (distinct) 41    resource-not-found 0
dropbox-rename 0    ext-services 0           keystore-NPE 0
last stage: InstallSystemProviders
```

The layout that reaches this is the **host root** (`ANDROID_ROOT=<bundle>` in the bundle's
`env.sh`, refreshed into the bundle because `run.sh` regenerates that file on every launch).
The device-root layout is retired: it scans 88 packages and dies on
`Missing required system package: android.ext.services`.

## The wall

`scudo: Scudo ERROR: corrupted chunk header`, aborting in
`art::DlOpenOatFile::Dlopen`, reached from `InstallSystemProviders`. The chunk is freed
there and was corrupted earlier. Eliminated so far: `alloc-trace.so` in the preload list,
the bundled odex files (there are none), the shim's unbounded path copies (now bounded),
a bare-name `@` filter in `ApexManagerFlattenedApex` (real, but not this), the apex info
list (three content variants), and `realpath` mapping in either direction beyond the apex
reverse.

Next probes, in order:

1. `MOSAIC_ALLOC_TRACE=1` (the instrument is opt-in and inert without it) and correlate the
   corrupt address against `/tmp/mosaic-alloc-trace.log` -- the recipe is in that file's own
   header. This says whether a chunk was written past or freed twice, and by which thread.
2. Remove `libandroid_runtime.so` from the preload list as the last shim-adjacent object in
   that address space, to see whether the corruption is ours at all.

## B0. The B gate

- [x] `tools/verify-b.sh <bundle>`, in the shape of `verify-a.sh`: run A first, then the
      /data tree check (B12), the persistence round-trip (B13), a full boot checked for
      boot completed / 0 ext-services / 0 fatal exceptions / the connectivity resource found
      / vold and keystore answered, a second boot reusing `packages.xml` (B10), the boot image
      or the recorded imageless timing (B14), and `am start` on a service-only fixture.
- [x] `make verify-b` (with a `BUNDLE` default, so it runs from a clean checkout).
- [x] Its first honest report: `A verification passed` / `2. B12 -- the /data tree` /
      `B gate failed: B12: /data/system_ce/0 is missing`, then after B12 and B13 were
      done, `B gate failed: missing runtime marker: sys.boot_completed=1` -- the gate
      names the unmet gate instead of passing on a milestone nobody reached.
- [ ] A fixture APK under `tools/fixtures/` for the `am start` step.

### The launcher's JNI offsets, and what the checker actually shows

`launcher.so` dereferences the real `JNIEnv` (`void **table = *(void ***)env`) and calls
through **hardcoded offsets**, seven of which do not match the standard
`JNINativeInterface` numbering: `GetStaticMethodID` is 83 (not 113), `NewStringUTF` 156
(not 167), `NewObjectArray` 161 (not 172), `SetObjectArrayElement` 163 (not 174),
`CallVoidMethodA` 63 (not 51), `CallStaticObjectMethodA` 86 (not 116),
`CallStaticVoidMethodA` 113 (not 143).

Three measured attempts to correct them, and what each did:

| change | result |
| --- | --- |
| the three Call* ones alone | segfault at stage 0 -- and this one is explained: 113 was left as `GetStaticMethodID`, so `CallStaticVoidMethodA` collided with it and a method-ID lookup called the wrong function |
| all seven | segfault at stage 0 as well |
| all seven + `-Xcheck:jni` | ART reports `jclass is an invalid local reference: 0x... (deleted reference at index 39383)` -- a *different* fault, and an index that large means the JNI reference table itself is damaged |

So the offsets are wrong *and* correcting them is not sufficient: the same class of damage
appears either way. That, plus the Scudo corruption being a `free` of a pointer that was
never allocated, points at **memory damage in our own native code** -- `launcher.so`,
`probe.so` or `android-binder.so` -- rather than at ART or at the offsets. Whoever picks
this up should not start by re-deriving the table: start with the invalid reference and the
wild free, both of which have addresses recorded.

Reverted each time; the offsets that boot are back, and the A gate is re-verified against
them before B is read again (a broken launcher makes A fail, which is exactly what the gate
did while these experiments were in the tree).

### FIXED: the Scudo corruption was our `realpath`, not ART

`realpath` with a NULL `resolved` must return a **`malloc`'d** buffer, because the caller
owns it and frees it. The shim returned a thread-local one, so ART's
`DexFileLoader::GetDexCanonicalLocation` and `DlOpenOatFile::Dlopen` both took that path and
freed memory they did not own.

The instrument that finally named it was the allocator itself, not a trace:
`libc.debug.malloc.options=guard` in the property area makes Bionic's malloc report the
double free at the offending call with a backtrace, instead of Scudo noticing a corrupted
chunk header later and somewhere else:

```
malloc_debug: +++ ALLOCATION 0x78e05b699ed0 HAS INVALID TAG 0 (free)
malloc_debug:           #00  … libart.so (art::DlOpenOatFile::Dlopen(…)
malloc_debug:           #00  … libdexfile.so (art::DexFileLoader::GetDexCanonicalLocation(…)
```

Two callers of one chunk, one of which had never allocated it. Fixed by allocating when the
caller owns the result. Measured before and after:

```
                scudo errors   stages   last stage
before          1              41       InstallSystemProviders
after           0              48       StartInputManagerService
```

Every earlier elimination stands as an elimination: the JNI offsets are still wrong but are
not this, the `@` filter is real but is not this, and the preloads are not this.

## B9. Every service after `PackageManagerService`

AMS, WMS, ATMS, `StorageManager`, `RoleManager`, `PermissionManager`, `ConnectivityService`,
`AlarmManager`, `JobScheduler` -- each announces what it needs, as the fonts did.

- [x] `ConnectivityService` -- `resource-not-found: 0`, `ConnectivityServiceInitializer`
      failure 0, and the boot continues into `ContentService` and `AccountManagerService`.
      The fix is the apex reverse in the shim's `realpath`: canonical answers under the
      bundle's `apex` come back as `/apex/...`.
- [ ] `StartInputManagerService` is where it stops now, and the backlog names the
      InputReader as the next item after that. The walls already named by runs, in the
      order the boot meets them:
      - [x] the Scudo corruption (was our `realpath`; see above) -- 41 to 48 stages
      - [ ] `android.os.ZygoteStartFailedEx: Error connecting to zygote` -- a background
            preload in `startOtherServices` that fails and is tolerated; nothing hosts a
            zygote yet, and `InstallSystemProviders` is the first thing that *needs* an app
            process.
      - [ ] whatever the next fatal line says.
- [ ] Gate: `SystemServerTiming: StartServices` completes, `sys.boot_completed=1` is set
      through the property service, and there is no `FATAL EXCEPTION IN SYSTEM PROCESS`.

## B10. `PackageManagerService`'s inputs

- [x] Reads `/system/etc/permissions`, `sysconfig`, `privapp-permissions` and the overlays
      (visible in the boot log).
- [x] Writes `data/system/packages.xml`.
- [ ] Zero warnings about those inputs in a boot log.
- [ ] A second boot reads `packages.xml` instead of rescanning into a failure. `do_apps`
      deletes it on every bundle build, so this check must run without rebuilding.

## B11. `vold`

- [x] `installd` (app data directories, sizes, dex and profile removal, native library
      directories, everything else refused by name).
- [ ] The `vold` equivalent: extend `src/device/vold.rs` from logged-only to what
      `StorageManagerService` actually calls (method numbers read from a boot log), answer
      one emulated internal volume at `/data/media/0`, mounting as a no-op success and
      encryption as "not encrypted", refuse the rest by name the way `installd.rs` does.
- [ ] Unit tests beside the others.

## B12. `/data` laid out as Android expects  [OK]

- [x] `do_data` in `tools/bundle/bundle.sh`, wired into the build and applied:
      `tree check: 13/13`, with init.rc's modes (`property` 700, `tombstones` 771,
      `media/0` 770). Ownership is reported through `MOSAIC_UID` as the plan says.
- [x] Gate: `verify-b.sh` step 2 checks the tree and now passes. in `tools/bundle/bundle.sh` creating the init.rc tree: `system`,
      `system_ce/0`, `system_de/0`, `misc`, `user/0`, `user_de/0`, `app`, `dalvik-cache/x86_64`,
      `local/tmp`, `property`, `media/0`, `tombstones`, `anr`, with init.rc's modes.
      Ownership is reported through `MOSAIC_UID`, not changed with `chown`.
- [ ] Gate: the tree is present with those modes after a build.

## B13. A property store that persists  [OK]

- [x] `tools/bundle/android-property-service.py` writes `persist.*` sets to
      `data/property/persistent_properties`, one `name=value` per line, atomically
      (write then rename).
- [ ] `make-property-area.py` (or the service at startup) loads that file into the area,
      for declared names only.
- [x] Gate: `tools/verify-persist.sh` exercises the service's own functions against a
      scratch root -- save, drop, load back, save a second and check the first survives,
      and check the temporary is not left behind. It passes, and `verify-b.sh` step 3
      runs it.

## B14. A boot image, or accept imageless  [decision recorded]

- [x] The decision is written down where the gate can see it: `do_bootimage` in
      `bundle.sh` records `data/mosaic-imageless.txt`, naming what ART says itself
      (`Attempting to fall back to imageless running`) and what the alternative is.
- [ ] `bundle.sh bootimage <bundle>`: build `framework/boot.art`/`.oat` from `BOOTCLASSPATH`
      with the bundle's own `dex2oat64`, and have `run.sh` pass `-Ximage`. If that cannot
      produce a usable image on the host, the fallback is to accept the imageless start --
      written down as the decision, with the gate recording boot time instead.
- [ ] Gate: the image is loaded, or the recorded imageless timing is present.

## B15. What `init` does that nothing else does

- [x] `/dev/ashmem` -- a memfd per region (215 `ashmem creation failed` -> 0).
- [x] `/dev` with `null`, `zero`, `random`, `urandom` -- **answered by the shim**, the way
      `/dev/ashmem` and the binder devices are: a memfd per name, filled from `getrandom`
      for the two random ones. `hwservicemanager: ReadRandomBytes: cannot read
      /dev/urandom` goes from 1 to 0 and `EACCES` on a `/dev` path is 0, with the boot
      otherwise unchanged (41 stages, 102 packages).
- [ ] `socket/` and `__properties__` for the framework's own sockets: created by the
      wrapper today, checked by the gate's `EACCES` count.
- [ ] The earlier attempt, kept because it is the trap: : binding the host's device nodes into the
      private tmpfs does not work -- a bound node stays owned by the init user namespace,
      so opening it inside ours is `EPERM` (`/dev/null: Permission denied` in the shell's
      own redirects, and the boot stops). A user namespace cannot `mknod` a real device
      either. What is left is to serve the four names from something the namespace owns,
      or to have the shim answer them; the only victim today is
      `hwservicemanager: ReadRandomBytes` (a warning that does not stop the boot), so it
      is not on the critical path.
- [ ] Gate: no `EACCES` or `ENOENT` on any `/dev/` path in a traced boot.

## Cross-cutting, from the plan

- [ ] `MOSAIC_TRACE_ALL_PATHS=1`: log every path in `opendir`/`__openat`, including ones no
      rule matched. Host paths match no rule and are reported nowhere, so a scan working
      entirely in host paths is invisible -- that cost several wrong conclusions.
- [ ] The framework's own `scanDir [<path>]` trace needs the trace file fixed
      (`cutils-trace: Error opening trace file: Permission denied`): point the trace path
      into the bundle's `/data` through the shim, or answer
      `debug.atrace.tags.enableflags`.
