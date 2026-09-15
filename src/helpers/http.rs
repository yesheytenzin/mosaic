// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::Path;

pub async fn retrieve(url: &str) -> anyhow::Result<(u16, Vec<u8>)> {
    let client = reqwest::Client::new();
    let resp = client.get(url).send().await?;
    let status = resp.status().as_u16();
    let body = resp.bytes().await?.to_vec();
    Ok((status, body))
}

pub fn retrieve_blocking(url: &str) -> anyhow::Result<(u16, Vec<u8>)> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(retrieve(url))
}

pub async fn download(
    url: &str,
    filename: &str,
    cache: bool,
    work: &str,
) -> anyhow::Result<String> {
    let dest_dir = if cache {
        format!("{}/cache", work)
    } else {
        "/tmp".to_string()
    };
    std::fs::create_dir_all(&dest_dir)?;
    let dest = format!("{}/{}", dest_dir, filename);

    if Path::new(&dest).exists() && cache {
        return Ok(dest);
    }

    let (status, body) = retrieve(url).await?;
    if status != 200 {
        anyhow::bail!("Failed to download {}: HTTP {}", url, status);
    }
    std::fs::write(&dest, &body)?;
    Ok(dest)
}
