// SPDX-License-Identifier: GPL-3.0-or-later

//! HTTP helpers with the same cache layout and error codes the original used.
//! `retrieve` returns status 200 on success, -1 for a malformed URL, and -2
//! for a connection or transport failure, so callers can distinguish them
//! without exceptions.

use crate::args::MosaicArgs;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;

/// Fetch a URL and return its status and body.
pub async fn retrieve(url: &str, headers: Option<HashMap<String, String>>) -> (i32, Vec<u8>) {
    if reqwest::Url::parse(url).is_err() {
        return (-1, Vec::new());
    }
    let client = reqwest::Client::new();
    let mut request = client.get(url);
    if let Some(headers) = headers {
        for (k, v) in headers {
            request = request.header(k, v);
        }
    }
    match request.send().await {
        Ok(response) => {
            let status = response.status().as_u16() as i32;
            match response.bytes().await {
                Ok(body) => (status, body.to_vec()),
                Err(_) => (status, Vec::new()),
            }
        }
        Err(e) => match e.status() {
            Some(status) => (status.as_u16() as i32, Vec::new()),
            None => (-2, Vec::new()),
        },
    }
}

pub fn retrieve_blocking(url: &str, headers: Option<HashMap<String, String>>) -> (i32, Vec<u8>) {
    crate::helpers::runtime::block_on(retrieve(url, headers))
}

fn cache_path(work: &str, prefix: &str, url: &str) -> String {
    let prefix = prefix.replace('/', "_");
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    let digest = hex::encode(hasher.finalize());
    format!("{}/cache_http/{}_{}", work, prefix, digest)
}

/// Download a URL into the cache. Returns the path, or None on an allowed 404.
///
/// The cache key mirrors the original: `<work>/cache_http/<prefix>_<sha256(url)>`.
pub async fn download(
    args: &MosaicArgs,
    url: &str,
    prefix: &str,
    cache: bool,
    allow_404: bool,
) -> anyhow::Result<Option<String>> {
    let cache_dir = format!("{}/cache_http", args.work);
    std::fs::create_dir_all(&cache_dir)?;
    let path = cache_path(&args.work, prefix, url);

    if Path::new(&path).exists() {
        if cache {
            return Ok(Some(path));
        }
        let _ = std::fs::remove_file(&path);
    }

    log::debug!("Downloading {}", url);

    if reqwest::Url::parse(url).is_err() {
        anyhow::bail!("Failed to download {}: malformed URL", url);
    }
    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to download {}: {}", url, e))?;

    let status = response.status();
    if status.as_u16() == 404 && allow_404 {
        log::warn!("WARNING: file not found: {}", url);
        return Ok(None);
    }
    if !status.is_success() {
        anyhow::bail!("Failed to download {}: HTTP {}", url, status);
    }

    let mut file = std::fs::File::create(&path)?;
    let total = response.content_length();
    let mut received: u64 = 0;
    let mut stream = response.bytes_stream();
    use futures_util::StreamExt;
    let mut last_report = std::time::Instant::now();
    let mb = |bytes: u64| (bytes as f64) / 1_000_000.0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| anyhow::anyhow!("Failed to download {}: {}", url, e))?;
        use std::io::Write;
        file.write_all(&chunk)?;
        received += chunk.len() as u64;
        if last_report.elapsed() >= std::time::Duration::from_secs(2) {
            match total {
                Some(total) => {
                    log::info!("[Downloading] {:.2} MB/{:.2} MB", mb(received), mb(total))
                }
                None => log::info!("[Downloading] {:.2} MB", mb(received)),
            }
            last_report = std::time::Instant::now();
        }
    }
    if let Some(total) = total {
        log::info!("[Downloading] {:.2} MB/{:.2} MB", mb(received), mb(total));
    }

    Ok(Some(path))
}

pub fn download_blocking(
    args: &MosaicArgs,
    url: &str,
    prefix: &str,
    cache: bool,
    allow_404: bool,
) -> anyhow::Result<Option<String>> {
    crate::helpers::runtime::block_on(download(args, url, prefix, cache, allow_404))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_path_is_stable_and_hashed() {
        let a = cache_path("/var/lib/mosaic", "images/system", "https://example.com/x");
        let b = cache_path("/var/lib/mosaic", "images/system", "https://example.com/x");
        assert_eq!(a, b);
        assert!(a.starts_with("/var/lib/mosaic/cache_http/images_system_"));
        let c = cache_path("/var/lib/mosaic", "images/system", "https://example.com/y");
        assert_ne!(a, c, "different urls must get different cache keys");
    }

    #[test]
    fn malformed_url_reports_minus_one() {
        assert_eq!(retrieve_blocking("not a url", None).0, -1);
    }
}
