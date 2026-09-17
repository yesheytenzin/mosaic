// SPDX-License-Identifier: GPL-3.0-or-later

//! The runtime bundle: the pinned, hash-verified build of host-native ART and
//! Bionic that every app process is built from (ADR-0010).
//!
//! It replaces the container images. The download, cache and verification path
//! is the same one the retired port used for `system.img`, which is why it is
//! cheap to introduce.

use crate::args::MosaicArgs;
use crate::helpers::http;
use std::path::Path;

/// Host architecture, as it appears in the bundle name.
pub fn host_arch() -> String {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64".to_string(),
        "aarch64" => "arm64".to_string(),
        other => other.to_string(),
    }
}

pub fn runtime_dir(work: &str) -> String {
    format!("{}/runtime", work)
}

/// Where a runtime installed for the whole machine lives.
///
/// The bundle is read-only and identical for every user of a machine, so one copy
/// belongs to all of them rather than one 1.5 GB copy each. An administrator puts
/// it here with `sudo mosaic runtime install` -- root's own work directory is this
/// one -- and every user's launch finds it.
pub fn shared_runtime_dir() -> String {
    std::env::var("MOSAIC_SHARED_RUNTIME").unwrap_or_else(|_| "/var/lib/mosaic/runtime".to_string())
}

/// Every place a bundle may be, nearest first: this user's, then the machine's.
pub fn runtime_dirs(work: &str) -> Vec<String> {
    runtime_dirs_in(work, &shared_runtime_dir())
}

pub fn runtime_dirs_in(work: &str, shared: &str) -> Vec<String> {
    let mine = runtime_dir(work);
    if mine == shared {
        vec![mine]
    } else {
        vec![mine, shared.to_string()]
    }
}

/// The bundle to run from, and which version it is: this user's if they have one,
/// otherwise the machine's.
pub fn resolve(work: &str) -> Option<(String, String)> {
    resolve_in(work, &shared_runtime_dir())
}

/// The same, with the machine-wide directory given rather than read from the
/// environment, so that the decision can be tested without one.
pub fn resolve_in(work: &str, shared: &str) -> Option<(String, String)> {
    for dir in runtime_dirs_in(work, shared) {
        // `continue`, not `?`: a user with no bundle of their own is the ordinary
        // case, and returning early there would hide the machine's completely.
        let version = match std::fs::read_to_string(format!("{}/version", dir)) {
            Ok(version) => version.trim().to_string(),
            Err(_) => continue,
        };
        if version.is_empty() {
            continue;
        }
        let candidate = format!("{}/{}-{}", dir, version, host_arch());
        if Path::new(&candidate).is_dir() {
            return Some((candidate, version));
        }
    }
    None
}

pub fn version_dir(work: &str, version: &str) -> String {
    format!("{}/{}-{}", runtime_dir(work), version, host_arch())
}

/// The version currently installed, if any.
pub fn installed_version(work: &str) -> Option<String> {
    resolve(work).map(|(_, version)| version)
}

/// Where the bundle in use actually is, which is not always under this user's work
/// directory: a machine-wide one is found too.
pub fn installed_dir(work: &str) -> Option<String> {
    resolve(work).map(|(dir, _)| dir)
}

/// Point the work directory at a bundle that already exists on disk.
///
/// A symlink, not a copy: the bundle is the user's, may be large, and may be
/// rebuilt in place -- and a link means a rebuild is picked up without saying so.
/// What is checked is what `require` needs: a bundle with a `run.sh` to launch
/// through and a boot classpath to give ART.
pub fn use_local(work: &str, directory: &str, version: &str) -> anyhow::Result<String> {
    let dir = Path::new(directory);
    for needed in ["run.sh", "bootclasspath.txt"] {
        anyhow::ensure!(
            dir.join(needed).exists(),
            "{} does not look like a runtime bundle: no {}",
            directory,
            needed
        );
    }
    std::fs::create_dir_all(runtime_dir(work))?;
    let link = version_dir(work, version);
    if Path::new(&link).exists() || std::fs::symlink_metadata(&link).is_ok() {
        std::fs::remove_file(&link)?;
    }
    std::os::unix::fs::symlink(dir, &link)?;
    std::fs::write(
        format!("{}/version", runtime_dir(work)),
        format!("{}\n", version),
    )?;
    Ok(link)
}

/// Directory to run from: the extracted bundle, or an error explaining how to
/// get one.
pub fn require(args: &MosaicArgs) -> anyhow::Result<String> {
    require_in(&args.work, &shared_runtime_dir())
}

/// The same, with the machine-wide directory given rather than read from the
/// environment.
pub fn require_in(work: &str, shared: &str) -> anyhow::Result<String> {
    match resolve_in(work, shared) {
        // The path that was found, not the path this user's work directory would
        // have held: a machine-wide bundle is somewhere else, and handing back the
        // user's own path would name a directory that does not exist.
        Some((dir, _)) => Ok(dir),
        None => {
            // Say which of the two things is wrong. A link that exists but points
            // somewhere this process cannot see looks exactly like nothing being
            // installed, and that is not a rare case: the broker runs with
            // PrivateTmp=yes, so a bundle left in /tmp is invisible to it while
            // being perfectly visible to whoever built it.
            let dir = runtime_dir(work);
            let marker = format!("{}/version", dir);
            let hint = match std::fs::read_to_string(&marker) {
                Ok(version) => {
                    let version = version.trim();
                    let link = version_dir(work, version);
                    format!(
                        " {} records version {} and {} does not resolve to a directory -- \
                         if it points into /tmp, this process cannot see it (the broker runs \
                         with a private /tmp); put the bundle somewhere under {}",
                        marker, version, link, dir
                    )
                }
                Err(_) => format!(
                    " Run 'mosaic runtime install', which builds one from this machine's \
                     system image, or ask an administrator to install the machine-wide one: \
                     sudo mosaic runtime install"
                ),
            };
            anyhow::bail!("no runtime bundle installed.{}", hint)
        }
    }
}

pub fn bundle_url(channel: &str, version: &str) -> String {
    format!(
        "{}/runtime-{}-{}.tar.xz",
        channel.trim_end_matches('/'),
        version,
        host_arch()
    )
}

/// The environment a Bionic process in this bundle expects.
///
/// init sets these on a device. Without `ANDROID_ROOT` ART cannot find its
/// configuration; without `ANDROID_DATA` it has nowhere to write
/// `dalvik-cache`. The bundle's own `env.sh` holds the same set, which is what
/// the build tool writes for interactive use.
pub fn bundle_env(dir: &str) -> Vec<(String, String)> {
    let mut env = vec![
        (
            "LD_CONFIG_FILE".to_string(),
            format!("{}/ld.config.txt", dir),
        ),
        ("ANDROID_ROOT".to_string(), dir.to_string()),
        ("ANDROID_DATA".to_string(), format!("{}/data", dir)),
        ("ANDROID_ART_ROOT".to_string(), dir.to_string()),
        ("ANDROID_I18N_ROOT".to_string(), format!("{}/i18n", dir)),
        ("ANDROID_TZDATA_ROOT".to_string(), dir.to_string()),
        ("ANDROID_TMP".to_string(), format!("{}/tmp", dir)),
    ];
    // app_process reads the boot classpath from the environment, unlike
    // dalvikvm which takes it as an argument.
    if let Ok(bootclasspath) = std::fs::read_to_string(format!("{}/bootclasspath.txt", dir)) {
        let bootclasspath = bootclasspath.trim();
        if !bootclasspath.is_empty() {
            env.push(("BOOTCLASSPATH".to_string(), bootclasspath.to_string()));
        }
    }
    env
}

/// Start a program out of the bundle. This is the one place a Bionic process is
/// spawned, so the broker, `runtime verify`, and any later caller agree on the
/// environment and the working directory (ADR-0012).
pub fn run_in_bundle(
    args: &MosaicArgs,
    dir: &str,
    program: &str,
    program_args: &[String],
) -> anyhow::Result<String> {
    for sub in ["data/dalvik-cache", "tmp"] {
        std::fs::create_dir_all(format!("{}/{}", dir, sub))?;
    }

    let cmd = std::iter::once(program.to_string())
        .chain(program_args.iter().cloned())
        .collect::<Vec<String>>();
    let message = format!("$ {}", cmd.join(" "));
    let env = bundle_env(dir);

    // ART writes to stderr and exits non-zero on failure, so the runner is told
    // not to treat a non-zero exit as fatal: the caller decides, having seen
    // what ART printed. Output is captured rather than echoed, so a caller can
    // report it once and in its own words.
    let output =
        crate::helpers::process::core(args, &message, &cmd, None, &env, "log", true, false, false)?;
    Ok(output.unwrap_or_default())
}

/// The path to ART inside a bundle.
pub fn dalvikvm_path(dir: &str) -> String {
    format!("{}/bin/dalvikvm64", dir)
}

/// Download, verify and extract the bundle for `version`.
pub async fn fetch(args: &MosaicArgs, channel: &str, version: &str) -> anyhow::Result<String> {
    std::fs::create_dir_all(runtime_dir(&args.work))?;

    let dest = version_dir(&args.work, version);
    if Path::new(&dest).is_dir() {
        log::info!(
            "Runtime bundle {} ({}) already installed",
            version,
            host_arch()
        );
        write_marker(&args.work, version)?;
        return Ok(dest);
    }

    let url = bundle_url(channel, version);
    log::info!("Fetching runtime bundle {}", url);

    let archive = http::download(args, &url, "runtime", true, false)
        .await?
        .ok_or_else(|| anyhow::anyhow!("runtime bundle not found: {}", url))?;

    let expected = http::retrieve(&format!("{}.sha256", url), None).await.1;
    if !expected.is_empty() {
        let expected = String::from_utf8_lossy(&expected)
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string();
        let actual = http::sha256_file(&archive)?;
        if !expected.is_empty() && expected != actual {
            let _ = std::fs::remove_file(&archive);
            anyhow::bail!(
                "runtime bundle hash mismatch: expected {}, got {}",
                expected,
                actual
            );
        }
        log::info!("Runtime bundle hash verified");
    } else {
        log::warn!("No published hash for {}, skipping verification", url);
    }

    log::info!("Extracting to {}", dest);
    std::fs::create_dir_all(&dest)?;
    let file = std::fs::File::open(&archive)?;
    let decoder = xz2::read::XzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    tar.unpack(&dest)?;

    write_marker(&args.work, version)?;
    Ok(dest)
}

fn write_marker(work: &str, version: &str) -> anyhow::Result<()> {
    std::fs::write(
        format!("{}/version", runtime_dir(work)),
        format!("{}\n", version),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A machine-wide bundle is found by a user who has none of their own, and the
    /// path handed to the launcher is the machine's, not a directory that does not
    /// exist under their work directory.
    #[test]
    fn a_machine_wide_bundle_is_found_too() {
        let shared = tempfile::tempdir().unwrap();
        let version_dir = shared.path().join(format!("shared-{}", host_arch()));
        std::fs::create_dir_all(&version_dir).unwrap();
        std::fs::write(version_dir.join("run.sh"), b"#!/bin/bash\n").unwrap();
        std::fs::write(version_dir.join("bootclasspath.txt"), b"").unwrap();
        std::fs::write(shared.path().join("version"), b"shared\n").unwrap();

        let work = tempfile::tempdir().unwrap();
        let work = work.path().to_str().unwrap();
        assert_eq!(installed_version(work), None, "nothing of the user's own");

        let shared_dir = shared.path().to_str().unwrap();
        assert_eq!(
            resolve_in(work, shared_dir).map(|(_, v)| v),
            Some("shared".to_string())
        );
        assert_eq!(
            resolve_in(work, shared_dir).map(|(d, _)| d),
            Some(version_dir.to_string_lossy().to_string())
        );

        // This user's own bundle takes precedence over the machine's.
        let mine = tempfile::tempdir().unwrap();
        std::fs::write(mine.path().join("run.sh"), b"#!/bin/bash\n").unwrap();
        std::fs::write(mine.path().join("bootclasspath.txt"), b"").unwrap();
        use_local(work, mine.path().to_str().unwrap(), "mine").unwrap();
        assert_eq!(installed_version(work), Some("mine".to_string()));

        // And a user with their own bundle keeps it, machine-wide or not.
        assert_eq!(
            resolve_in(work, shared_dir).map(|(_, v)| v),
            Some("mine".to_string())
        );
    }

    /// A bundle built here is a bundle: `use_local` links it and records the
    /// version, and `require` then finds it.
    #[test]
    fn a_local_bundle_is_used_and_found() {
        let bundle = tempfile::tempdir().unwrap();
        std::fs::write(bundle.path().join("run.sh"), b"#!/bin/bash\n").unwrap();
        std::fs::write(bundle.path().join("bootclasspath.txt"), b"").unwrap();

        let work = tempfile::tempdir().unwrap();
        let work = work.path().to_str().unwrap();
        let link = use_local(work, bundle.path().to_str().unwrap(), "local").unwrap();
        assert_eq!(link, version_dir(work, "local"));
        assert_eq!(installed_version(work), Some("local".to_string()));
        assert!(Path::new(&link).join("run.sh").exists(), "through the link");

        // Something that is not a bundle is refused rather than linked.
        let empty = tempfile::tempdir().unwrap();
        let err = use_local(work, empty.path().to_str().unwrap(), "nope").unwrap_err();
        assert!(err.to_string().contains("does not look like"), "{}", err);
    }

    use super::*;

    #[test]
    fn url_layout_is_stable() {
        let url = bundle_url("https://example.com/mosaic/", "3");
        assert!(url.starts_with("https://example.com/mosaic/runtime-3-"));
        assert!(
            url.ends_with(".tar.xz"),
            "the name has to match the decoder"
        );
    }

    #[test]
    fn missing_runtime_reports_how_to_get_one() {
        use clap::Parser;
        let dir = tempfile::tempdir().unwrap();
        let cli = crate::args::Cli::try_parse_from([
            "mosaic",
            "-w",
            dir.path().to_str().unwrap(),
            "query",
        ])
        .unwrap();
        let args = crate::args::MosaicArgs::from_cli(cli);
        let err = require(&args).unwrap_err().to_string();
        assert!(err.contains("runtime install"), "got: {}", err);
    }
}
