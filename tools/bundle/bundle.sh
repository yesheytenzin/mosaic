#!/bin/bash
# Build and inspect a Mosaic runtime bundle straight out of an Android system
# image, using only debugfs. No root, no loop mounts, no container.
#
#   bundle.sh build   <image> <bundle>     # produce a complete, runnable bundle
#   bundle.sh index   <image> <bundle>     # catalog the image's libraries
#   bundle.sh closure <image> <bundle>     # fill in every missing dependency
#   bundle.sh stage   <image> <bundle> <inode> <name> [args...]
#   bundle.sh run     <bundle> <binary> [args...]
#   bundle.sh compile <bundle> <apk> [filter]   # dex2oat an app's bytecode
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
#    entry. The bundle carries the system list and an explicit empty vendor list
#    so SystemConfig and ART see the same device contract on every build.

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
    # The bundle's own binaries are scanned too. A library that only a *program*
    # needs is invisible to a scan of lib64 alone -- `idmap2d` needs
    # `libidmap2.so` and `libidmap2_policies.so`, and the linker refuses to start
    # it without them ("CANNOT LINK EXECUTABLE ... not found").
    missing=$(
      for so in "$out"/lib64/*.so "$out"/lib64/bionic/*.so "$out"/bin/*; do
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

write_ld_android_stub() { # <bundle>
  local out="$1"
  local target="${MOSAIC_ANDROID_TARGET:-x86_64-linux-android21}"
  local source
  source=$(mktemp /tmp/mosaic-ld-android-XXXXXX.c)
  cat >"$source" <<'EOF'
/* The soname is the whole content. */
EOF
  if clang --target="$target" -shared -fPIC -nostdlib \
      -Wl,-soname,ld-android.so -o "$out/lib64/ld-android.so" "$source" 2>/dev/null; then
    rm -f "$source"
    return 0
  fi
  rm -f "$source"
  echo "warning: could not build the ld-android.so stub; a copy of the linker" >&2
  echo "         here makes the process abort with 'linker cannot load itself'" >&2
  return 1
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

  # libc and libdl_android have DT_NEEDED ld-android.so, and on a device that
  # name is satisfied by the linker itself. Copying the linker binary here does
  # *not* do the same thing: a copy is a second file, so the linker maps it as an
  # ordinary library, runs its constructors out of order, and its own
  # detect_self_exec aborts the process with
  #
  #   error: linker cannot load itself
  #
  # A symlink does not help either -- it is the same inode, and the same abort.
  # What works, and what newer Android does, is a real stub with that soname: the
  # namespace resolves the dependency to this file, nothing of the linker is
  # mapped, and the linker's own symbols still come from the linker.
  write_ld_android_stub "$out"

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
#
# The jar lists are not hardcoded: the image ships them, in an /etc/classpaths
# config per partition and per apex, and derive_classpath is what turns them into
# BOOTCLASSPATH at boot (init.rc does `load_exports /data/system/environ/classpath`).
# Reading them means the bundle matches whatever image it was built from, which is
# how the missing-class wall was finally cleared.
JARS_DATA=(
  "/system/apex/com.android.i18n/etc/icu/icudt70l.dat:i18n/etc/icu"
)

# Resource packages the framework opens by path. AssetManager loads the platform
# resources as /system/framework/framework-res.apk and every other *-res.apk the
# build put there (a LineageOS image adds its own), and it fails to start without
# them: "Failed to create system AssetManager", caused by whichever one is
# missing. They are collected from the image rather than listed, for that reason.

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
  echo "  + $name"
}

# The jar paths named by one classpath config: the entries are protobuf strings
# with a length prefix before the path, so everything up to the first slash goes.
jars_from_config() { # <config file>
  strings -a "$1" 2>/dev/null \
    | grep -oE '(/apex|/system|/vendor)/[^ ]*\.jar' | awk 'NF && !seen[$0]++'
}

# Every classpath config the image has: the system partition's and one per apex.
classpath_configs() { # <image> <bootclasspath|systemserverclasspath>
  local img="$1" want="$2" apex inode
  inode=$(debugfs -R "ls -l /system/etc/classpaths" "$img" 2>/dev/null \
    | awk -v n="$want.pb" '$NF == n { print $1 }')
  [ -n "$inode" ] && debugfs -R "dump <$inode> $out/index/system-$want.pb" "$img" 2>/dev/null >/dev/null
  for apex in $(debugfs -R "ls -l /system/apex" "$img" 2>/dev/null \
      | awk '$1 ~ /^[0-9]+$/ && $NF != "." && $NF != ".." { print $NF }'); do
    inode=$(debugfs -R "ls -l /system/apex/$apex/etc/classpaths" "$img" 2>/dev/null \
      | awk -v n="$want.pb" '$NF == n { print $1 }')
    [ -n "$inode" ] || continue
    debugfs -R "dump <$inode> $out/index/$apex-$want.pb" "$img" 2>/dev/null >/dev/null
  done
}

# Stage the jars a classpath names and write the classpath file for the bundle.
stage_classpath() { # <image> <bootclasspath|systemserverclasspath> <output file>
  local img="$1" want="$2" list="$3" jar base where found
  classpath_configs "$img" "$want"

  # The ART core libraries come first, as they do on a device; the rest follow in
  # the order the configs were read.
  local ordered=""
  for config in "$out/index"/*-"$want".pb; do
    [ -f "$config" ] || continue
    while read -r jar; do
      base=$(basename "$jar")
      case "$base" in core-oj.jar|core-libart.jar|core-icu4j.jar|okhttp.jar|bouncycastle.jar|apache-xml.jar)
        ordered="$ordered $jar" ;; esac
    done < <(jars_from_config "$config")
  done
  for config in "$out/index"/*-"$want".pb; do
    [ -f "$config" ] || continue
    while read -r jar; do ordered="$ordered $jar"; done < <(jars_from_config "$config")
  done

  : > "$list"
  local seen=""
  for jar in $ordered; do
    base=$(basename "$jar")
    case " $seen " in *" $base "*) continue ;; esac
    seen="$seen $base"
    if [ ! -f "$out/framework/$base" ]; then
      where=""
      for candidate in "/system$jar" "$jar" "/system/apex${jar#/apex}"; do
        inode=$(debugfs -R "ls -l $(dirname "$candidate")" "$img" 2>/dev/null \
          | awk -v n="$(basename "$candidate")" '$NF == n { print $1 }')
        if [ -n "$inode" ]; then where="$candidate"; break; fi
      done
      if [ -z "$where" ]; then
        echo "  MISSING from the image: $jar" >&2
        continue
      fi
      dump_path "$img" "$where" "$out/framework/$base" >/dev/null || continue
      echo "  + $base"
    fi
    printf '%s/framework/%s:' "$out" "$base" >> "$list"
  done
  sed -i 's/:$//' "$list"
  echo "  $want: $(tr ':' '\n' < "$list" | wc -l) jars"
}

# The system apps. The package manager scans these and then *requires* them: it
# looks for exactly one privileged app handling ACTION_INSTALL_PACKAGE and aborts
# the boot without one ("There must be exactly one installer; found []"). The
# bundle's root is what the framework sees as /system, so they land at the top
# level, which is where its paths point.
# What this runtime provides, in the form the framework looks for it. Written by
# the bundle rather than taken from the image: it declares the HALs *this side*
# hosts, and a HAL listed here that is missing is worse than one absent, because
# `ServiceManager.waitForDeclaredService` would then wait for it.
do_vintf() { # <bundle>
  local out="$1"
  mkdir -p "$out/etc/vintf"
  cat > "$out/etc/vintf/manifest.xml" <<'XML'
<?xml version="1.0" encoding="utf-8"?>
<manifest version="1.0" type="device">
    <hal format="aidl">
        <name>android.hardware.health</name>
        <version>1</version>
        <fqname>IHealth/default</fqname>
    </hal>
</manifest>
XML
  # Both places the framework reads: the file and the directory of fragments, which
  # is where this image keeps its own HAL declarations.
  mkdir -p "$out/etc/vintf/manifest" "$out/vendor/etc/vintf/manifest"
  cp "$out/etc/vintf/manifest.xml" "$out/etc/vintf/manifest/android.hardware.health-service.xml"
  cp "$out/etc/vintf/manifest.xml" "$out/vendor/etc/vintf/manifest.xml"
  cp "$out/etc/vintf/manifest.xml" "$out/vendor/etc/vintf/manifest/android.hardware.health-service.xml"
  echo "  + etc/vintf/manifest.xml (the health HAL)"
}

do_apps() { # <image> <bundle>
  local img="$1" out="$2" dir
  # The package manager's own record of what it scanned. It is state, not a build
  # product, and a stale one is worse than none: a bundle whose apps changed but
  # whose packages.xml did not makes PMS answer from the old scan and report a
  # package it now has as missing ("Required services extension package is missing").
  rm -rf "$out/data/system/packages.xml" "$out/data/system/packages.list" "$out/data/system/package_cache"
  # The apexes too, and not as an afterthought: this image ships them *flattened*,
  # as directories under /system/apex, which is what makes `ApexManagerFlattenedApex`
  # the implementation the framework uses -- it lists /apex itself and never asks the
  # apex service. Several packages the package manager *requires* live inside them
  # (the extension services and the permission controller), so without this the boot
  # aborts on a package that is right there in the image.
  # The overlay compiler and its daemon. The framework asks for the `idmap`
  # service at boot (the overlay manager creates the system overlays' idmaps), and
  # `idmap2d` is the daemon that registers it -- so it is a *program* the bundle
  # has to carry and run, not something to answer from Rust.
  for tool in idmap2 idmap2d hwservicemanager; do
    dump_path "$img" "/system/bin/$tool" "$out/bin/$tool" || true
    # The same interpreter rewrite the other binaries get. Without it these keep
    # `/system/bin/linker64` in PT_INTERP, which the kernel resolves itself -- no
    # path shim can help -- and the binary fails to start with "cannot execute:
    # required file not found". `idmap2d` doing that is silent: it is the daemon
    # that registers `idmap`, so the service is simply absent and every call to it
    # waits out its timeout.
    if [ -x "$out/bin/$tool" ] && command -v patchelf >/dev/null && [ -x "$out/linker64" ]; then
      patchelf --set-interpreter "$out/linker64" "$out/bin/$tool" 2>/dev/null || true
    fi
  done

  # Apps are not all under /system on a modern image. The Lineage settings
  # provider that DisplayPolicy asks for lives in /system_ext/priv-app, so
  # extracting only /system leaves a real provider missing from the package
  # manager's scan. /system_ext and /product are symlinks in the image; read
  # their canonical /system/... directories, but keep the logical partition
  # names in the bundle because the framework's path rules redirect them.
  for spec in "system:/system" "system_ext:/system/system_ext" "product:/system/product"; do
    partition=${spec%%:*}
    source=${spec#*:}
    for dir in app priv-app; do
      target="$out"
      [ "$partition" = system ] || target="$out/$partition"
      mkdir -p "$target"
      rm -rf "$target/$dir"
      debugfs -R "rdump $source/$dir $target" "$img" >/dev/null 2>&1 || true
      if [ -d "$target/$dir" ]; then
        echo "  + $partition/$dir/ ($(find "$target/$dir" -name '*.apk' 2>/dev/null | wc -l) apks)"
      else
        echo "  --  $partition/$dir/: not in the image, so nothing to scan" >&2
      fi
    done
  done
  rm -rf "$out/apex"
  debugfs -R "rdump /system/apex $out" "$img" >/dev/null 2>&1 || true
  if [ -d "$out/apex" ]; then
    echo "  + apex/ ($(find "$out/apex" -name '*.apk' 2>/dev/null | wc -l) apks)"
  else
    echo "  --  apex/: not in the image, so nothing to scan" >&2
  fi
}

do_jars() { # <image> <bundle>
  local img="$1" out="$2" entry src rel
  mkdir -p "$out/framework"

  echo "  boot classpath, from the image's own configs"
  stage_classpath "$img" bootclasspath "$out/bootclasspath.txt"

  echo "  system server classpath, from the image's own configs"
  stage_classpath "$img" systemserverclasspath "$out/systemserverclasspath.txt"

  # Every resource package in the framework directory.
  local res
  for res in $(debugfs -R "ls -l /system/framework" "$img" 2>/dev/null \
      | awk '$1 ~ /^[0-9]+$/ && $NF ~ /-res\.apk$/ { print $NF }'); do
    dump_path "$img" "/system/framework/$res" "$out/framework/$res"
  done

  # Configuration the framework reads by path: the font configuration is the
  # first thing SystemFonts asks for, and it fails with a NullPointerException in
  # FileInputStream when the file is absent, because the descriptor is null.
  mkdir -p "$out/etc" "$out/etc/sysconfig" "$out/etc/permissions" "$out/fonts"
  local conf
  mkdir -p "$out/etc/compatconfig" \
    "$out/product/etc/sysconfig" "$out/product/etc/permissions" \
    "$out/system_ext/etc/compatconfig" \
    "$out/system_ext/etc/sysconfig" "$out/system_ext/etc/permissions" \
    "$out/odm/etc/sysconfig" "$out/odm/etc/permissions"
  # Compatibility configuration is part of the device contract. The bundle
  # already redirects /system_ext and /apex, but without these XML files the
  # framework's SystemConfig has no arrays to initialize and aborts before
  # PlatformCompat starts.
  for conf in $(debugfs -R "ls -l /system/etc/compatconfig" "$img" 2>/dev/null \
      | awk '$1 ~ /^[0-9]+$/ && $NF ~ /-compat-config\.xml$/ { print $NF }'); do
    dump_path "$img" "/system/etc/compatconfig/$conf" "$out/etc/compatconfig/$conf"
  done
  mkdir -p "$out/system_ext/etc/compatconfig"
  for conf in $(debugfs -R "ls -l /system/system_ext/etc/compatconfig" "$img" 2>/dev/null \
      | awk '$1 ~ /^[0-9]+$/ && $NF ~ /-compat-config\.xml$/ { print $NF }'); do
    dump_path "$img" "/system/system_ext/etc/compatconfig/$conf" \
      "$out/system_ext/etc/compatconfig/$conf"
  done
  for conf in $(debugfs -R "ls -l /system/etc" "$img" 2>/dev/null \
      | awk '$1 ~ /^[0-9]+$/ && $NF ~ /^fonts.*\.xml$/ { print $NF }'); do
    dump_path "$img" "/system/etc/$conf" "$out/etc/$conf"
  done

  # And the fonts those files name, which Typeface.create opens by path.
  local font
  for font in $(debugfs -R "ls -l /system/fonts" "$img" 2>/dev/null \
      | awk '$1 ~ /^[0-9]+$/ && $NF != "." && $NF != ".." { print $NF }'); do
    inode=$(debugfs -R "ls -l /system/fonts" "$img" 2>/dev/null \
      | awk -v n="$font" '$NF == n { print $1 }')
    [ -n "$inode" ] || continue
    debugfs -R "dump <$inode> $out/fonts/$font" "$img" 2>/dev/null >/dev/null
  done
  echo "  + $(ls "$out/fonts" | wc -l) font files"

  # Files the framework reads from the other partitions. The vendor image is
  # optional because a bundle can be built from a system image alone; when it is
  # given, the files the framework refuses to do without come from it --
  # /vendor/etc/public.libraries.txt is fatal to SystemConfig when absent.
  # SystemConfig treats a missing /vendor/etc/public.libraries.txt as fatal, and
  # neither image has one (in this image /vendor lives inside system and only the
  # system list exists). An empty list is valid and says the truth: no vendor
  # libraries are exposed.
  mkdir -p "$out/vendor/etc"
  if [ -n "${MOSAIC_VENDOR_IMAGE:-}" ] && [ -f "${MOSAIC_VENDOR_IMAGE}" ]; then
    dump_path "$MOSAIC_VENDOR_IMAGE" "/etc/public.libraries.txt" \
      "$out/vendor/etc/public.libraries.txt" 2>/dev/null || true
  fi
  if [ ! -s "$out/vendor/etc/public.libraries.txt" ]; then
    printf '# No vendor libraries are exposed by this bundle.\n' \
      > "$out/vendor/etc/public.libraries.txt"
    echo "  + vendor/etc/public.libraries.txt (empty)"
  fi

  for entry in "${JARS_DATA[@]}"; do
    src="${entry%%:*}"
    rel="${entry##*:}"
    mkdir -p "$out/$rel"
    dump_path "$img" "$src" "$out/$rel/$(basename "$src")" || true
  done
}

do_build() { # <image> <bundle>
  local img="$1" out="$2"
  mkdir -p "$out/bin" "$out/lib64" "$out/lib64/bionic" "$out/data/dalvik-cache" "$out/tmp" "$out/index"

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
libstats_jni.so
libandroid_servers.so
libjavacrypto.so
libhwui.so
# The services' own JNI libraries. `System.loadLibrary` reaches these with a
# dlopen, so a DT_NEEDED walk cannot see them, and they are not `lib*_jni.so`
# under an apex either -- they sit in /system/lib64 unremarked. Each one is
# needed by exactly one service:
#
#   libalarm_jni.so        AlarmManagerService   (the first to be asked for)
#   libmedia_jni.so        MediaPlayer, AudioSystem
#   librs_jni.so           RenderScript
#   librtp_jni.so          the RTP stack
#   libdrmframework_jni.so DrmManagerService
#   libaudioeffect_jni.so  AudioEffect
#   libprintspooler_jni.so PrintSpooler
#   libhidcommand_jni.so   the input command queue
#   libuinputcommand_jni.so
#   libnfc_nci_jni.so      NfcService
libalarm_jni.so
libaudioeffect_jni.so
libdrmframework_jni.so
libhidcommand_jni.so
libmedia_jni.so
libnfc_nci_jni.so
libprintspooler_jni.so
librs_jni.so
librtp_jni.so
libuinputcommand_jni.so
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

  echo "staging the system apps..."
  do_apps "$img" "$out"

  echo "declaring the HALs this side hosts..."
  do_vintf "$out"

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

  # Clear the previous area first. Its files are written read-only, the way a
  # device has them, so a second build cannot overwrite them: it fails with
  # "Permission denied" and leaves the *old* area in place, which is worse than
  # failing, because the bundle then says it is ready with properties that no
  # longer match the image. Removing first is allowed even so, because unlinking
  # needs permission on the directory, not on the file.
  rm -f "$out"/properties/* 2>/dev/null || {
    echo "cannot clear $out/properties; remove it and build again" >&2
    return 1
  }

  build_prop=""
  if dump_path "$img" /system/build.prop "$out/etc/build.prop" >/dev/null 2>&1; then
    build_prop="$out/etc/build.prop"
  fi
  # And the status is checked: a bundle whose property area is missing or stale
  # is not a bundle, and everything downstream reads identity out of it.
  if [ -n "$build_prop" ]; then
    python3 "$gen" "$out/properties" --build-prop "$build_prop" >/dev/null || return 1
  else
    python3 "$gen" "$out/properties" >/dev/null || return 1
  fi
  echo "  + properties/ ($(ls "$out/properties" | tr '\n' ' '))"
}

# Pack a bundle into the artifact `mosaic runtime fetch` expects, next to the
# checksum it verifies:
#
#   bundle.sh pack <bundle> [outdir]
#
#   runtime-<version>-<arch>.tar.xz
#   runtime-<version>-<arch>.tar.xz.sha256
#
# The version comes from the bundle's own `version` marker if it has one, so that a
# bundle packed for publishing keeps the name it was installed under. Put both files
# on any host and a different machine installs the runtime with
#
#   MOSAIC_BUNDLE_CHANNEL=https://that.host/somewhere mosaic runtime fetch
#
# Compression is xz because that is what the fetch side decodes; the level is 1 so
# that packing a few hundred megabytes takes a minute rather than ten.
do_pack() { # [--version V] <bundle> [outdir]
  local version_arg="" out="${PWD}" bundle=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --version) version_arg="${2:?--version needs a value}"; shift ;;
      *) if [ -z "$bundle" ]; then bundle="$1"; else out="$1"; fi ;;
    esac
    shift
  done
  [ -n "$bundle" ] || { echo "usage: bundle.sh pack [--version V] <bundle> [outdir]" >&2; return 2; }
  [ -d "$bundle" ] || { echo "no such bundle: $bundle" >&2; return 2; }
  [ -f "$bundle/run.sh" ] || { echo "$bundle does not look like a bundle" >&2; return 2; }

  local version="local" arch
  [ -f "$bundle/version" ] && version=$(tr -d '\n' < "$bundle/version")
  [ -n "$version_arg" ] && version="$version_arg"
  arch=$(uname -m)
  case "$arch" in
    aarch64|arm64) arch=aarch64 ;;
    *) arch=x86_64 ;;
  esac

  mkdir -p "$out"
  local name="runtime-${version}-${arch}"
  echo "packing $bundle as $name.tar.xz"
  # From inside the bundle, so the archive holds the bundle's contents and unpacking
  # it into <work>/runtime/<version>-<arch> gives that directory the bundle's files.
  ( cd "$bundle" && tar -c . ) | xz -1 -T0 > "$out/$name.tar.xz" || return 1
  sha256sum "$out/$name.tar.xz" | awk '{print $1}' > "$out/$name.tar.xz.sha256"
  echo "  $out/$name.tar.xz ($(du -h "$out/$name.tar.xz" | cut -f1))"
  echo "  $out/$name.tar.xz.sha256"
  echo
  echo "A different machine installs it with:"
  echo "  MOSAIC_BUNDLE_CHANNEL=<the directory it is served from> mosaic runtime fetch"
}

# Compile an installed app's bytecode with the bundle's ART. This is the closest
# thing to running a real app that the runtime alone can do: dex2oat loads the
# app's DEX, verifies it against the boot classpath, and with the optimizing
# compiler emits machine code for it. Running the app itself needs the framework
# services, which is a later phase.
do_compile() { # <bundle> <apk> [compiler-filter]
  local out="$1" apk="$2" filter="${3:-speed}"
  if [ ! -f "$apk" ]; then
    echo "no such apk: $apk" >&2
    return 1
  fi
  if [ ! -x "$out/bin/dex2oat64" ]; then
    echo "the bundle has no bin/dex2oat64; stage it first (bundle.sh stage ... dex2oat64)" >&2
    return 1
  fi
  [ -f "$out/env.sh" ] || do_env "$out"

  local work
  work=$(mktemp -d "${TMPDIR:-/tmp}/mosaic-compile.XXXXXX")
  if ! unzip -o -q "$apk" 'classes*.dex' -d "$work"; then
    echo "could not extract dex from $apk" >&2
    return 1
  fi
  local dex_count
  dex_count=$(ls "$work"/classes*.dex 2>/dev/null | wc -l)
  if [ "$dex_count" -eq 0 ]; then
    echo "no classes.dex in $apk" >&2
    return 1
  fi

  local name oat
  name=$(basename "${apk%.apk}")
  oat="$work/$name.oat"

  local bootclasspath
  bootclasspath=$(cat "$out/bootclasspath.txt")

  echo "compiling $dex_count dex file(s) from $(basename "$apk") with filter $filter"
  # shellcheck disable=SC1091
  (
    . "$out/env.sh"
    mkdir -p "$out/data/dalvik-cache" "$out/tmp"
    # One -Xmx that fits: ART commits what this asks for, so a large heap fails
    # the compiler's arena mapping on a busy host.
    "$out/bin/dex2oat64" \
      --dex-file="$work/classes.dex" \
      --dex-location="/data/app/$name/base.apk" \
      --oat-file="$oat" \
      --instruction-set=x86_64 \
      --compiler-filter="$filter" \
      --runtime-arg "-Xbootclasspath:$bootclasspath" \
      --runtime-arg -Xms16m --runtime-arg -Xmx256m
  )
  local status=$?

  if [ $status -ne 0 ] || [ ! -s "$oat" ]; then
    echo "dex2oat failed (status $status)" >&2
    return 1
  fi
  echo "produced $(stat -c %s "$oat") bytes of compiled output at $oat"
  echo "  vdex: $(ls "$work"/*.vdex 2>/dev/null | head -1)"
  return 0
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
  apps)    do_apps "${2:?image}" "${3:?bundle}" ;;
  vintf)   do_vintf "${2:?bundle}" ;;
  closure) do_closure "${2:?image}" "${3:?bundle}" ;;
  stage)   do_stage "${2:?image}" "${3:?bundle}" "${4:?inode}" "${5:?name}" "${@:6}" ;;
  jars)    do_jars "${2:?image}" "${3:?bundle}" ;;
  build)   do_build "${2:?image}" "${3:?bundle}" ;;
  compile) do_compile "${2:?bundle}" "${3:?apk}" "${4:-speed}" ;;
  env)     write_linker_config "${2:?bundle}"; do_env "${2:?bundle}" ;;
  run)     do_run "${2:?bundle}" "${3:?binary}" "${@:4}" ;;
  pack)    do_pack "${2:?bundle}" "${3:-$PWD}" ;;
  *)       sed -n '2,8p' "$0"; exit 2 ;;
esac
