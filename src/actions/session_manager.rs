// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use crate::config::SessionDefaults;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use zbus::interface;
use zbus::Connection;

pub struct SessionManagerService {
    args: MosaicArgs,
    quit_tx: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
}

impl SessionManagerService {
    pub fn new(
        args: MosaicArgs,
        quit_tx: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    ) -> Self {
        Self { args, quit_tx }
    }
}

#[interface(name = "id.mosaic.SessionManager")]
impl SessionManagerService {
    async fn stop(&self) -> zbus::fdo::Result<()> {
        // Do stop and quit
        let args_clone = self.args.clone();
        tokio::task::spawn_blocking(move || {
            let _ = do_stop(&args_clone);
            let _ = stop_container(false);
        })
        .await
        .ok();
        // Signal quit
        if let Some(tx) = self.quit_tx.lock().unwrap().take() {
            let _ = tx.send(());
        }
        Ok(())
    }
}

fn session_to_hashmap(session: &SessionDefaults) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("user_name".to_string(), session.user_name.clone());
    m.insert("user_id".to_string(), session.user_id.clone());
    m.insert("group_id".to_string(), session.group_id.clone());
    m.insert("host_user".to_string(), session.host_user.clone());
    m.insert("pid".to_string(), session.pid.clone());
    m.insert("xdg_data_home".to_string(), session.xdg_data_home.clone());
    m.insert(
        "xdg_runtime_dir".to_string(),
        session.xdg_runtime_dir.clone(),
    );
    m.insert(
        "wayland_display".to_string(),
        session.wayland_display.clone(),
    );
    m.insert(
        "pulse_runtime_path".to_string(),
        session.pulse_runtime_path.clone(),
    );
    m.insert("state".to_string(), session.state.clone());
    m.insert("lcd_density".to_string(), session.lcd_density.clone());
    m.insert(
        "background_start".to_string(),
        session.background_start.clone(),
    );
    m.insert(
        "mosaic_user_state".to_string(),
        session.mosaic_user_state.clone(),
    );
    m.insert("mosaic_data".to_string(), session.mosaic_data.clone());
    m
}

pub async fn start(args: MosaicArgs) -> anyhow::Result<()> {
    let session_defaults = SessionDefaults::new();
    let mut session = session_to_hashmap(&session_defaults);

    // Handle WAYLAND_DISPLAY
    let wayland_display = session.get("wayland_display").cloned().unwrap_or_default();
    let wayland_display = if wayland_display == "None" || wayland_display.is_empty() {
        log::warn!("WAYLAND_DISPLAY is not set, defaulting to \"wayland-0\"");
        session.insert("wayland_display".to_string(), "wayland-0".to_string());
        "wayland-0".to_string()
    } else {
        wayland_display
    };

    let wayland_socket_path = if Path::new(&wayland_display).is_absolute() {
        wayland_display.clone()
    } else {
        let xdg_runtime_dir = session.get("xdg_runtime_dir").cloned().unwrap_or_default();
        if xdg_runtime_dir == "None" || xdg_runtime_dir.is_empty() {
            log::error!(
                "XDG_RUNTIME_DIR is not set; please don't start a Mosaic session with 'sudo'!"
            );
            anyhow::bail!("XDG_RUNTIME_DIR not set");
        }
        Path::new(&xdg_runtime_dir)
            .join(&wayland_display)
            .to_string_lossy()
            .to_string()
    };

    if !Path::new(&wayland_socket_path).exists() {
        log::error!(
            "Wayland socket '{}' doesn't exist; are you running a Wayland compositor?",
            wayland_socket_path
        );
        anyhow::bail!("Wayland socket missing");
    }

    let mosaic_data = session.get("mosaic_data").cloned().unwrap_or_default();
    if !Path::new(&mosaic_data).is_dir() {
        std::fs::create_dir_all(&mosaic_data)?;
    }

    let dpi = crate::helpers::props::host_get("ro.sf.lcd_density");
    let dpi = if dpi.is_empty() {
        if let Ok(grid) = std::env::var("GRID_UNIT_PX") {
            if let Ok(v) = grid.parse::<i32>() {
                (v * 20).to_string()
            } else {
                "0".to_string()
            }
        } else {
            "0".to_string()
        }
    } else {
        dpi
    };
    session.insert("lcd_density".to_string(), dpi);
    session.insert("background_start".to_string(), "true".to_string());

    // Call container Start via D-Bus
    if let Err(e) = crate::helpers::ipc::container_start(session.clone()).await {
        if let Some(msg) = e.to_string().lines().last() {
            log::error!("{}", msg);
        }
        log::error!("Mosaic container is not listening");
        std::process::exit(0);
    }

    // Start services
    let session_defaults_for_services = SessionDefaults {
        user_name: session.get("user_name").cloned().unwrap_or_default(),
        user_id: session.get("user_id").cloned().unwrap_or_default(),
        group_id: session.get("group_id").cloned().unwrap_or_default(),
        host_user: session.get("host_user").cloned().unwrap_or_default(),
        pid: session.get("pid").cloned().unwrap_or_default(),
        xdg_data_home: session.get("xdg_data_home").cloned().unwrap_or_default(),
        xdg_runtime_dir: session.get("xdg_runtime_dir").cloned().unwrap_or_default(),
        wayland_display: session.get("wayland_display").cloned().unwrap_or_default(),
        pulse_runtime_path: session
            .get("pulse_runtime_path")
            .cloned()
            .unwrap_or_default(),
        state: session.get("state").cloned().unwrap_or_default(),
        lcd_density: session.get("lcd_density").cloned().unwrap_or_default(),
        background_start: session.get("background_start").cloned().unwrap_or_default(),
        mosaic_user_state: session
            .get("mosaic_user_state")
            .cloned()
            .unwrap_or_default(),
        mosaic_data: session.get("mosaic_data").cloned().unwrap_or_default(),
    };

    crate::services::user_manager::start(&args, &session_defaults_for_services, None).ok();
    crate::services::clipboard_manager::start(&args).ok();
    crate::services::notification_manager::start(&args, &session_defaults_for_services).ok();

    // Run session service
    run_session_service(args, session).await
}

async fn run_session_service(
    args: MosaicArgs,
    _session: HashMap<String, String>,
) -> anyhow::Result<()> {
    let conn = Connection::session().await?;
    // Ensure we own the name (we already requested above, but need to serve)
    // The connection we used earlier for checking is dropped; create new and request again
    // For simplicity, use the same connection for serving
    // We need to handle D-Bus disconnection

    let (quit_tx, quit_rx) = tokio::sync::oneshot::channel::<()>();
    let quit_tx = Arc::new(Mutex::new(Some(quit_tx)));
    let svc = SessionManagerService::new(args.clone(), quit_tx.clone());
    conn.object_server().at("/SessionManager", svc).await?;
    conn.request_name("id.mosaic.Session").await?;

    // Handle signals
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigint = signal(SignalKind::interrupt())?;
        let mut sigterm = signal(SignalKind::terminate())?;
        let mut sighup = signal(SignalKind::hangup())?;
        let mut sigusr1 = signal(SignalKind::user_defined1())?;

        tokio::select! {
            _ = quit_rx => {
                log::info!("Session Stop called via D-Bus");
            },
            _ = sigint.recv() => {
                log::info!("SIGINT received, stopping session");
                let _ = do_stop(&args);
                let _ = stop_container(false);
            },
            _ = sigterm.recv() => {
                log::info!("SIGTERM received, stopping session");
                let _ = do_stop(&args);
                let _ = stop_container(false);
            },
            _ = sighup.recv() => {
                log::info!("SIGHUP received, stopping session");
                let _ = do_stop(&args);
                let _ = stop_container(false);
            },
            _ = sigusr1.recv() => {
                log::info!("SIGUSR1 received, stopping session");
                let _ = do_stop(&args);
            },
            _ = wait_for_disconnect(conn.clone()) => {
                log::info!("D-Bus disconnected, stopping session");
                let _ = do_stop(&args);
                let _ = stop_container(false);
            },
        }
    }
    #[cfg(not(unix))]
    {
        quit_rx.await.ok();
    }

    // Cleanup services on exit
    let _ = do_stop(&args);
    Ok(())
}

async fn wait_for_disconnect(_conn: Connection) {
    // Wait until the connection is closed (peer disconnect)
    // zbus doesn't have a direct disconnect signal, so we just pending
    std::future::pending::<()>().await;
}

pub fn do_stop(args: &MosaicArgs) -> anyhow::Result<()> {
    crate::services::user_manager::stop(args).ok();
    crate::services::clipboard_manager::stop(args).ok();
    crate::services::notification_manager::stop(args).ok();
    Ok(())
}

pub fn stop(_args: &MosaicArgs) -> anyhow::Result<()> {
    // Try D-Bus SessionManager Stop
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let res = rt.block_on(crate::helpers::ipc::session_stop());
    if res.is_ok() {
        return Ok(());
    }
    // Fallback to container Stop via SystemBus
    let _ = rt.block_on(crate::helpers::ipc::container_stop(true));
    Ok(())
}

fn stop_container(quit_session: bool) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let _ = rt.block_on(crate::helpers::ipc::container_stop(quit_session));
    Ok(())
}
