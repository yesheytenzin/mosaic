#!/bin/bash
# Build and inspect a Mosaic runtime bundle straight out of an Android system
# image, using only debugfs. No root, no loop mounts, no container.
#
#   bundle.sh index   <image> <bundle>
#   bundle.sh closure <image> <bundle> ["linker64 ..." args...]
#   bundle.sh stage   <image> <bundle> <inode> <name> [args...]
#   bundle.sh run     <bundle> <binary> [args...]
#
# This is the tool that answers "what does the runtime bundle actually contain".
# Its output is the input to the real packaging step: the bundle is a directory
# of Bionic and ART libraries plus a linker, a linker config, and the environment
# a Bionic process expects, not an operating system.
#
# The two things that are easy to get wrong and expensive to debug:
#
#  * Android's /system/lib64 is mostly symlinks into /system/apex/*. Dumping a
#    symlink yields a zero-byte file that the linker reports as
#    "file offset for the library ... >= file size". So the index records
#    whether an entry is a real file and only real files are staged.
#
#  * A dlopen failure can be silent, and the library the linker wanted is only
#    named in an error that never appears. Walking DT_NEEDED with readelf finds
#    those gaps first, which is the difference between one command and twenty
#    rounds of guessing. Libraries that are dlopen'd rather than declared are
#    seeded by name in <bundle>/seed.txt.

set -u

cmd="${1:?subcommand}"

# Directories worth searching, most specific first.
search_dirs() {
  echo /system/apex/com.android.runtime/lib64/bionic
  echo /system/apex/com.android.runtime/lib64
  echo /system/apex/com.android.runtime/bin
  echo /system/apex/com.android.art/lib64
  echo /system/apex/com.android.art/bin
  echo /system/lib64
  echo /system/lib64/hw
  echo /system/apex/com.android.i18n/lib64
  echo /system/apex/com.android.conscrypt/lib64
  echo /system/apex/com.android.os.statsd/lib64
  echo /system/apex/com.android.adbd/lib64
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

  echo "index: $(wc -l < "$out/index/all.txt") entries in $out/index/all.txt"
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
      echo "gave up after 25 rounds; still missing:"
      echo "$missing" | sed 's/^/  /'
      return 1
    fi
    for lib in $missing; do stage_one "$img" "$out" "$lib"; done
  done
  echo "bundle holds $(find "$out/lib64" -name '*.so' | wc -l) libraries"
}

write_linker_config() { # <bundle>
  local out="$1"
  cat > "$out/ld.config.txt" <<EOF
# Generated for a Mosaic runtime bundle. Android generates the equivalent at
# boot with linkerconfig, from system/etc/linker.config.pb plus properties;
# this file stands in for that, pointing at the bundle instead of /system.
#
# The linker reads this file at startup for the process it is loading, so it
# has to cover the linker's own path as well as the program's. A file already
# in the bundle is left alone.
dir.bundle = $out

[bundle]
additional.namespaces = system
namespace.default.isolated = false
namespace.default.search.paths = $out/lib64:$out/lib64/bionic
namespace.default.permitted.paths = $out/lib64:$out/lib64/bionic
namespace.system.is_exported = true
namespace.system.search.paths = $out/lib64:$out/lib64/bionic
namespace.system.permitted.paths = $out/lib64:$out/lib64/bionic
EOF
}

do_stage() { # <image> <bundle> <inode> <name> [args...]
  local img="$1" out="$2" inode="$3" name="$4"
  shift 4
  [ -f "$out/index/all.txt" ] || do_index "$img" "$out"

  debugfs -R "dump <$inode> $out/bin/$name" "$img" 2>/dev/null >/dev/null
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

  # Point the binary at our linker instead of /system/bin/linker64. The bundle
  # is not installed at the paths an Android image uses, and the kernel looks up
  # PT_INTERP as an absolute path.
  if command -v patchelf >/dev/null; then
    patchelf --set-interpreter "$out/linker64" "$out/bin/$name"
  else
    echo "warning: patchelf not found; run the binary as '$out/linker64 $name'" >&2
  fi

  local attempt
  for attempt in $(seq 1 40); do
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
      echo "UNRESOLVED (not a missing library):"
      printf '%s\n' "$output" | head -20
      return 1
    fi
    stage_one "$img" "$out" "$missing" || {
      printf '%s\n' "$output" | head -5
      return 1
    }
  done
  echo "gave up after 40 libraries"
  return 1
}

do_run() { # <bundle> <binary> [args...]
  local out="$1" name="$2"
  shift 2
  # A Bionic process expects the environment init would have set. Without
  # ANDROID_ROOT it cannot find its configuration; without ANDROID_DATA it has
  # nowhere to write dalvik-cache.
  export LD_CONFIG_FILE="$out/ld.config.txt"
  export ANDROID_ROOT="$out" ANDROID_DATA="$out/data" ANDROID_ART_ROOT="$out" \
         ANDROID_I18N_ROOT="$out/javalib" ANDROID_TZDATA_ROOT="$out" ANDROID_TMP="$out/tmp"
  mkdir -p "$out/data/dalvik-cache" "$out/tmp"

  # Let the kernel load the binary when its interpreter is inside the bundle,
  # which is what stage does. Otherwise drive the linker by hand, which is how
  # a binary staged by hand with debugfs still runs.
  local interp
  interp=$(readelf -p .interp "$out/bin/$name" 2>/dev/null | sed -n 's/.*\]  //p' | head -1)
  if [ -n "$interp" ] && [ -x "$interp" ]; then
    "$out/bin/$name" "$@"
  else
    "$out/linker64" "$out/bin/$name" "$@"
  fi
}

case "$cmd" in
  index)   do_index "${2:?image}" "${3:?bundle}" ;;
  closure) do_closure "${2:?image}" "${3:?bundle}" ;;
  stage)   do_stage "${2:?image}" "${3:?bundle}" "${4:?inode}" "${5:?name}" "${@:6}" ;;
  run)     do_run "${2:?bundle}" "${3:?binary}" "${@:4}" ;;
  *)       sed -n '2,10p' "$0"; exit 2 ;;
esac
