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
