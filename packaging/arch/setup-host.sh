#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Host prerequisites for running Mosaic in container mode on Arch.
#
# Mosaic boots Android inside LXC, which needs the binder kernel driver.
#
# Preferred path: a kernel with binder built in. Arch's official linux-zen
# enables CONFIG_ANDROID_BINDER_IPC and CONFIG_ANDROID_BINDERFS, so no
# out-of-tree module is involved and nothing can conflict:
#
#   sudo pacman -S linux-zen linux-zen-headers
#   reboot into it, then run this script
#
# Fallback path: the stock Arch and Omarchy kernels do not enable binder, so an
# out-of-tree module is installed instead. That module is known to accept
# transactions and never reply on some kernels; packaging/arch/binder-check.sh
# tells you which situation you are in.
#
# ashmem is not required either way; Mosaic falls back to memfd.
#
# Run as your normal user. yay is used for the AUR packages and sudo only for
# configuration files.

set -eu

if [ "$(id -u)" -eq 0 ]; then
    echo "Run this as your normal user, not root: yay must not run as root." >&2
    exit 1
fi

if ! command -v yay >/dev/null 2>&1; then
    echo "yay is required for libgbinder-git (and binder_linux-dkms on the" >&2
    echo "fallback path). Install an AUR helper first." >&2
    exit 1
fi

echo "==> Installing lxc and libgbinder"
yay -S --needed --noconfirm lxc libgbinder-git

if grep -qw binder /proc/filesystems; then
    echo "==> Kernel already provides binder; skipping the out-of-tree module"
    # Make sure the DKMS module never loads on top of the built-in driver.
    sudo install -Dm644 /dev/stdin /etc/modprobe.d/mosaic-binder-dkms.conf <<'EOF'
blacklist binder_linux
EOF
    echo "    binderfs is available. Mosaic creates the device nodes on first start."
else
    echo "==> Kernel has no binder; installing the out-of-tree module"
    yay -S --needed --noconfirm binder_linux-dkms

    echo "==> Configuring the binder module"
    sudo install -Dm644 /dev/stdin /etc/modprobe.d/mosaic-binder.conf <<'EOF'
options binder_linux devices="binder,hwbinder,vndbinder"
EOF
    sudo install -Dm644 /dev/stdin /etc/modules-load.d/mosaic-binder.conf <<'EOF'
binder_linux
EOF

    echo "==> Loading the binder module now"
    sudo modprobe binder_linux

    echo "==> Checking the result"
    for node in /dev/binder /dev/hwbinder /dev/vndbinder; do
        if [ -e "$node" ]; then
            echo "ok: $node"
        else
            echo "MISSING: $node" >&2
            echo "Check 'dkms status' and that linux headers match 'uname -r'." >&2
            exit 1
        fi
    done
fi

cat <<'NOTE'

==> Binder health

The nodes existing is not the same as the driver working. The out-of-tree
module in particular sometimes accepts requests and then never replies,
leaving processes in uninterruptible sleep (D). Verify it with:

    sh packaging/arch/binder-check.sh

If that reports a hang, the driver is at fault, not Mosaic. On the module path
the reliable fix is to switch to a kernel with binder built in:

    sudo pacman -S linux-zen linux-zen-headers

then reboot into it and run this script again.

NOTE

echo
echo "Host prerequisites are ready. From the repo root, next:"
echo "  sudo make install"
echo "  sudo mosaic init"
