// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;

async fn get_session() -> anyhow::Result<std::collections::HashMap<String, String>> {
    crate::helpers::ipc::container_get_session().await
}

async fn container_freeze() -> anyhow::Result<()> {
    crate::helpers::ipc::container_freeze().await
}

async fn container_unfreeze() -> anyhow::Result<()> {
    crate::helpers::ipc::container_unfreeze().await
}

async fn check_session_exists() -> anyhow::Result<()> {
    let session = crate::helpers::ipc::container_get_session().await?;
    if session.is_empty() {
        anyhow::bail!("No session");
    }
    Ok(())
}

pub async fn get(args: &MosaicArgs, key: &str) -> anyhow::Result<()> {
    if check_session_exists().await.is_err() {
        log::error!("Mosaic session is stopped");
        anyhow::bail!("Mosaic session is stopped");
    }

    let session = get_session().await.unwrap_or_default();
    let state = session.get("state").cloned().unwrap_or_default();
    if state == "FROZEN" {
        let _ = container_unfreeze().await;
    }

    if let Some(ret) = crate::helpers::props::get(args, key) {
        println!("{}", ret);
    }

    if state == "FROZEN" {
        let _ = container_freeze().await;
    }
    Ok(())
}

pub async fn set(args: &MosaicArgs, key: &str, value: &str) -> anyhow::Result<()> {
    if check_session_exists().await.is_err() {
        log::error!("Mosaic session is stopped");
        anyhow::bail!("Mosaic session is stopped");
    }

    let session = get_session().await.unwrap_or_default();
    let state = session.get("state").cloned().unwrap_or_default();
    if state == "FROZEN" {
        let _ = container_unfreeze().await;
    }

    crate::helpers::props::set(args, key, value);

    if state == "FROZEN" {
        let _ = container_freeze().await;
    }
    Ok(())
}
