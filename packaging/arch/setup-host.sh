#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Host prerequisites for running Mosaic in container mode on Arch.
#
# Mosaic boots Android inside LXC, which needs the binder kernel driver. On
# Arch that driver is not in the mainline kernel, so it comes from the
# binder_linux-dkms AUR package and is built against the running kernel's
# headers. ashmem is not required; Mosaic falls back to memfd when /dev/ashmem
# is absent.
#
# Run as your normal user. It uses yay for the AUR package and sudo only for
# the module configuration files.
#
#   sh packaging/arch/setup-host.sh
#
# Then update the binary and initialize:
#
#   sudo make install
#   sudo mosaic init

set -eu

if [ "$(id -u)" -eq 0 ]; then
    echo "Run this as your normal user, not root: yay must not run as root." >&2
    exit 1
fi

if ! command -v yay >/dev/null 2>&1; then
    echo "yay is required for the binder_linux-dkms AUR package." >&2
    echo "Install an AUR helper, or build binder_linux-dkms manually." >&2
    exit 1
fi

echo "==> Installing lxc, binder_linux-dkms and libgbinder"
yay -S --needed --noconfirm lxc binder_linux-dkms libgbinder-git

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
if grep -qw binder /proc/filesystems; then
    echo "ok: binderfs is available"
fi

cat <<'NOTE'

==> Binder health

The nodes existing is not the same as the driver working. The out-of-tree
anbox binder sometimes accepts requests and then never replies, leaving
processes in uninterruptible sleep (D). Check for that after starting a
container:

    ps -eo stat,args | grep -E "^D"        # guest Android processes in D state
    sudo dmesg | grep -iE "hung_task|blocked for more than" | tail

If either shows up, the driver is at fault, not Mosaic. The reliable fix is a
kernel that ships binder in-tree instead of the out-of-tree module:

    yay -S linux-xanmod-anbox linux-xanmod-anbox-headers

NOTE

echo
echo "Host prerequisites are ready. From the repo root, next:"
echo "  sudo make install"
echo "  sudo mosaic init"
