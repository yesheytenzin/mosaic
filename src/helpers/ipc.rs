// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::HashMap;
use zbus::Connection;

#[zbus::proxy(
    interface = "id.mosaic.ContainerManager",
    default_service = "id.mosaic.Container",
    default_path = "/ContainerManager"
)]
trait ContainerManager {
    fn start(&self, session: HashMap<String, String>) -> zbus::Result<()>;
    fn stop(&self, quit_session: bool) -> zbus::Result<()>;
    fn freeze(&self) -> zbus::Result<()>;
    fn unfreeze(&self) -> zbus::Result<()>;
    fn get_session(&self) -> zbus::Result<HashMap<String, String>>;
}

#[zbus::proxy(
    interface = "id.mosaic.SessionManager",
    default_service = "id.mosaic.Session",
    default_path = "/SessionManager"
)]
trait SessionManager {
    fn stop(&self) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "id.mosaic.Initializer",
    default_service = "id.mosaic.Container",
    default_path = "/Initializer"
)]
trait Initializer {
    fn init(&self, params: HashMap<String, String>) -> zbus::Result<()>;
    fn cancel(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn progress_changed(&self, message: String) -> zbus::Result<()>;

    #[zbus(signal)]
    fn finished(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn interrupted(&self) -> zbus::Result<()>;
}

pub async fn container_start(session: HashMap<String, String>) -> anyhow::Result<()> {
    let conn = Connection::system().await?;
    let proxy = ContainerManagerProxy::new(&conn).await?;
    proxy
        .start(session)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(())
}

pub async fn container_stop(quit_session: bool) -> anyhow::Result<()> {
    let conn = Connection::system().await?;
    let proxy = ContainerManagerProxy::new(&conn).await?;
    proxy
        .stop(quit_session)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(())
}

pub async fn container_freeze() -> anyhow::Result<()> {
    let conn = Connection::system().await?;
    let proxy = ContainerManagerProxy::new(&conn).await?;
    proxy.freeze().await.map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(())
}

pub async fn container_unfreeze() -> anyhow::Result<()> {
    let conn = Connection::system().await?;
    let proxy = ContainerManagerProxy::new(&conn).await?;
    proxy
        .unfreeze()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(())
}

pub async fn container_get_session() -> anyhow::Result<HashMap<String, String>> {
    let conn = Connection::system().await?;
    let proxy = ContainerManagerProxy::new(&conn).await?;
    let session = proxy
        .get_session()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(session)
}

pub async fn session_stop() -> anyhow::Result<()> {
    let conn = Connection::session().await?;
    let proxy = SessionManagerProxy::new(&conn).await?;
    proxy.stop().await.map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(())
}

pub async fn initializer_init(params: HashMap<String, String>) -> anyhow::Result<()> {
    let conn = Connection::system().await?;
    let proxy = InitializerProxy::new(&conn).await?;
    proxy
        .init(params)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(())
}

pub async fn initializer_cancel() -> anyhow::Result<()> {
    let conn = Connection::system().await?;
    let proxy = InitializerProxy::new(&conn).await?;
    proxy.cancel().await.map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(())
}

pub fn dbus_container_service_path() -> &'static str {
    "/ContainerManager"
}

pub fn dbus_container_interface() -> &'static str {
    "id.mosaic.ContainerManager"
}

pub fn get_session_via_dbus_blocking() -> anyhow::Result<HashMap<String, String>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(container_get_session())
}
