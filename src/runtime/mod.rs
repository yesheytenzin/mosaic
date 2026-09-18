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

/// Headers a bundle download should carry.
///
/// A release in a *private* repository answers 404 to anything unauthenticated, which
/// looks exactly like a release that does not exist. A deployment that keeps its
/// runtime internal sets MOSAIC_BUNDLE_TOKEN and can fetch it; one that publishes it
/// needs no token at all.
pub fn bundle_headers() -> Option<std::collections::HashMap<String, String>> {
    let token = std::env::var("MOSAIC_BUNDLE_TOKEN")
        .ok()
        .filter(|token| !token.is_empty())?;
    let mut headers = std::collections::HashMap::new();
    headers.insert("Authorization".to_string(), format!("Bearer {}", token));
    Some(headers)
}

/// The same, asking for an asset's *bytes*.
///
/// An API asset URL answers with a JSON description of the asset unless this is asked
/// for, and the release *description* answers 404 if it is -- which is exactly the
/// mistake that made the API path look broken: one header set for two different things.
pub fn bundle_asset_headers() -> Option<std::collections::HashMap<String, String>> {
    let mut headers = bundle_headers()?;
    headers.insert("Accept".to_string(), "application/octet-stream".to_string());
    Some(headers)
}

/// Where an asset lives.
///
/// Over a release in a private repository, github.com's download URLs are not reachable
/// at all: they are browser-facing and answer 404 to a token as well. The API is the way
/// in, so it is used whenever a token is set and the channel names a release. Otherwise
/// the plain URL, which is what a public release serves and what a directory does.
pub async fn asset_url(channel: &str, version: &str, asset: &str) -> anyhow::Result<String> {
    if bundle_headers().is_some() {
        if let Some((repo, release)) = github_release(channel) {
            if let Some(url) = api_asset_url(&repo, &release, asset).await? {
                log::info!("Resolved {} through the GitHub API", asset);
                return Ok(url);
            }
        }
    }
    let _ = version;
    Ok(format!("{}/{}", channel.trim_end_matches('/'), asset))
}

/// The repository and release a channel names, when it names one.
///
/// Three shapes reach the same place: a release's `latest/download` directory, its
/// `download/<tag>` directory, and the API's own release URL. The download forms work
/// for a public repository and not for a private one; the API form works for both.
fn github_release(channel: &str) -> Option<(String, String)> {
    let rest = channel
        .strip_prefix("https://github.com/")
        .or_else(|| channel.strip_prefix("https://api.github.com/repos/"))?;
    let rest = rest.strip_prefix("repos/").unwrap_or(rest);
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() < 3 || parts[2] != "releases" {
        return None;
    }
    let repo = format!("{}/{}", parts[0], parts[1]);
    let release = match parts.get(3)? {
        &"latest" => "latest".to_string(),
        // `download/<tag>` and `tags/<tag>` both name a release by its tag.
        &"download" | &"tags" => parts.get(4)?.to_string(),
        other => other.to_string(),
    };
    Some((repo, release))
}

/// A release asset's own URL, through the API. That URL is the only one that serves a
/// private repository's assets, and it needs `Accept: application/octet-stream`.
async fn api_asset_url(repo: &str, release: &str, asset: &str) -> anyhow::Result<Option<String>> {
    // A release is addressed three ways and only one of them is bare: `latest`, a
    // numeric id, and a tag -- and a tag lives under `/tags/`. Asking for
    // `/releases/<tag>` reads the tag as an id and answers 404, which looks exactly
    // like a release that is not there.
    let release = match release {
        "latest" => "latest".to_string(),
        tag if tag.parse::<u64>().is_ok() => tag.to_string(),
        tag => format!("tags/{}", tag),
    };
    let endpoint = format!("https://api.github.com/repos/{}/releases/{}", repo, release);
    let (status, body) = crate::helpers::http::retrieve(&endpoint, bundle_headers()).await;
    if status != 200 || body.is_empty() {
        return Ok(None);
    }
    let release: serde_json::Value = serde_json::from_slice(&body)?;
    let found = release
        .get("assets")
        .and_then(|assets| assets.as_array())
        .and_then(|assets| {
            assets
                .iter()
                .find(|a| a.get("name").and_then(|n| n.as_str()) == Some(asset))
        })
        .and_then(|a| a.get("url").and_then(|u| u.as_str()))
        .map(|u| u.to_string());
    Ok(found)
}

/// Check a downloaded file against the sha256 published beside it, if one was.
///
/// The same check `runtime fetch` makes, for the same reason: a runtime that arrives
/// over a network is the one thing here worth verifying. A sidecar that does not
/// exist is not a failure -- not every host publishes one -- but a sidecar that
/// disagrees is.
/// Where a URL that names an asset actually lives, and where its checksum does.
///
/// For a public release, or any plain directory, those are the URL itself and
/// `<url>.sha256`. For a release in a private repository github.com serves neither, so
/// both are resolved through the API, which is the only thing that serves them.
pub async fn reachable(url: &str) -> anyhow::Result<(String, Option<String>)> {
    let name = url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let channel = url[..url.len().saturating_sub(name.len())].trim_end_matches('/');
    if bundle_headers().is_some() && github_release(channel).is_some() {
        let archive = asset_url(channel, "", &name).await?;
        let sidecar = asset_url(channel, "", &format!("{}.sha256", name))
            .await
            .ok();
        return Ok((archive, sidecar));
    }
    Ok((url.to_string(), Some(format!("{}.sha256", url))))
}

/// Check a downloaded file against the sha256 published beside it, when there is one.
pub async fn verify_checksum(sidecar: Option<&str>, file: &str) -> anyhow::Result<()> {
    let Some(sidecar) = sidecar else {
        return Ok(());
    };
    let (status, body) = crate::helpers::http::retrieve(sidecar, bundle_asset_headers()).await;
    if status != 200 || body.is_empty() {
        return Ok(());
    }
    let expected = String::from_utf8_lossy(&body)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    if expected.is_empty() {
        return Ok(());
    }
    let actual = crate::helpers::http::sha256_file(file)?;
    anyhow::ensure!(
        actual == expected,
        "{} does not match the published hash\n  published {}\n  downloaded {}",
        sidecar,
        expected,
        actual
    );
    log::info!("Runtime bundle hash verified");
    Ok(())
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
                Err(_) => " Run 'mosaic runtime install', which builds one from this \
                            machine's system image, or ask an administrator to install the \
                            machine-wide one: sudo mosaic runtime install"
                    .to_string(),
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

    // Over a release in a private repository, github.com's download URLs are not
    // reachable at all: they are browser-facing and answer 404 to a token as well.
    // The API is the way in, and it is used whenever a token is set and the channel
    // names a release -- otherwise the plain URL, which is what a public release wants.
    let asset = format!("runtime-{}-{}.tar.xz", version, host_arch());
    let url = asset_url(channel, version, &asset).await?;
    log::info!("Fetching runtime bundle {}", url);

    let archive = http::download_with(args, &url, "runtime", true, false, bundle_asset_headers())
        .await?
        .ok_or_else(|| anyhow::anyhow!("runtime bundle not found: {}", url))?;

    // The sidecar is resolved the same way the archive was: over the API when the
    // channel is a private release, because there is no URL to append `.sha256` to.
    let sidecar = format!("{}.sha256", asset);
    let expected = match asset_url(channel, version, &sidecar).await {
        Ok(sidecar_url) => http::retrieve(&sidecar_url, bundle_asset_headers()).await.1,
        Err(_) => Vec::new(),
    };
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

    unpack_archive_into(&archive, &dest)?;
    write_marker(&args.work, version)?;
    Ok(dest)
}

/// Unpack a bundle archive into the place a version lives, checking its sha256 when
/// one was published beside it. Shared by `runtime fetch` and `runtime install <url>`,
/// which are the same operation reached two ways.
pub fn unpack_archive(args: &MosaicArgs, archive: &str, version: &str) -> anyhow::Result<String> {
    let dest = version_dir(&args.work, version);
    unpack_archive_into(archive, &dest)?;
    write_marker(&args.work, version)?;
    Ok(dest)
}

fn unpack_archive_into(archive: &str, dest: &str) -> anyhow::Result<()> {
    log::info!("Extracting to {}", dest);
    std::fs::create_dir_all(dest)?;
    let file = std::fs::File::open(archive)?;
    let decoder = xz2::read::XzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    tar.unpack(dest)?;
    Ok(())
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

    /// A channel names a repository and a release in three shapes, and the parser has
    /// to see all of them: the two browser-facing download directories work for a
    /// public repository, and the API form is the one that works for a private one.
    #[test]
    fn a_channel_names_a_repository_and_a_release() {
        for channel in [
            "https://github.com/me/mosaic/releases/latest/download",
            "https://github.com/me/mosaic/releases/download/runtime-local-x86_64",
            "https://api.github.com/repos/me/mosaic/releases/latest",
            "https://api.github.com/repos/me/mosaic/releases/tags/runtime-local-x86_64",
        ] {
            assert!(
                github_release(channel).is_some(),
                "{} should name a release",
                channel
            );
        }
        assert_eq!(
            github_release("https://github.com/me/mosaic/releases/latest/download"),
            Some(("me/mosaic".to_string(), "latest".to_string()))
        );
        assert_eq!(
            github_release("https://github.com/me/mosaic/releases/download/v1"),
            Some(("me/mosaic".to_string(), "v1".to_string()))
        );
        assert_eq!(
            github_release("https://api.github.com/repos/me/mosaic/releases/tags/v1"),
            Some(("me/mosaic".to_string(), "v1".to_string()))
        );
        // A directory or any other host is not a release.
        assert_eq!(github_release("/var/lib/mosaic/published"), None);
        assert_eq!(github_release("https://example.org/mosaic"), None);
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
