#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# One-shot binder driver check.
#
# The out-of-tree anbox binder sometimes accepts a transaction and never
# replies, leaving the caller in uninterruptible sleep (D). A normal timeout
# cannot kill a task in that state, and `timeout` itself would then block
# waiting for the child. So run the probe in the background, poll for it, and
# give up without waiting if it never finishes.
#
# On a kernel with binder built in (Arch's linux-zen), the device nodes do not
# exist until binderfs is mounted and they are allocated. Run this with sudo in
# that case and it will set them up first.
#
# Exit codes:
#   0  the driver answered
#   1  the driver did not answer (a probe process may be stuck in D)
#   2  the check could not run (no binder, missing library or compiler)
#
#   sh packaging/arch/binder-check.sh          # probe
#   sudo sh packaging/arch/binder-check.sh     # also create the nodes if needed

set -eu

dir=$(mktemp -d)

if ! command -v cc >/dev/null 2>&1; then
    echo "cc is required (install base-devel)." >&2
    exit 2
fi

if ! pkg-config --exists libgbinder; then
    echo "libgbinder is not installed (AUR: libgbinder-git)." >&2
    exit 2
fi

# --- device nodes ---------------------------------------------------------

if [ ! -e /dev/binder ]; then
    if ! grep -qw binder /proc/filesystems; then
        echo "This kernel has no binder support and there is no /dev/binder." >&2
        echo "Install a kernel with binder built in:" >&2
        echo "  sudo pacman -S linux-zen linux-zen-headers" >&2
        exit 2
    fi

    if [ "$(id -u)" -ne 0 ]; then
        echo "binder is available but the device nodes do not exist yet."
        echo "Re-run with sudo so this script can mount binderfs and create them,"
        echo "or just start Mosaic once, which does the same on first start."
        exit 2
    fi

    echo "==> Mounting binderfs and creating the device nodes"
    mkdir -p /dev/binderfs
    mount -t binder binder /dev/binderfs 2>/dev/null || true

    cat >"$dir/nodes.c" <<'C'
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>
#include <linux/android/binderfs.h>

int main(int argc, char **argv) {
    int fd = open("/dev/binderfs/binder-control", O_RDWR);
    if (fd < 0) {
        perror("open /dev/binderfs/binder-control");
        return 1;
    }
    for (int i = 1; i < argc; i++) {
        struct binderfs_device dev;
        memset(&dev, 0, sizeof(dev));
        strncpy(dev.name, argv[i], sizeof(dev.name) - 1);
        /* EEXIST is fine, the node is already there. */
        if (ioctl(fd, BINDER_CTL_ADD, &dev) < 0) {
            perror(argv[i]);
        }
    }
    return 0;
}
C

    cc "$dir/nodes.c" -o "$dir/nodes"
    "$dir/nodes" binder vndbinder hwbinder || true
    for n in binder vndbinder hwbinder; do
        if [ -e "/dev/binderfs/$n" ]; then
            ln -sf "/dev/binderfs/$n" "/dev/$n"
        fi
    done
fi

if [ ! -e /dev/binder ]; then
    echo "/dev/binder still does not exist; cannot probe." >&2
    exit 2
fi

# --- probe ----------------------------------------------------------------

out="$dir/out"

cat >"$dir/check.c" <<'C'
#include <gbinder.h>
#include <stdio.h>

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    GBinderServiceManager *sm = gbinder_servicemanager_new("/dev/binder");
    if (!sm) {
        printf("no service manager object for /dev/binder\n");
        return 2;
    }
    printf("service manager object created\n");
    printf("is_present=%d\n", gbinder_servicemanager_is_present(sm));

    int status = 0;
    GBinderRemoteObject *obj =
        gbinder_servicemanager_get_service_sync(sm, "waydroidplatform", &status);
    printf("waydroidplatform=%s status=%d\n", obj ? "found" : "absent", status);
    return 0;
}
C

# shellcheck disable=SC2046
cc "$dir/check.c" -o "$dir/check" \
    $(pkg-config --cflags --libs libgbinder glib-2.0 gobject-2.0) || {
    echo "failed to compile the probe" >&2
    exit 2
}

"$dir/check" >"$out" 2>&1 &
pid=$!

# Poll for up to about 10 seconds without ever blocking on the child.
i=0
while [ "$i" -lt 20 ]; do
    if ! kill -0 "$pid" 2>/dev/null; then
        break
    fi
    sleep 0.5
    i=$((i + 1))
done

if kill -0 "$pid" 2>/dev/null; then
    echo "DRIVER HUNG: the binder transaction did not return within 10 seconds."
    echo "The probe process is likely stuck in D state; only a reboot clears it."
    echo
    cat "$out" 2>/dev/null || true
    kill -9 "$pid" 2>/dev/null || true
    exit 1
fi

cat "$out"
if grep -q "waydroidplatform=" "$out" 2>/dev/null; then
    echo
    echo "Driver answered. If the service is absent the guest is simply not up yet."
    exit 0
fi

echo
echo "The probe exited without completing a lookup. Treat the driver as suspect." >&2
exit 1
