// SPDX-License-Identifier: GPL-3.0-or-later

//! The narrow privileged helper (ADR-0008).
//!
//! Its only job is the part that needs root: create the app's system user,
//! create and own its data directory, and delete that directory on uninstall.
//! The broker stays unprivileged and asks for this through `pkexec`, so a bug
//! in IPC routing is not a root bug.
//!
//! The helper is the same binary in a different mode, which keeps one artifact
//! to install and one place to audit.

use crate::args::{MosaicArgs, UidHelperArgs};
use crate::broker::registry::Package;
use anyhow::Context;
use std::process::Command;

fn is_root() -> bool {
    nix::unistd::geteuid().is_root()
}

/// The system user name for a package. Android package names contain dots,
/// which are not valid in a user name.
fn user_name(package: &str) -> String {
    let compact: String = package
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let trimmed = compact.trim_matches('-');
    let name = format!("mosaic-{}", trimmed);
    name.chars().take(31).collect()
}

/// Privileged mode. Runs as root, does one thing, exits.
pub fn run(args: &MosaicArgs, helper: &UidHelperArgs) -> anyhow::Result<()> {
    if !is_root() {
        anyhow::bail!(
            "uid-helper must run as root (it creates system users). \
             Invoke it through the broker, which authorizes with pkexec"
        );
    }
    let _ = args;

    if helper.remove {
        let data_dir = helper
            .data_dir
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("--data-dir is required with --remove"))?;
        remove(helper, data_dir)?;
        return Ok(());
    }

    let uid = helper
        .uid
        .ok_or_else(|| anyhow::anyhow!("--uid is required unless --remove is given"))?;
    let data_dir = helper
        .data_dir
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("--data-dir is required"))?;
    allocate(helper, uid, data_dir)?;
    println!("{}", uid);
    Ok(())
}

fn allocate(helper: &UidHelperArgs, uid: u32, data_dir: &str) -> anyhow::Result<()> {
    let user = user_name(&helper.package);
    ensure_user(&user, uid);
    std::fs::create_dir_all(data_dir).with_context(|| format!("creating {}", data_dir))?;

    if let Some(apk) = &helper.apk {
        let dest = format!("{}/base.apk", data_dir);
        std::fs::copy(apk, &dest).with_context(|| format!("copying {} into {}", apk, data_dir))?;
        chown(&dest, uid)?;
    }

    set_mode(data_dir, 0o700)?;
    chown(data_dir, uid)?;
    log::info!("Allocated uid {} ({}) for {}", uid, user, helper.package);
    Ok(())
}

fn remove(helper: &UidHelperArgs, data_dir: &str) -> anyhow::Result<()> {
    if std::path::Path::new(data_dir).exists() {
        std::fs::remove_dir_all(data_dir).with_context(|| format!("removing {}", data_dir))?;
    }
    let user = user_name(&helper.package);
    // Best effort: the user may never have been created, and a missing user is
    // not a reason to leave the package half removed.
    if is_root() {
        let _ = Command::new("userdel").arg(&user).status();
    }
    log::info!("Removed {} and its user {}", data_dir, user);
    Ok(())
}

fn ensure_user(user: &str, uid: u32) {
    if nix::unistd::User::from_name(user).ok().flatten().is_some() {
        return;
    }
    let status = Command::new("useradd")
        .args([
            "--system",
            "--no-create-home",
            "--user-group",
            "--uid",
            &uid.to_string(),
            user,
        ])
        .status();
    match status {
        Ok(s) if s.success() => log::info!("Created system user {} ({})", user, uid),
        // A missing `useradd` or a UID collision is survivable: the data
        // directory ownership is what actually isolates the app.
        Ok(s) => log::warn!("useradd {} exited {}", user, s),
        Err(e) => log::warn!("could not run useradd for {}: {}", user, e),
    }
}

fn chown(path: &str, uid: u32) -> anyhow::Result<()> {
    let uid = nix::unistd::Uid::from_raw(uid);
    let gid = nix::unistd::Gid::from_raw(uid.as_raw());
    nix::unistd::chown(path, Some(uid), Some(gid))
        .with_context(|| format!("chown {} to {}", path, uid))?;
    Ok(())
}

fn set_mode(path: &str, mode: u32) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

/// Ask the helper to allocate a user for `package`. Called by the broker.
///
/// The broker is unprivileged, so this elevates. `noninteractive` selects
/// `sudo -n` (used by the daemon, which has no session to prompt on) over
/// `pkexec`, which shows the desktop's polkit dialog.
pub fn invoke(
    args: &MosaicArgs,
    package: &str,
    uid: u32,
    apk: Option<&str>,
    data_dir: &str,
    noninteractive: bool,
) -> anyhow::Result<()> {
    let mut command = elevated(noninteractive)?;
    command
        .arg("--work")
        .arg(&args.work)
        .arg("uid-helper")
        .arg(package)
        .arg("--uid")
        .arg(uid.to_string())
        .arg("--data-dir")
        .arg(data_dir);
    if let Some(apk) = apk {
        command.arg("--apk").arg(apk);
    }
    run_helper(command)
}

/// Ask the helper to delete an app's data. Best effort, as the broker treats a
/// failed cleanup as a warning rather than a failed uninstall.
pub fn invoke_remove(args: &MosaicArgs, package: &Package) -> anyhow::Result<()> {
    let mut command = elevated(true)?;
    command
        .arg("--work")
        .arg(&args.work)
        .arg("uid-helper")
        .arg(&package.name)
        .arg("--remove")
        .arg("--data-dir")
        .arg(&package.data_dir);
    run_helper(command)
}

fn elevated(noninteractive: bool) -> anyhow::Result<Command> {
    let exe = std::env::current_exe()?;
    if is_root() {
        return Ok(Command::new(exe));
    }
    if noninteractive {
        let mut command = Command::new("sudo");
        command.arg("-n").arg(exe);
        return Ok(command);
    }
    if which::which("pkexec").is_ok() {
        let mut command = Command::new("pkexec");
        command.arg(exe);
        return Ok(command);
    }
    anyhow::bail!("allocating an app user needs root, and neither pkexec nor sudo is available")
}

fn run_helper(mut command: Command) -> anyhow::Result<()> {
    let output = command.output().context("could not run the uid helper")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "uid helper refused: {}",
            stderr.trim().lines().last().unwrap_or("no output")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_name_is_filesystem_safe() {
        assert_eq!(user_name("com.termux"), "mosaic-com-termux");
        assert_eq!(user_name("org.example.app"), "mosaic-org-example-app");
    }

    #[test]
    fn user_name_is_bounded() {
        let long = "com.example.".repeat(10);
        assert!(user_name(&long).len() <= 31);
    }

    #[test]
    fn removing_missing_data_dir_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("gone");
        let helper = UidHelperArgs {
            package: "com.termux".to_string(),
            uid: None,
            apk: None,
            remove: true,
            data_dir: Some(missing.to_string_lossy().to_string()),
        };
        remove(&helper, helper.data_dir.as_deref().unwrap()).unwrap();
    }
}
