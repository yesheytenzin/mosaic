// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use crate::config::SessionDefaults;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use zbus::object_server::SignalContext;
use zbus::Connection;

static PREPARED_DRIVERS: Mutex<bool> = Mutex::new(false);

fn set_permissions(
    args: &MosaicArgs,
    perm_list: Option<Vec<String>>,
    mode: &str,
) -> anyhow::Result<()> {
    let default_list = vec![
        "/dev/ashmem".to_string(),
        "/dev/sw_sync".to_string(),
        "/sys/kernel/debug/sync/sw_sync".to_string(),
        "/dev/Vcodec".to_string(),
        "/dev/MTK_SMI".to_string(),
        "/dev/mdp_sync".to_string(),
        "/dev/mtk_cmdq".to_string(),
        "/dev/graphics".to_string(),
        "/dev/pvr_sync".to_string(),
        "/dev/ion".to_string(),
    ];
    let list = perm_list.unwrap_or_else(|| {
        let mut l = default_list;
        for p in glob::glob("/dev/dri/renderD*")
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
        {
            l.push(p.to_string_lossy().to_string());
        }
        for p in glob::glob("/dev/fb*")
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
        {
            l.push(p.to_string_lossy().to_string());
        }
        for p in glob::glob("/dev/video*")
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
        {
            l.push(p.to_string_lossy().to_string());
        }
        for p in glob::glob("/dev/dma_heap/*")
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
        {
            l.push(p.to_string_lossy().to_string());
        }
        l
    });

    for path in list {
        if Path::new(&path).exists() {
            let _ = crate::helpers::run::user(
                args,
                &[
                    "chmod".to_string(),
                    mode.to_string(),
                    "-R".to_string(),
                    path,
                ],
                "log",
                false,
                Some(false),
            );
        }
    }
    Ok(())
}

fn prepare_drivers_once(args: &MosaicArgs) -> anyhow::Result<()> {
    let mut prepared = PREPARED_DRIVERS.lock().unwrap();
    if *prepared {
        return Ok(());
    }
    let cfg = crate::config::load(&args.config);
    let vendor_type = cfg
        .mosaic
        .get("vendor_type")
        .cloned()
        .unwrap_or_else(|| "MAINLINE".to_string());
    if vendor_type == "MAINLINE" {
        crate::helpers::drivers::probe_binder_driver(args).ok();
        crate::helpers::drivers::probe_ashmem_driver(args);
    }
    // Ensure args cache has binder nodes loaded
    let mut args_clone = args.clone();
    // The args cache is read-only here, so load the binder node names from config.
    let _ = crate::helpers::drivers::load_binder_nodes(&mut args_clone);
    // Set permissions for binder nodes if available
    if let Some(binder) = cfg.mosaic.get("binder") {
        let _ = set_permissions(
            args,
            Some(vec![
                format!("/dev/{}", binder),
                format!(
                    "/dev/{}",
                    cfg.mosaic
                        .get("vndbinder")
                        .cloned()
                        .unwrap_or_else(|| "vndbinder".to_string())
                ),
                format!(
                    "/dev/{}",
                    cfg.mosaic
                        .get("hwbinder")
                        .cloned()
                        .unwrap_or_else(|| "hwbinder".to_string())
                ),
            ]),
            "666",
        );
    }
    *prepared = true;
    Ok(())
}

pub fn do_start(args: &MosaicArgs, session: &HashMap<String, String>) -> anyhow::Result<()> {
    if !crate::actions::initializer::is_initialized(args) {
        anyhow::bail!("Mosaic is not initialized");
    }

    prepare_drivers_once(args)?;
    log::info!("Starting up container for a new session");

    let tools_src = crate::config::Defaults::tools_src();
    let net_sh = format!("{}/data/scripts/mosaic-net.sh", tools_src);
    crate::helpers::run::user(
        args,
        &[net_sh, "start".to_string()],
        "log",
        false,
        Some(true),
    )?;

    if which::which(crate::guest::SENSORD_BIN).is_ok() {
        let binder = crate::config::load(&args.config)
            .mosaic
            .get("binder")
            .cloned()
            .unwrap_or_else(|| "binder".to_string());
        crate::helpers::run::user(
            args,
            &[
                crate::guest::SENSORD_BIN.to_string(),
                format!("/dev/{}", binder),
            ],
            "background",
            false,
            Some(false),
        )?;
    }

    if which::which("start").is_ok() {
        let _ = crate::helpers::run::user(
            args,
            &["start".to_string(), "cgroup-lite".to_string()],
            "log",
            false,
            Some(false),
        );
    }

    if Path::new("/sys/fs/cgroup/schedtune").exists()
        && Path::new("/sys/fs/cgroup/schedtune").is_dir()
    {
        if std::fs::create_dir("/sys/fs/cgroup/schedtune/probe0").is_err() {
            let _ = crate::helpers::run::user(
                args,
                &[
                    "umount".to_string(),
                    "-l".to_string(),
                    "/sys/fs/cgroup/schedtune".to_string(),
                ],
                "log",
                false,
                Some(false),
            );
        } else {
            let _ = std::fs::create_dir("/sys/fs/cgroup/schedtune/probe0/probe1");
            let _ = std::fs::remove_dir("/sys/fs/cgroup/schedtune/probe0/probe1");
            let _ = std::fs::remove_dir("/sys/fs/cgroup/schedtune/probe0");
        }
    }

    if which::which("stop").is_ok() {
        let _ = crate::helpers::run::user(
            args,
            &["stop".to_string(), "nfcd".to_string()],
            "log",
            false,
            Some(false),
        );
    } else if which::which("systemctl").is_ok() {
        let check = crate::helpers::run::user(
            args,
            &[
                "systemctl".to_string(),
                "is-active".to_string(),
                "-q".to_string(),
                "nfcd".to_string(),
            ],
            "log",
            false,
            Some(false),
        )
        .is_ok();
        if check {
            let _ = crate::helpers::run::user(
                args,
                &[
                    "systemctl".to_string(),
                    "stop".to_string(),
                    "nfcd".to_string(),
                ],
                "log",
                false,
                Some(false),
            );
        }
    }

    set_permissions(args, None, "777")?;

    // Create session-specific LXC config
    let session_defaults = SessionDefaults {
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
    crate::helpers::lxc::generate_session_lxc_config(args, &session_defaults)?;

    // Backwards compatibility: check config contains config_session
    if let Ok(content) = std::fs::read_to_string(format!("{}/lxc/mosaic/config", args.work)) {
        if !content.contains("config_session") {
            crate::helpers::mount::bind(
                args,
                &session_defaults.mosaic_data,
                &crate::config::Defaults::new().data,
                true,
                false,
            )?;
        }
    }

    let cfg = crate::config::load(&args.config);
    let images_path = cfg
        .mosaic
        .get("images_path")
        .cloned()
        .unwrap_or_else(|| format!("{}/images", args.work));
    crate::helpers::images::mount_rootfs(args, &images_path, &session_defaults)?;
    crate::helpers::protocol::set_aidl_version(args)?;
    crate::helpers::lxc::start(args)?;
    crate::services::hardware_manager::start(args)?;

    Ok(())
}

pub fn stop(
    args: &MosaicArgs,
    quit_session: bool,
    session: Option<HashMap<String, String>>,
) -> anyhow::Result<()> {
    if !crate::actions::initializer::is_initialized(args) {
        anyhow::bail!("Mosaic is not initialized");
    }
    log::info!("Stopping container");
    crate::services::hardware_manager::stop(args).ok();
    let status = crate::helpers::lxc::status(args);
    if status != "STOPPED" {
        crate::helpers::lxc::stop(args)?;
        while crate::helpers::lxc::status(args) != "STOPPED" {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }

    let tools_src = crate::config::Defaults::tools_src();
    let net_sh = format!("{}/data/scripts/mosaic-net.sh", tools_src);
    let _ = crate::helpers::run::user(
        args,
        &[net_sh, "stop".to_string()],
        "log",
        false,
        Some(false),
    );

    if which::which("start").is_ok() {
        let _ = crate::helpers::run::user(
            args,
            &["start".to_string(), "nfcd".to_string()],
            "log",
            false,
            Some(false),
        );
    } else if which::which("systemctl").is_ok() {
        let check = crate::helpers::run::user(
            args,
            &[
                "systemctl".to_string(),
                "is-enabled".to_string(),
                "-q".to_string(),
                "nfcd".to_string(),
            ],
            "log",
            false,
            Some(false),
        )
        .is_ok();
        if check {
            let _ = crate::helpers::run::user(
                args,
                &[
                    "systemctl".to_string(),
                    "start".to_string(),
                    "nfcd".to_string(),
                ],
                "log",
                false,
                Some(false),
            );
        }
    }

    if which::which(crate::guest::SENSORD_BIN).is_ok() {
        if let Ok(pid) = crate::helpers::run::user(
            args,
            &["pidof".to_string(), crate::guest::SENSORD_BIN.to_string()],
            "log",
            true,
            Some(false),
        ) {
            let pid = pid.trim();
            if !pid.is_empty() {
                let _ = crate::helpers::run::user(
                    args,
                    &["kill".to_string(), "-9".to_string(), pid.to_string()],
                    "log",
                    false,
                    Some(false),
                );
            }
        }
    }

    crate::helpers::images::umount_rootfs(args).ok();
    let _ = crate::helpers::mount::umount_all(args, &crate::config::Defaults::new().data);

    if quit_session {
        if let Some(sess) = session {
            if let Some(pid_str) = sess.get("pid") {
                if let Ok(pid) = pid_str.parse::<i32>() {
                    log::info!("Terminating session because the container was stopped");
                    unsafe { libc::kill(pid, libc::SIGUSR1) };
                }
            }
        }
    }

    Ok(())
}

pub fn freeze(args: &MosaicArgs) -> anyhow::Result<()> {
    let status = crate::helpers::lxc::status(args);
    if status == "RUNNING" {
        crate::helpers::lxc::freeze(args)?;
        while crate::helpers::lxc::status(args) == "RUNNING" {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        Ok(())
    } else {
        anyhow::bail!("Mosaic container is {}", status)
    }
}

pub fn unfreeze(args: &MosaicArgs) -> anyhow::Result<()> {
    let status = crate::helpers::lxc::status(args);
    if status == "FROZEN" {
        crate::helpers::lxc::unfreeze(args)?;
        while crate::helpers::lxc::status(args) == "FROZEN" {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        Ok(())
    } else {
        anyhow::bail!("Mosaic container is {}", status)
    }
}

// D-Bus service implementation

pub struct ContainerManagerService {
    args: MosaicArgs,
    session: Arc<Mutex<Option<HashMap<String, String>>>>,
}

impl ContainerManagerService {
    pub fn new(args: MosaicArgs) -> Self {
        Self {
            args,
            session: Arc::new(Mutex::new(None)),
        }
    }
}

#[zbus::interface(name = "id.mosaic.ContainerManager")]
impl ContainerManagerService {
    async fn start(
        &self,
        session: HashMap<String, String>,
        #[zbus(header)] header: zbus::MessageHeader<'_>,
    ) -> zbus::fdo::Result<()> {
        let conn = Connection::system()
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        let dbus_proxy = zbus::fdo::DBusProxy::new(&conn)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        let sender = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("No sender".to_string()))?;
        let bus_name = zbus::names::BusName::try_from(sender.as_str())
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        let uid = dbus_proxy
            .get_connection_unix_user(bus_name.clone())
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        let pid = dbus_proxy
            .get_connection_unix_process_id(bus_name)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

        let session_user_id = session.get("user_id").cloned().unwrap_or_default();
        if uid.to_string() != "0" && uid.to_string() != session_user_id {
            return Err(zbus::fdo::Error::Failed(
                "Cannot start a session on behalf of another user".to_string(),
            ));
        }
        let session_pid = session.get("pid").cloned().unwrap_or_default();
        if uid.to_string() != "0" && pid.to_string() != session_pid {
            return Err(zbus::fdo::Error::Failed("Invalid session pid".to_string()));
        }

        {
            let sess = self.session.lock().unwrap();
            if sess.is_some() {
                return Err(zbus::fdo::Error::Failed(
                    "Already tracking a session".to_string(),
                ));
            }
        }

        // Call do_start
        let args_clone = self.args.clone();
        let session_clone = session.clone();
        let session_for_check = session.clone();
        // Run blocking
        let res = tokio::task::spawn_blocking(move || do_start(&args_clone, &session_clone))
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        if let Err(e) = res {
            return Err(zbus::fdo::Error::Failed(e.to_string()));
        }

        let mut sess = self.session.lock().unwrap();
        *sess = Some(session_for_check);
        Ok(())
    }

    async fn stop(&self, quit_session: bool) -> zbus::fdo::Result<()> {
        let sess = self.session.lock().unwrap().clone();
        let args_clone = self.args.clone();
        let res = tokio::task::spawn_blocking(move || stop(&args_clone, quit_session, sess))
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        if let Err(e) = res {
            log::debug!("Error while stopping container: {}", e);
        }
        // Clear session
        let mut sess = self.session.lock().unwrap();
        *sess = None;
        Ok(())
    }

    async fn freeze(&self) -> zbus::fdo::Result<()> {
        if !crate::actions::initializer::is_initialized(&self.args) {
            return Err(zbus::fdo::Error::Failed(
                "Mosaic is not initialized".to_string(),
            ));
        }
        let args_clone = self.args.clone();
        tokio::task::spawn_blocking(move || freeze(&args_clone))
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    async fn unfreeze(&self) -> zbus::fdo::Result<()> {
        if !crate::actions::initializer::is_initialized(&self.args) {
            return Err(zbus::fdo::Error::Failed(
                "Mosaic is not initialized".to_string(),
            ));
        }
        let args_clone = self.args.clone();
        tokio::task::spawn_blocking(move || unfreeze(&args_clone))
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    async fn get_session(&self) -> zbus::fdo::Result<HashMap<String, String>> {
        if !crate::actions::initializer::is_initialized(&self.args) {
            return Err(zbus::fdo::Error::Failed(
                "Mosaic is not initialized".to_string(),
            ));
        }
        let mut sess = self.session.lock().unwrap().clone().unwrap_or_default();
        let state = crate::helpers::lxc::status(&self.args);
        sess.insert("state".to_string(), state);
        Ok(sess)
    }
}

pub struct InitializerService {
    args: MosaicArgs,
    worker: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl InitializerService {
    pub fn new(args: MosaicArgs) -> Self {
        Self {
            args,
            worker: Arc::new(Mutex::new(None)),
        }
    }
}

#[zbus::interface(name = "id.mosaic.Initializer")]
impl InitializerService {
    async fn init(
        &self,
        params: HashMap<String, String>,
        #[zbus(header)] header: zbus::MessageHeader<'_>,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
    ) -> zbus::fdo::Result<()> {
        // Cancel previous
        {
            let mut w = self.worker.lock().unwrap();
            if let Some(handle) = w.take() {
                handle.abort();
            }
        }

        // Check polkit if needed
        let channels_cfg = crate::config::load_channels();
        let default_system = channels_cfg
            .channels
            .get("system_channel")
            .cloned()
            .unwrap_or_default();
        let default_vendor = channels_cfg
            .channels
            .get("vendor_channel")
            .cloned()
            .unwrap_or_default();
        let system_channel = params.get("system_channel").cloned().unwrap_or_default();
        let vendor_channel = params.get("vendor_channel").cloned().unwrap_or_default();
        let no_auth = system_channel == default_system && vendor_channel == default_vendor;
        if !no_auth {
            let conn2 = Connection::system()
                .await
                .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
            let dbus_proxy2 = zbus::fdo::DBusProxy::new(&conn2)
                .await
                .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
            let sender2 = header
                .sender()
                .ok_or_else(|| zbus::fdo::Error::Failed("No sender".to_string()))?;
            let bus_name2 = zbus::names::BusName::try_from(sender2.as_str())
                .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
            let pid = dbus_proxy2
                .get_connection_unix_process_id(bus_name2)
                .await
                .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

            let polkit_conn = Connection::system()
                .await
                .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
            let polkit_proxy = PolkitAuthorityProxy::new(&polkit_conn)
                .await
                .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
            let subject = std::collections::HashMap::from([
                ("pid".to_string(), zbus::zvariant::Value::new(pid)),
                ("start-time".to_string(), zbus::zvariant::Value::new(0u64)),
            ]);
            let details = HashMap::new();
            let (is_auth, _, _) = polkit_proxy
                .check_authorization(
                    ("unix-process", subject),
                    "id.mosaic.Initializer.Init",
                    details,
                    1,
                    "",
                )
                .await
                .map_err(|e| zbus::fdo::Error::Failed(format!("Polkit check failed: {}", e)))?;
            if !is_auth {
                return Err(zbus::fdo::Error::AccessDenied(
                    "Polkit: Authentication failed".to_string(),
                ));
            }
        }

        let args_clone = self.args.clone();
        let params_clone = params.clone();
        let ctxt_owned = ctxt.to_owned();
        // Stream progress out of the blocking init into ProgressChanged signals.
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let forward_ctxt = ctxt_owned.clone();
        let forward = tokio::spawn(async move {
            while let Some(message) = progress_rx.recv().await {
                let _ = InitializerService::progress_changed(&forward_ctxt, &message).await;
            }
        });

        let handle = tokio::spawn(async move {
            let _ = InitializerService::progress_changed(&ctxt_owned, "Starting initialization...")
                .await;
            let mut args_for_init = args_clone.clone();
            let res = tokio::task::spawn_blocking(move || {
                let report = move |message: &str| {
                    let _ = progress_tx.send(format!("{}\n", message));
                };
                crate::actions::initializer::init_sync(
                    &mut args_for_init,
                    &params_clone,
                    Some(&report),
                )
            })
            .await;
            let _ = forward.await;

            match res {
                Ok(Ok(_)) => {
                    let _ = Self::finished(&ctxt_owned).await;
                }
                Ok(Err(e)) => {
                    log::error!("Init failed: {}", e);
                    let _ = Self::interrupted(&ctxt_owned).await;
                }
                Err(e) => {
                    log::error!("Init join failed: {}", e);
                    let _ = Self::interrupted(&ctxt_owned).await;
                }
            }
        });

        let mut w = self.worker.lock().unwrap();
        *w = Some(handle);
        Ok(())
    }

    async fn cancel(&self) -> zbus::fdo::Result<()> {
        let mut w = self.worker.lock().unwrap();
        if let Some(handle) = w.take() {
            handle.abort();
        }
        Ok(())
    }

    #[zbus(signal)]
    async fn progress_changed(ctxt: &SignalContext<'_>, message: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn finished(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn interrupted(ctxt: &SignalContext<'_>) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.PolicyKit1.Authority",
    default_service = "org.freedesktop.PolicyKit1",
    default_path = "/org/freedesktop/PolicyKit1/Authority"
)]
trait PolkitAuthority {
    fn check_authorization(
        &self,
        subject: (&str, HashMap<String, zbus::zvariant::Value<'_>>),
        action_id: &str,
        details: HashMap<String, String>,
        flags: u32,
        cancellation_id: &str,
    ) -> zbus::Result<(bool, bool, HashMap<String, String>)>;
}

pub async fn run_container_service(args: MosaicArgs) -> anyhow::Result<()> {
    let conn = Connection::system().await?;
    let container_svc = ContainerManagerService::new(args.clone());
    let initializer_svc = InitializerService::new(args.clone());

    conn.object_server()
        .at("/ContainerManager", container_svc)
        .await?;
    conn.object_server()
        .at("/Initializer", initializer_svc)
        .await?;

    conn.request_name("id.mosaic.Container").await?;

    log::info!("Container service started");

    // Handle signals
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigint = signal(SignalKind::interrupt())?;
        let mut sigterm = signal(SignalKind::terminate())?;
        tokio::select! {
            _ = sigint.recv() => {
                log::info!("Received SIGINT, stopping container");
                // Stop container
                let _ = stop(&args, false, None);
            },
            _ = sigterm.recv() => {
                log::info!("Received SIGTERM, stopping container");
                let _ = stop(&args, false, None);
            },
            _ = std::future::pending::<()>() => {},
        }
    }
    #[cfg(not(unix))]
    {
        std::future::pending::<()>().await;
    }

    Ok(())
}

// Legacy sync functions for CLI

pub fn start_service_blocking(args: MosaicArgs) -> anyhow::Result<()> {
    crate::helpers::runtime::block_on(run_container_service(args))
}
