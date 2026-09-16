// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use std::path::Path;

pub fn is_mount(folder: &str) -> bool {
    let real = std::fs::canonicalize(folder)
        .unwrap_or_else(|_| Path::new(folder).to_path_buf())
        .to_string_lossy()
        .to_string();
    let content = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    for line in content.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.len() < 2 {
            continue;
        }
        let mountpoint = words[1];
        if mountpoint == real || words[0] == real {
            return true;
        }
    }
    false
}

pub fn bind(
    args: &MosaicArgs,
    source: &str,
    destination: &str,
    create_folders: bool,
    umount: bool,
) -> anyhow::Result<()> {
    if is_mount(destination) {
        if umount {
            umount_all(args, destination)?;
        } else {
            return Ok(());
        }
    }

    for path in [source, destination] {
        if Path::new(path).exists() {
            continue;
        }
        if create_folders {
            crate::helpers::run::user(
                args,
                &["mkdir".to_string(), "-p".to_string(), path.to_string()],
                "log",
                false,
                Some(true),
            )?;
        } else {
            anyhow::bail!("Mount failed, folder does not exist: {}", path);
        }
    }

    crate::helpers::run::user(
        args,
        &[
            "mount".to_string(),
            "-o".to_string(),
            "bind".to_string(),
            source.to_string(),
            destination.to_string(),
        ],
        "log",
        false,
        Some(true),
    )?;

    if !is_mount(destination) {
        anyhow::bail!("Mount failed: {} -> {}", source, destination);
    }
    Ok(())
}

pub fn bind_file(
    args: &MosaicArgs,
    source: &str,
    destination: &str,
    create_folders: bool,
) -> anyhow::Result<()> {
    if is_mount(destination) {
        return Ok(());
    }
    if !Path::new(destination).exists() {
        if create_folders {
            if let Some(parent) = Path::new(destination).parent() {
                if !parent.exists() {
                    crate::helpers::run::user(
                        args,
                        &[
                            "mkdir".to_string(),
                            "-p".to_string(),
                            parent.to_string_lossy().to_string(),
                        ],
                        "log",
                        false,
                        Some(true),
                    )?;
                }
            }
        }
        crate::helpers::run::user(
            args,
            &["touch".to_string(), destination.to_string()],
            "log",
            false,
            Some(true),
        )?;
    }
    crate::helpers::run::user(
        args,
        &[
            "mount".to_string(),
            "-o".to_string(),
            "bind".to_string(),
            source.to_string(),
            destination.to_string(),
        ],
        "log",
        false,
        Some(true),
    )?;
    Ok(())
}

pub fn umount_all_list(prefix: &str, source: &str) -> anyhow::Result<Vec<String>> {
    let mut ret = Vec::new();
    let real_prefix = std::fs::canonicalize(prefix)
        .unwrap_or_else(|_| Path::new(prefix).to_path_buf())
        .to_string_lossy()
        .to_string();
    let content = std::fs::read_to_string(source)?;
    for line in content.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.len() < 2 {
            anyhow::bail!("Failed to parse line in {}: {}", source, line);
        }
        let mut mountpoint = words[1].to_string();
        if mountpoint.starts_with(&real_prefix) {
            let deleted_str = r"\040(deleted)";
            if mountpoint.ends_with(deleted_str) {
                mountpoint.truncate(mountpoint.len() - deleted_str.len());
            }
            ret.push(mountpoint);
        }
    }
    ret.sort_by(|a, b| b.cmp(a));
    Ok(ret)
}

pub fn umount_all(args: &MosaicArgs, folder: &str) -> anyhow::Result<()> {
    let all_list = umount_all_list(folder, "/proc/mounts")?;
    for mp in &all_list {
        crate::helpers::run::user(
            args,
            &["umount".to_string(), mp.clone()],
            "log",
            false,
            Some(true),
        )?;
    }
    for mp in &all_list {
        if is_mount(mp) {
            anyhow::bail!("Failed to umount: {}", mp);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn mount(
    args: &MosaicArgs,
    source: &str,
    destination: &str,
    create_folders: bool,
    umount: bool,
    readonly: bool,
    mount_type: Option<&str>,
    options: Option<Vec<String>>,
    force: bool,
) -> anyhow::Result<()> {
    if is_mount(destination) {
        if umount {
            umount_all(args, destination)?;
        } else if !force {
            return Ok(());
        }
    }

    if !Path::new(destination).exists() {
        if create_folders {
            crate::helpers::run::user(
                args,
                &[
                    "mkdir".to_string(),
                    "-p".to_string(),
                    destination.to_string(),
                ],
                "log",
                false,
                Some(true),
            )?;
        } else {
            anyhow::bail!("Mount failed, folder does not exist: {}", destination);
        }
    }

    let mut extra = Vec::new();
    let mut opt_args = Vec::new();
    if let Some(t) = mount_type {
        extra.push("-t".to_string());
        extra.push(t.to_string());
    }
    if readonly {
        opt_args.push("ro".to_string());
    }
    if let Some(opts) = options {
        opt_args.extend(opts);
    }
    if !opt_args.is_empty() {
        extra.push("-o".to_string());
        extra.push(opt_args.join(","));
    }

    let mut cmd = vec!["mount".to_string()];
    cmd.extend(extra);
    cmd.push(source.to_string());
    cmd.push(destination.to_string());

    crate::helpers::run::user(args, &cmd, "log", false, Some(true))?;

    if !is_mount(destination) {
        anyhow::bail!("Mount failed: {} -> {}", source, destination);
    }
    Ok(())
}

pub fn mount_overlay(
    args: &MosaicArgs,
    lower_dirs: &[String],
    destination: &str,
    upper_dir: Option<&str>,
    work_dir: Option<&str>,
    create_folders: bool,
    readonly: bool,
) -> anyhow::Result<()> {
    let mut dirs = lower_dirs.to_vec();
    let mut options = vec![format!("lowerdir={}", lower_dirs.join(":"))];

    if let Some(upper) = upper_dir {
        dirs.push(upper.to_string());
        if let Some(work) = work_dir {
            dirs.push(work.to_string());
            options.push(format!("upperdir={}", upper));
            options.push(format!("workdir={}", work));
        }
    }

    // xino=off for kernel >= 4.17
    if let Ok(version) = get_kernel_version() {
        if version >= (4, 17) {
            options.push("xino=off".to_string());
        }
    }

    for dir in &dirs {
        if !Path::new(dir).exists() {
            if create_folders {
                crate::helpers::run::user(
                    args,
                    &["mkdir".to_string(), "-p".to_string(), dir.clone()],
                    "log",
                    false,
                    Some(true),
                )?;
            } else {
                anyhow::bail!("Mount failed, folder does not exist: {}", dir);
            }
        }
    }

    mount(
        args,
        "overlay",
        destination,
        create_folders,
        // Do not umount the destination. The overlay's lowest lowerdir is the
        // destination path itself, so unmounting first would point it at an
        // empty directory. force=true stacks the overlay over the live mount.
        false,
        readonly,
        Some("overlay"),
        Some(options),
        true,
    )
}

fn get_kernel_version() -> anyhow::Result<(u32, u32)> {
    let ver = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .unwrap_or_else(|_| "5.0.0".to_string());
    let parts: Vec<&str> = ver.trim().split('.').collect();
    let major = parts.first().unwrap_or(&"0").parse().unwrap_or(0);
    let minor = parts.get(1).unwrap_or(&"0").parse().unwrap_or(0);
    Ok((major, minor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn umount_all_list_parsing() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "/dev/sda1 / ext4 rw 0 0").unwrap();
        writeln!(f, "/dev/sda1 /var/lib/mosaic/rootfs ext4 rw 0 0").unwrap();
        writeln!(f, "/dev/sda1 /var/lib/mosaic/rootfs/vendor ext4 rw 0 0").unwrap();
        writeln!(f, "tmpfs /tmp tmpfs rw 0 0").unwrap();
        let path = f.path().to_str().unwrap();
        let list = umount_all_list("/var/lib/mosaic/rootfs", path).unwrap();
        assert_eq!(list.len(), 2);
        assert!(list[0].contains("vendor"));
    }

    #[test]
    fn deleted_suffix_removed() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            "/dev/sda1 /var/lib/mosaic/rootfs\\040(deleted) ext4 rw 0 0"
        )
        .unwrap();
        let path = f.path().to_str().unwrap();
        let list = umount_all_list("/var/lib/mosaic/rootfs", path).unwrap();
        assert_eq!(list[0], "/var/lib/mosaic/rootfs");
    }
}
