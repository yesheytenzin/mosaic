#!/bin/bash
# Build and inspect a Mosaic runtime bundle straight out of an Android system
# image, using only debugfs. No root, no loop mounts, no container.
#
#   bundle.sh build   <image> <bundle>     # produce a complete, runnable bundle
#   bundle.sh index   <image> <bundle>     # catalog the image's libraries
#   bundle.sh closure <image> <bundle>     # fill in every missing dependency
#   bundle.sh stage   <image> <bundle> <inode> <name> [args...]
#   bundle.sh run     <bundle> <binary> [args...]
#
# `build` is the one that matters: it produces the artifact ADR-0010 describes,
# and it is the input to every later phase. The other subcommands are the
# pieces, useful when a dependency is wrong and one binary needs chasing.
#
# Things that cost real time to discover, all encoded below:
#
#  * /system/lib64 is mostly symlinks into /system/apex/*. Dumping a symlink
#    yields a zero-byte file, reported by the linker as
#    "file offset for the library ... >= file size". Only real files are staged.
#
#  * A dlopen failure can be silent, and the library is named only in an error
#    that never appears. Walking DT_NEEDED with readelf finds those gaps first.
#    Libraries that are dlopen'd rather than declared are seeded by name.
#
#  * The linker config must make the process's *own* namespace the one that is
#    visible, or ART's classloader namespace is a sibling of the namespace
#    holding libc.so, and every later dlopen tries to load a second libc.so.
#    Bionic refuses that ("TLS symbol ... using IE access model") because libc
#    uses initial-exec TLS. Hence one section, named system, default visible.
#
#  * ART reads public.libraries.txt relative to ANDROID_ROOT and preloads every
#    entry, aborting on the first failure. That path is not exercised yet and is
#    left disabled in the bundle (see "Known gaps" in docs/runtime-bundle.md).

set -uo pipefail

cmd="${1:?subcommand}"

# Directories worth searching, most specific first.
search_dirs() {
  echo /system/apex/com.android.runtime/lib64/bionic
  echo /system/apex/com.android.runtime/lib64
  echo /system/apex/com.android.runtime/bin
  echo /system/apex/com.android.art/lib64
  echo /system/apex/com.android.art/bin
  echo /system/apex/com.android.i18n/lib64
  echo /system/apex/com.android.os.statsd/lib64
  echo /system/apex/com.android.adbd/lib64
  echo /system/apex/com.android.conscrypt/lib64
  echo /system/lib64
  echo /system/lib64/hw
}

# Every other apex too, since libraries move between them between releases.
apex_dirs() {
  debugfs -R "ls -l /system/apex" "$1" 2>/dev/null \
    | awk '$1 ~ /^[0-9]+$/ && $NF != "." && $NF != ".." { print $NF }' \
    | while read -r apex; do echo "/system/apex/$apex/lib64"; done
}

do_index() {
  local img="$1" out="$2"
  mkdir -p "$out/index" "$out/lib64" "$out/lib64/bionic"
  : > "$out/index/all.txt"

  {
    search_dirs
    # `debugfs ls` without -l prints a compact form that silently drops
    # entries, so every listing here uses -l and takes the last field.
    apex_dirs "$img"
  } | while read -r dir; do
    key=$(echo "$dir" | tr '/' '_')
    debugfs -R "ls -l $dir" "$img" 2>/dev/null > "$out/index/$key.txt"
    # inode mode (type) links uid gid date time name; mode 12xxxx is a symlink.
    awk -v d="$dir" '
      $1 ~ /^[0-9]+$/ && $NF != "." && $NF != ".." {
        kind = ($2 ~ /^12/) ? "link" : "real"
        print $1, d"/"$NF, kind
      }' "$out/index/$key.txt" >> "$out/index/all.txt"
  done

  echo "index: $(wc -l < "$out/index/all.txt") entries"
}

find_real() { # <bundle> <basename> -> inode of a real file
  awk -v want="$2" '$3 == "real" { n = split($2, p, "/"); if (p[n] == want) { print $1; exit } }' \
    "$1/index/all.txt"
}

dir_of() { awk -v i="$2" '$1 == i { print $2; exit }' "$1/index/all.txt"; }

present() { [ -f "$1/lib64/$2" ] || [ -f "$1/lib64/bionic/$2" ]; }

stage_one() { # <image> <bundle> <basename>
  local img="$1" out="$2" lib="$3" inode where dest
  present "$out" "$lib" && return 0
  inode=$(find_real "$out" "$lib")
  if [ -z "$inode" ]; then
    echo "  NOT IN IMAGE: $lib"
    return 1
  fi
  where=$(dir_of "$out" "$inode")
  case "$where" in */bionic) dest="$out/lib64/bionic" ;; *) dest="$out/lib64" ;; esac
  debugfs -R "dump <$inode> $dest/$lib" "$img" 2>/dev/null >/dev/null
  chmod 644 "$dest/$lib" 2>/dev/null
  echo "  + $lib  ($where)"
}

needed_of() { readelf -d "$1" 2>/dev/null | sed -n 's/.*NEEDED.*\[\(.*\)\]/\1/p'; }

do_closure() {
  local img="$1" out="$2"
  [ -f "$out/index/all.txt" ] || do_index "$img" "$out"

  if [ -f "$out/seed.txt" ]; then
    echo "seeding dlopen'd libraries:"
    while read -r seed; do [ -n "$seed" ] && stage_one "$img" "$out" "$seed"; done < "$out/seed.txt"
  fi

  local round=0
  while :; do
    round=$((round + 1))
    local missing
    missing=$(
      for so in "$out"/lib64/*.so "$out"/lib64/bionic/*.so; do
        [ -f "$so" ] || continue
        for need in $(needed_of "$so"); do present "$out" "$need" || echo "$need"; done
      done | sort -u
    )
    if [ -z "$missing" ]; then
      echo "closure complete after $((round - 1)) rounds"
      break
    fi
    if [ "$round" -gt 25 ]; then
      echo "gave up after 25 rounds; still missing:" >&2
      echo "$missing" | sed 's/^/  /' >&2
      return 1
    fi
    for lib in $missing; do stage_one "$img" "$out" "$lib"; done
  done
  echo "bundle holds $(find "$out/lib64" -name '*.so' | wc -l) libraries"
}

write_linker_config() { # <bundle>
  local out="$1"
  cat > "$out/ld.config.txt" <<EOF
# Linker configuration for a Mosaic runtime bundle.
#
# On a device, linkerconfig generates the equivalent at boot from
# system/etc/linker.config.pb plus system properties. This stands in for it.
#
# One section, named system, covering the whole bundle: the linker creates the
# process's namespaces at startup from the section that matches the executable,
# and the namespace holding libc.so has to be the one ART looks up by name.
# A separate namespace named "system" looks equivalent and is not: ART's
# classloader namespace would be its child, so every dlopen would load a second
# libc.so, which bionic refuses because libc uses initial-exec TLS.
#
# visible = true is what exports the namespace to
# android_get_exported_namespace, which is how ART and libnativebridge ask for
# it by name.
dir.system = $out

[system]
namespace.default.isolated = false
namespace.default.visible = true
namespace.default.search.paths = $out/lib64:$out/lib64/bionic
namespace.default.permitted.paths = $out/lib64:$out/lib64/bionic
EOF
}

stage_payload() { # <image> <bundle> <inode> <name>
  local img="$1" out="$2" inode="$3" name="$4"

  debugfs -R "dump <$inode> $out/bin/$name" "$img" 2>/dev/null >/dev/null
  # Most of /system/bin is symlinks into the apexes, and dumping one yields the
  # link target rather than a program. The index knows which entries are real
  # files, so a non-ELF dump is re-resolved by name.
  if [ "$(head -c 4 "$out/bin/$name" 2>/dev/null | od -An -tx1 | tr -d ' \n')" != "7f454c46" ]; then
    local real_inode
    real_inode=$(find_real "$out" "$name")
    if [ -n "$real_inode" ]; then
      debugfs -R "dump <$real_inode> $out/bin/$name" "$img" 2>/dev/null >/dev/null
    fi
  fi
  if [ "$(head -c 4 "$out/bin/$name" 2>/dev/null | od -An -tx1 | tr -d ' \n')" != "7f454c46" ]; then
    echo "$name is not an ELF file in this image" >&2
    return 1
  fi
  chmod 755 "$out/bin/$name"

  # The linker is libc's NEEDED "ld-android.so" as well as a program, so the
  # same file appears under both names.
  local linker_inode
  linker_inode=$(find_real "$out" linker64)
  if [ -z "$linker_inode" ]; then
    echo "could not find linker64 in the image" >&2
    return 1
  fi
  debugfs -R "dump <$linker_inode> $out/linker64" "$img" 2>/dev/null >/dev/null
  chmod 755 "$out/linker64"
  cp -f "$out/linker64" "$out/lib64/ld-android.so"

  [ -f "$out/ld.config.txt" ] || write_linker_config "$out"

  # Point the binary at our linker instead of /system/bin/linker64: the bundle
  # is not installed at the paths an Android image uses, and the kernel looks up
  # PT_INTERP as an absolute path.
  if command -v patchelf >/dev/null; then
    patchelf --set-interpreter "$out/linker64" "$out/bin/$name"
  else
    echo "warning: patchelf not found; run it as '$out/linker64 $name'" >&2
  fi
}

do_stage() { # <image> <bundle> <inode> <name> [args...]
  local img="$1" out="$2" inode="$3" name="$4"
  shift 4
  [ -f "$out/index/all.txt" ] || do_index "$img" "$out"
  stage_payload "$img" "$out" "$inode" "$name" || return 1

  # Iterate: run it, read the missing library out of the linker's error, stage
  # it, run again. A binary with a large closure needs many rounds, and a
  # library that stages but is still not found would otherwise spin silently, so
  # every round says what it is chasing.
  local attempt
  for attempt in $(seq 1 200); do
    local output status missing
    output=$(do_run "$out" "$name" "$@" 2>&1)
    status=$?
    if [ $status -eq 0 ]; then
      echo "RESOLVED (exit 0)"
      printf '%s\n' "$output"
      return 0
    fi

    missing=$(printf '%s\n' "$output" | sed -n 's/.*library "\([^"]*\)" not found.*/\1/p' | head -1)
    if [ -z "$missing" ]; then
      # dalvikvm dlopens libart.so and reports a null name when that fails, so
      # the missing library has to be inferred rather than read.
      if printf '%s\n' "$output" | grep -q 'initialize JNI invocation API'; then
        echo "  chasing libart.so (dlopen reports no name)"
        stage_one "$img" "$out" libart.so || return 1
        continue
      fi
      echo "UNRESOLVED (not a missing library):"
      printf '%s\n' "$output" | head -25
      return 1
    fi

    echo "  chasing $missing"
    stage_one "$img" "$out" "$missing" || {
      printf '%s\n' "$output" | head -5
      return 1
    }
  done
  echo "gave up after 200 libraries" >&2
  return 1
}

# The classpath and the data files ART needs beyond the shared libraries.
JARS=(
  "/system/apex/com.android.art/javalib/core-oj.jar:javalib"
  "/system/apex/com.android.art/javalib/core-libart.jar:javalib"
  "/system/apex/com.android.i18n/javalib/core-icu4j.jar:javalib"
  "/system/apex/com.android.art/javalib/okhttp.jar:javalib"
  "/system/apex/com.android.art/javalib/bouncycastle.jar:javalib"
  "/system/apex/com.android.art/javalib/apache-xml.jar:javalib"
  "/system/framework/framework.jar:framework"
  "/system/framework/framework-graphics.jar:framework"
  "/system/framework/ext.jar:framework"
  "/system/framework/ims-common.jar:framework"
  "/system/framework/android.hidl.base-V1.0-java.jar:framework"
  "/system/framework/android.hidl.manager-V1.0-java.jar:framework"
)

dump_path() { # <image> <source path> <destination file>
  local img="$1" src="$2" dst="$3" name dir inode
  name=$(basename "$src")
  dir=$(dirname "$src")
  inode=$(debugfs -R "ls -l $dir" "$img" 2>/dev/null \
    | awk -v want="$name" '$1 ~ /^[0-9]+$/ && $NF == want { print $1; exit }')
  if [ -z "$inode" ]; then
    echo "  MISSING: $src" >&2
    return 1
  fi
  debugfs -R "dump <$inode> $dst" "$img" 2>/dev/null >/dev/null
  echo "  + $name -> $dst ($(stat -c %s "$dst") bytes)"
}

do_jars() { # <image> <bundle>
  local img="$1" out="$2" entry src rel
  for entry in "${JARS[@]}"; do
    src="${entry%%:*}"
    rel="${entry##*:}"
    mkdir -p "$out/$rel"
    dump_path "$img" "$src" "$out/$rel/$(basename "$src")"
  done

  # ART loads the ICU data file by name from ANDROID_I18N_ROOT.
  mkdir -p "$out/i18n/etc/icu"
  dump_path "$img" /system/apex/com.android.i18n/etc/icu/icudt70l.dat \
    "$out/i18n/etc/icu/icudt70l.dat"

  # The boot classpath, in the order ART expects it: core libraries, then the
  # framework, then the optional modules.
  {
    local rel
    for rel in core-oj.jar core-libart.jar core-icu4j.jar okhttp.jar \
               bouncycastle.jar apache-xml.jar; do
      printf '%s/javalib/%s:' "$out" "$rel"
    done
    for rel in framework.jar framework-graphics.jar ext.jar ims-common.jar \
               android.hidl.base-V1.0-java.jar android.hidl.manager-V1.0-java.jar; do
      printf '%s/framework/%s:' "$out" "$rel"
    done
  } | sed 's/:$//' > "$out/bootclasspath.txt"
  echo "  + bootclasspath.txt"
}

do_build() { # <image> <bundle>
  local img="$1" out="$2"
  mkdir -p "$out/bin" "$out/lib64" "$out/lib64/bionic" "$out/data/dalvik-cache" "$out/tmp"

  echo "indexing..."
  do_index "$img" "$out"

  echo "staging the linker and the ART binary..."
  local dalvikvm_inode
  dalvikvm_inode=$(debugfs -R "ls -l /system/apex/com.android.art/bin" "$img" 2>/dev/null \
    | awk '$NF == "dalvikvm64" { print $1 }')
  if [ -z "$dalvikvm_inode" ]; then
    echo "could not find dalvikvm64 in the image" >&2
    return 1
  fi
  stage_payload "$img" "$out" "$dalvikvm_inode" dalvikvm64 || return 1

  echo "staging libraries..."
  cat > "$out/seed.txt" <<'SEED'
# Loaded with dlopen, so a DT_NEEDED walk cannot see them.
libart.so
libartbase.so
libartpalette.so
libartpalette-system.so
libart-compiler.so
libdexfile.so
libnativebridge.so
libnativeloader.so
libprofile.so
libnativehelper.so
libicu_jni.so
libjavacore.so
libopenjdk.so
libopenjdkjvm.so
libadbconnection.so
libsigchain.so
SEED

  # ART preloads every entry in the device's public library list, so the bundle
  # has to carry them and their dependencies. The list itself is part of the
  # bundle too: libnativeloader aborts when it cannot read it.
  mkdir -p "$out/etc"
  if dump_path "$img" /system/etc/public.libraries.txt "$out/etc/public.libraries.txt"; then
    grep -vE '^[[:space:]]*#|^[[:space:]]*$' "$out/etc/public.libraries.txt" \
      | grep -v nopreload | awk '{ print $1 }' >> "$out/seed.txt"
  fi

  do_closure "$img" "$out" || return 1

  echo "staging the boot classpath and data files..."
  do_jars "$img" "$out"

  echo "building the property area..."
  do_properties "$img" "$out"

  echo "writing the linker configuration and environment..."
  write_linker_config "$out"
  do_env "$out"
  do_runner "$out"
  # Keep the generator in the bundle so run.sh can regenerate the paths for
  # whatever location the bundle ends up at.
  cp -f "${BASH_SOURCE[0]}" "$out/bundle.sh"
  chmod +x "$out/bundle.sh"

  echo "checking that ART starts..."
  local reported
  reported=$(do_run "$out" dalvikvm64 -XXlib:"$out/lib64/libart.so" -showversion 2>&1 | head -1)
  echo "  $reported"
  case "$reported" in
    *"ART version"*) echo "bundle ready at $out" ;;
    *) echo "bundle built but ART did not report a version" >&2; return 1 ;;
  esac
}

# The environment a Bionic process in this bundle expects. init sets these on a
# device; without them ART cannot find its configuration or a writable data
# directory. The broker applies the same set when it launches an app process,
# so it lives in the bundle rather than in whichever script happens to run it.
#
# This is what the `env` subcommand regenerates, because both files hold
# absolute paths and a moved bundle needs them rewritten.
do_env() {
  local out="$1"
  cat > "$out/env.sh" <<EOF
# Generated by tools/bundle/bundle.sh. Source this before running a Bionic
# binary out of this bundle.
export LD_CONFIG_FILE="$out/ld.config.txt"
export ANDROID_ROOT="$out"
export ANDROID_DATA="$out/data"
export ANDROID_ART_ROOT="$out"
export ANDROID_I18N_ROOT="$out/i18n"
export ANDROID_TZDATA_ROOT="$out"
export ANDROID_TMP="$out/tmp"
EOF
  # app_process reads the boot classpath from the environment, because unlike
  # dalvikvm it takes no -Xbootclasspath argument.
  if [ -f "$out/bootclasspath.txt" ]; then
    printf 'export BOOTCLASSPATH="%s"\n' "$(cat "$out/bootclasspath.txt")" >> "$out/env.sh"
  fi
  echo "  + env.sh"
}

# The wrapper is written once, by build. It is deliberately not regenerated by
# the `env` subcommand: run.sh calls that subcommand, so regenerating run.sh
# there would overwrite the wrapper with whatever copy of this script happens to
# be inside the bundle.
do_runner() {
  local out="$1"
  cat > "$out/run.sh" <<'EOF'
#!/bin/bash
# Run a binary from this bundle: run.sh dalvikvm64 [args...]
#
# The linker configuration and the environment both contain absolute paths, so
# they are regenerated from wherever this bundle now lives. That keeps a copied
# or moved bundle working.
here=$(cd "$(dirname "$0")" && pwd)
"$here/bundle.sh" env "$here" >/dev/null
. "$here/env.sh"
# LD_PRELOAD is applied here rather than inherited, so that it reaches the
# Bionic process and not the shell that is setting it up.
if [ -n "${MOSAIC_PRELOAD:-}" ]; then
  export LD_PRELOAD="$MOSAIC_PRELOAD"
fi
exec "$here/bin/$1" "${@:2}"
EOF
  chmod +x "$out/run.sh"
  echo "  + run.sh"
}

# The property area a Bionic process reads at startup. libc looks in
# /dev/__properties__, which needs root to provision (ADR-0013), so the bundle
# carries the generated files for the privileged step to install rather than
# writing them itself.
do_properties() { # <image> <bundle>
  local img="$1" out="$2" gen build_prop
  gen="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/make-property-area.py"
  if [ ! -f "$gen" ]; then
    echo "no make-property-area.py next to bundle.sh; skipping" >&2
    return 0
  fi
  command -v python3 >/dev/null || {
    echo "python3 not found; skipping the property area" >&2
    return 0
  }

  # The image's build.prop is the authority on product identity and SDK level,
  # so the generator seeds from it and adds the dalvik.vm.* values Mosaic picks.
  mkdir -p "$out/etc"
  build_prop=""
  if dump_path "$img" /system/build.prop "$out/etc/build.prop" >/dev/null 2>&1; then
    build_prop="$out/etc/build.prop"
  fi
  if [ -n "$build_prop" ]; then
    python3 "$gen" "$out/properties" --build-prop "$build_prop" >/dev/null
  else
    python3 "$gen" "$out/properties" >/dev/null
  fi
  echo "  + properties/ ($(ls "$out/properties" | tr '\n' ' '))"
}

do_run() { # <bundle> <binary> [args...]
  local out="$1" name="$2"
  shift 2
  [ -f "$out/env.sh" ] || do_env "$out"
  mkdir -p "$out/data/dalvik-cache" "$out/tmp"
  # shellcheck disable=SC1091
  . "$out/env.sh"

  local interp
  interp=$(readelf -p .interp "$out/bin/$name" 2>/dev/null | sed -n 's/.*\]  //p' | head -1)
  if [ -n "$interp" ] && [ -x "$interp" ]; then
    "$out/bin/$name" "$@"
  else
    "$out/linker64" "$out/bin/$name" "$@"
  fi
}

# Run the boot classpath through dalvikvm, which is how an app process is
# started (ADR-0012).
case "$cmd" in
  index)   do_index "${2:?image}" "${3:?bundle}" ;;
  closure) do_closure "${2:?image}" "${3:?bundle}" ;;
  stage)   do_stage "${2:?image}" "${3:?bundle}" "${4:?inode}" "${5:?name}" "${@:6}" ;;
  jars)    do_jars "${2:?image}" "${3:?bundle}" ;;
  build)   do_build "${2:?image}" "${3:?bundle}" ;;
  env)     write_linker_config "${2:?bundle}"; do_env "${2:?bundle}" ;;
  run)     do_run "${2:?bundle}" "${3:?binary}" "${@:4}" ;;
  *)       sed -n '2,8p' "$0"; exit 2 ;;
esac
