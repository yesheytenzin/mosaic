// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use crate::config::Defaults;
use std::collections::HashMap;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::Path;

/// Reports human readable progress while initializing, used by the remote
/// initializer service to stream to clients.
pub type Progress<'a> = Option<&'a (dyn Fn(&str) + Send + Sync)>;

pub fn is_initialized(args: &MosaicArgs) -> bool {
    Path::new(&args.config).is_file() && Path::new(&format!("{}/rootfs", args.work)).is_dir()
}

pub fn get_vendor_type() -> String {
    let vndk = crate::helpers::props::host_get("ro.vndk.version");
    let vendorapi = crate::helpers::props::host_get("ro.vendor.build.version.sdk");
    let mut ret = "MAINLINE".to_string();
    if !vndk.is_empty() {
        if let Ok(v) = vndk.parse::<i32>() {
            if v > 19 {
                let mut halium_ver = v - 19;
                if v > 31 {
                    halium_ver -= 1;
                }
                ret = format!("HALIUM_{}", halium_ver);
                if v == 32 {
                    ret.push('L');
                }
            }
        }
    } else if !vendorapi.is_empty() {
        if let Ok(v) = vendorapi.parse::<i32>() {
            if v > 32 {
                let halium_ver = v - 20;
                ret = format!("HALIUM_{}", halium_ver);
            }
        }
    }
    ret
}

fn setup_config(args: &mut MosaicArgs) -> anyhow::Result<bool> {
    let mut cfg = crate::config::load(&args.config);
    let arch = crate::helpers::arch::host()?;
    args.cache.insert("arch".to_string(), arch.clone());
    cfg.mosaic.insert("arch".to_string(), arch.clone());

    let vendor_type = get_vendor_type();
    args.cache
        .insert("vendor_type".to_string(), vendor_type.clone());
    cfg.mosaic
        .insert("vendor_type".to_string(), vendor_type.clone());

    // Setup binder nodes
    if let Err(e) = crate::helpers::drivers::setup_binder_nodes(args, &vendor_type) {
        log::warn!("Failed to setup binder nodes: {}", e);
        // Continue anyway for test environments without binder
    }
    if let Some(binder) = args.cache.get("BINDER_DRIVER") {
        cfg.mosaic.insert("binder".to_string(), binder.clone());
    }
    if let Some(vnd) = args.cache.get("VNDBINDER_DRIVER") {
        cfg.mosaic.insert("vndbinder".to_string(), vnd.clone());
    }
    if let Some(hw) = args.cache.get("HWBINDER_DRIVER") {
        cfg.mosaic.insert("hwbinder".to_string(), hw.clone());
    }

    let defaults = Defaults::new();
    let mut has_preinstalled = false;
    let mut images_path_opt: Option<String> = args.cache.get("images_path").cloned();

    for preinstalled in &defaults.preinstalled_images_paths {
        if Path::new(preinstalled).is_dir() {
            let system_path = format!("{}/system.img", preinstalled);
            let vendor_path = format!("{}/vendor.img", preinstalled);
            let system_exists = Path::new(&system_path).exists()
                || std::fs::metadata(&system_path)
                    .map(|m| m.file_type().is_block_device())
                    .unwrap_or(false);
            let vendor_exists = Path::new(&vendor_path).exists()
                || std::fs::metadata(&vendor_path)
                    .map(|m| m.file_type().is_block_device())
                    .unwrap_or(false);
            if system_exists && vendor_exists {
                has_preinstalled = true;
                images_path_opt = Some(preinstalled.clone());
                break;
            } else if Path::new(preinstalled).exists() {
                log::warn!(
                    "Found directory {} but missing system or vendor image, ignoring...",
                    preinstalled
                );
            }
        }
    }

    let images_path = images_path_opt.unwrap_or_else(|| defaults.images_path.clone());
    args.cache
        .insert("images_path".to_string(), images_path.clone());
    cfg.mosaic
        .insert("images_path".to_string(), images_path.clone());

    if has_preinstalled {
        args.cache
            .insert("system_ota".to_string(), "None".to_string());
        args.cache
            .insert("vendor_ota".to_string(), "None".to_string());
        cfg.mosaic
            .insert("system_ota".to_string(), "None".to_string());
        cfg.mosaic
            .insert("vendor_ota".to_string(), "None".to_string());
        cfg.mosaic.insert(
            "system_datetime".to_string(),
            defaults.system_datetime.clone(),
        );
        cfg.mosaic.insert(
            "vendor_datetime".to_string(),
            defaults.vendor_datetime.clone(),
        );
        crate::config::save(&args.config, &cfg)?;
        return Ok(true);
    }

    let channels_cfg = crate::config::load_channels();
    let mut system_channel = args
        .cache
        .get("system_channel")
        .cloned()
        .or_else(|| channels_cfg.channels.get("system_channel").cloned());
    let mut vendor_channel = args
        .cache
        .get("vendor_channel")
        .cloned()
        .or_else(|| channels_cfg.channels.get("vendor_channel").cloned());
    let mut rom_type = args
        .cache
        .get("rom_type")
        .cloned()
        .or_else(|| channels_cfg.channels.get("rom_type").cloned());
    let mut system_type = args
        .cache
        .get("system_type")
        .cloned()
        .or_else(|| channels_cfg.channels.get("system_type").cloned());

    // Also check args passed via init params (stored in cache already)
    // If still None, use defaults
    if system_channel.is_none() {
        system_channel = channels_cfg.channels.get("system_channel").cloned();
    }
    if vendor_channel.is_none() {
        vendor_channel = channels_cfg.channels.get("vendor_channel").cloned();
    }
    if rom_type.is_none() {
        rom_type = channels_cfg.channels.get("rom_type").cloned();
    }
    if system_type.is_none() {
        system_type = channels_cfg.channels.get("system_type").cloned();
    }

    let system_channel = system_channel.unwrap_or_default();
    let vendor_channel = vendor_channel.unwrap_or_default();
    let rom_type = rom_type.unwrap_or_default();
    let system_type = system_type.unwrap_or_default();

    if system_channel.is_empty() || vendor_channel.is_empty() {
        log::error!("ERROR: You must provide 'System OTA' and 'Vendor OTA' URLs.");
        return Ok(false);
    }

    args.cache
        .insert("system_channel".to_string(), system_channel.clone());
    args.cache
        .insert("vendor_channel".to_string(), vendor_channel.clone());
    args.cache.insert("rom_type".to_string(), rom_type.clone());
    args.cache
        .insert("system_type".to_string(), system_type.clone());

    let system_ota = format!(
        "{}/{}/{}{}/{}.json",
        system_channel,
        rom_type,
        crate::guest::OTA_PATH_PREFIX,
        arch,
        system_type
    );
    args.cache
        .insert("system_ota".to_string(), system_ota.clone());

    // Validate system OTA via HTTP blocking
    let (status, _) = crate::helpers::http::retrieve_blocking(&system_ota, None);
    if status != 200 {
        anyhow::bail!(
            "Failed to get system OTA channel: {}, error: {}",
            system_ota,
            status
        );
    }

    let device_codename = crate::helpers::props::host_get("ro.product.device");
    let mut found_vendor = false;
    let mut vendor_ota_final = String::new();
    let mut vendor_type_final = String::new();
    for vendor in [device_codename.clone(), get_vendor_type()] {
        if vendor.is_empty() {
            continue;
        }
        let vendor_ota = format!(
            "{}/{}{}/{}.json",
            vendor_channel,
            crate::guest::OTA_PATH_PREFIX,
            arch,
            vendor.replace(' ', "_")
        );
        let (status, _) = crate::helpers::http::retrieve_blocking(&vendor_ota, None);
        if status == 200 {
            found_vendor = true;
            vendor_ota_final = vendor_ota.clone();
            vendor_type_final = vendor.clone();
            args.cache.insert("vendor_type".to_string(), vendor.clone());
            cfg.mosaic.insert("vendor_type".to_string(), vendor.clone());
            break;
        }
    }

    if !found_vendor {
        anyhow::bail!("Failed to get vendor OTA channel: {}", vendor_ota_final);
    }

    args.cache
        .insert("vendor_ota".to_string(), vendor_ota_final.clone());

    let current_system_ota = cfg.mosaic.get("system_ota").cloned().unwrap_or_default();
    if system_ota != current_system_ota {
        cfg.mosaic.insert(
            "system_datetime".to_string(),
            defaults.system_datetime.clone(),
        );
    }
    let current_vendor_ota = cfg.mosaic.get("vendor_ota").cloned().unwrap_or_default();
    if vendor_ota_final != current_vendor_ota {
        cfg.mosaic.insert(
            "vendor_datetime".to_string(),
            defaults.vendor_datetime.clone(),
        );
    }

    cfg.mosaic
        .insert("vendor_type".to_string(), vendor_type_final);
    cfg.mosaic.insert("system_ota".to_string(), system_ota);
    cfg.mosaic
        .insert("vendor_ota".to_string(), vendor_ota_final);
    crate::config::save(&args.config, &cfg)?;
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
pub fn init(
    args: &mut MosaicArgs,
    force: bool,
    images_path: Option<String>,
    system_channel: Option<String>,
    vendor_channel: Option<String>,
    rom_type: Option<String>,
    system_type: Option<String>,
    progress: Progress<'_>,
) -> anyhow::Result<()> {
    let report = |message: &str| {
        if let Some(cb) = progress {
            cb(message);
        }
    };

    if is_initialized(args) && !force {
        log::info!("Already initialized");
        return Ok(());
    }

    // Store passed args into cache for setup_config
    if let Some(p) = images_path {
        if !p.is_empty() {
            args.cache.insert("images_path".to_string(), p);
        }
    }
    if let Some(p) = system_channel {
        args.cache.insert("system_channel".to_string(), p);
    }
    if let Some(p) = vendor_channel {
        args.cache.insert("vendor_channel".to_string(), p);
    }
    if let Some(p) = rom_type {
        args.cache.insert("rom_type".to_string(), p);
    }
    if let Some(p) = system_type {
        args.cache.insert("system_type".to_string(), p);
    }

    report("Setting up config");
    if !setup_config(args)? {
        return Ok(());
    }

    let mut status = "STOPPED".to_string();
    let lxc_path = format!("{}/lxc/mosaic", args.work);
    if Path::new(&lxc_path).exists() {
        status = crate::helpers::lxc::status(args);
    }

    let mut session: Option<HashMap<String, String>> = None;
    if status != "STOPPED" {
        if args.cache.contains_key("running_init_in_service") {
            // Try to get session from args cache? In Rust we don't store session in args
            // For now, just stop container via lxc
            session = None;
            let _ = crate::actions::container_manager::stop(args, false, session.clone());
        } else {
            report("Stopping container");
            log::info!("Stopping container");
            // Try D-Bus stop, fallback to lxc
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let res = rt.block_on(crate::helpers::ipc::container_stop(false));
            if res.is_err() {
                log::debug!("D-Bus Stop failed, falling back to lxc stop: {:?}", res);
                let _ = crate::actions::container_manager::stop(args, false, None);
            } else {
                // Try to get session via D-Bus
                if let Ok(s) = rt.block_on(crate::helpers::ipc::container_get_session()) {
                    session = Some(s);
                }
            }
        }
    }

    let cfg = crate::config::load(&args.config);
    let images_path = cfg
        .mosaic
        .get("images_path")
        .cloned()
        .unwrap_or_else(|| format!("{}/images", args.work));
    let preinstalled = Defaults::new().preinstalled_images_paths;
    report("Downloading images");
    if !preinstalled.contains(&images_path) {
        // Download images (blocking)
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        rt.block_on(crate::helpers::images::get(args))?;
    } else {
        crate::helpers::images::remove_overlay(args)?;
    }

    report("Creating directories");
    for (dir, sub) in [
        (format!("{}/rootfs", args.work), None),
        (format!("{}/overlay", args.work), Some("vendor")),
        (format!("{}/overlay_rw", args.work), Some("system")),
    ] {
        if !Path::new(&dir).is_dir() {
            std::fs::create_dir_all(&dir)?;
            if let Some(sub) = sub {
                std::fs::create_dir_all(format!("{}/{}", dir, sub))?;
            }
            if dir.contains("overlay_rw") {
                std::fs::create_dir_all(format!("{}/vendor", dir))?;
            }
        }
    }
    // Ensure overlay_work exists for mount_overlay
    let overlay_work = format!("{}/overlay_work", args.work);
    if !Path::new(&overlay_work).exists() {
        std::fs::create_dir_all(&overlay_work)?;
    }

    report("Configuring the container");
    crate::helpers::drivers::probe_ashmem_driver(args);
    crate::helpers::lxc::setup_host_perms(args).ok();
    crate::helpers::lxc::set_lxc_config(args).ok();
    crate::helpers::lxc::make_base_props(args).ok();

    if status != "STOPPED" {
        // Restart container
        if args.cache.contains_key("running_init_in_service") {
            if let Some(sess) = session {
                let _ = crate::actions::container_manager::do_start(args, &sess);
            }
        } else {
            report("Starting container");
            log::info!("Starting container");
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            if let Some(sess) = session {
                let _ = rt.block_on(crate::helpers::ipc::container_start(sess));
            } else {
                log::debug!("No session to restart container with");
            }
        }
    }

    // Fix permissions on work dir
    let _ = std::fs::set_permissions(&args.work, std::fs::Permissions::from_mode(0o700));

    Ok(())
}

pub fn init_sync(
    args: &mut MosaicArgs,
    params: &HashMap<String, String>,
    progress: Progress<'_>,
) -> anyhow::Result<()> {
    let system_channel = params.get("system_channel").cloned();
    let vendor_channel = params.get("vendor_channel").cloned();
    let system_type = params.get("system_type").cloned();
    let rom_type = params.get("rom_type").cloned();
    let images_path = params.get("images_path").cloned();
    init(
        args,
        true,
        images_path,
        system_channel,
        vendor_channel,
        rom_type,
        system_type,
        progress,
    )
}

/// Attach to the running Initializer service and stream its progress to
/// stdout, replacing the GTK dialog the original used.
pub async fn remote_init_client(args: &MosaicArgs) -> anyhow::Result<()> {
    use crate::helpers::ipc::InitializerProxy;

    {
        let connection = zbus::Connection::system().await?;
        let proxy = InitializerProxy::new(&connection).await?;

        let channels = crate::config::load_channels();
        let mut params = HashMap::new();
        for (key, value) in [
            ("system_channel", "system_channel"),
            ("vendor_channel", "vendor_channel"),
            ("system_type", "system_type"),
        ] {
            params.insert(
                key.to_string(),
                channels.channels.get(value).cloned().unwrap_or_default(),
            );
        }

        let mut progress = InitializerProxy::receive_progress_changed(&proxy).await?;
        let mut finished = InitializerProxy::receive_finished(&proxy).await?;
        let mut interrupted = InitializerProxy::receive_interrupted(&proxy).await?;

        println!("Waiting for waydroid container service...");
        proxy.init(params).await?;

        use futures_util::StreamExt;
        loop {
            tokio::select! {
                Some(signal) = progress.next() => {
                    if let Ok(args) = signal.args() {
                        print!("{}", args.message);
                        use std::io::Write;
                        let _ = std::io::stdout().flush();
                    }
                }
                Some(_) = finished.next() => {
                    if is_initialized(args) {
                        println!("\nDone");
                    }
                    break;
                }
                Some(_) = interrupted.next() => {
                    println!("\nInterrupted");
                    break;
                }
                else => break,
            }
        }
        Ok(())
    }
}
