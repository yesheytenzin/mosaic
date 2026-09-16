// SPDX-License-Identifier: GPL-3.0-or-later

//! The runtime bundle: the pinned, hash-verified build of host-native ART and
//! Bionic that every app process is built from (ADR-0010).
//!
//! It replaces the container images. The download, cache and verification path
//! is the same one the retired port used for `system.img`, which is why it is
//! cheap to introduce.

use crate::args::MosaicArgs;
use crate::config::Defaults;
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

pub fn version_dir(work: &str, version: &str) -> String {
    format!("{}/{}-{}", runtime_dir(work), version, host_arch())
}

/// The version currently extracted, if any.
pub fn installed_version(work: &str) -> Option<String> {
    let dir = runtime_dir(work);
    let marker = format!("{}/version", dir);
    let version = std::fs::read_to_string(marker).ok()?.trim().to_string();
    if version.is_empty() || !Path::new(&version_dir(work, &version)).is_dir() {
        None
    } else {
        Some(version)
    }
}

/// Directory to run from: the extracted bundle, or an error explaining how to
/// get one.
pub fn require(args: &MosaicArgs) -> anyhow::Result<String> {
    match installed_version(&args.work) {
        Some(version) => Ok(version_dir(&args.work, &version)),
        None => anyhow::bail!(
            "no runtime bundle installed. Run 'mosaic runtime fetch' first \
             (this downloads the host-native ART and Bionic build)"
        ),
    }
}

pub fn bundle_url(channel: &str, version: &str) -> String {
    format!(
        "{}/runtime-{}-{}.tar.zst",
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
    let defaults = Defaults::new();
    std::fs::create_dir_all(runtime_dir(&args.work))?;

    let dest = version_dir(&args.work, version);
    if Path::new(&dest).is_dir() {
        log::info!(
            "Runtime bundle {} ({}) already installed",
            version,
            host_arch()
        );
        write_marker(&defaults.work, version)?;
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

    write_marker(&defaults.work, version)?;
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

    #[test]
    fn url_layout_is_stable() {
        let url = bundle_url("https://example.com/mosaic/", "3");
        assert!(url.starts_with("https://example.com/mosaic/runtime-3-"));
        assert!(url.ends_with(".tar.zst"));
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
        assert!(err.contains("mosaic runtime fetch"), "got: {}", err);
    }
}
