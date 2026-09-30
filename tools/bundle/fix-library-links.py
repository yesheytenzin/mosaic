#!/usr/bin/env python3
"""Point the bundle's symlinks at the bundle, not at a device's paths.

An Android image stages an apex's libraries as symlinks into the *system* copy:

    /apex/com.android.adbd/lib64/libbase.so -> /system/lib64/libbase.so

because on a device both paths exist and which one wins is whichever the linker is
told to search first. A bundle stages the *targets* (`<bundle>/lib64/libbase.so` is
a real file) and keeps the *links* naming the device's absolute paths, so on a host
every one of them is dangling -- 288 of them in this bundle's library directories
when this was written.

That is not cosmetic. The linker resolves a dependency to a real path and checks
that path against the namespace's permitted directories, and a dangling link
resolves to nothing:

    dlopen failed: library "libnetworkstats.so" not permitted

inside the constructor of the service from `service-connectivity.jar`, on a bundle
whose file `libnetworkstats.so` was there all along.

Each link is rewritten to a *relative* path within the bundle, so moving or copying
the bundle keeps them working. A link whose target is not in the bundle is left
alone and reported: it names a library this bundle does not carry, which is worth
knowing rather than hiding.

    fix-library-links.py <bundle>
"""
import os
import sys
import glob


def target_for(bundle: str, link: str) -> str:
    """Where a device-absolute link target lives inside the bundle."""
    if link.startswith("/system/"):
        return os.path.join(bundle, link[len("/system/") :])
    if link.startswith("/apex/") or link.startswith("/vendor/"):
        return os.path.join(bundle, link[1:])
    return ""


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__.strip().splitlines()[0], file=sys.stderr)
        return 2
    bundle = os.path.abspath(sys.argv[1])
    # Every directory a library can be in, which is not only the library
    # directories: an app's own `lib/<abi>` is a symlink farm into the system
    # copies too (84 of them in this bundle when this was extended).
    directories = [os.path.join(bundle, "lib64"), os.path.join(bundle, "lib")]
    directories += sorted(glob.glob(os.path.join(bundle, "apex", "*", "lib64")))
    directories += sorted(glob.glob(os.path.join(bundle, "apex", "*", "lib")))
    directories += sorted(glob.glob(os.path.join(bundle, "*", "app", "*", "lib", "*")))
    directories += sorted(glob.glob(os.path.join(bundle, "*", "priv-app", "*", "lib", "*")))
    directories += sorted(glob.glob(os.path.join(bundle, "*", "*", "app", "*", "lib", "*")))
    directories += sorted(glob.glob(os.path.join(bundle, "*", "*", "priv-app", "*", "lib", "*")))

    rewritten = 0
    unresolved = []
    for directory in directories:
        for entry in sorted(glob.glob(os.path.join(directory, "*"))):
            if not os.path.islink(entry) or os.path.exists(entry):
                continue
            target = target_for(bundle, os.readlink(entry))
            if not target or not os.path.exists(target):
                unresolved.append((entry, os.readlink(entry)))
                continue
            os.remove(entry)
            os.symlink(os.path.relpath(target, os.path.dirname(entry)), entry)
            rewritten += 1

    print(
        f"  + library links: {rewritten} repointed into the bundle, "
        f"{len(unresolved)} left naming something the bundle does not carry"
    )
    for entry, link in unresolved[:10]:
        print(f"    {os.path.relpath(entry, bundle)} -> {link}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
