// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use std::path::Path;

fn get_config(args: &mut MosaicArgs) {
    let cfg = crate::config::load(&args.config);
    if let Some(v) = cfg.mosaic.get("arch") {
        args.cache.insert("arch".to_string(), v.clone());
    }
    if let Some(v) = cfg.mosaic.get("images_path") {
        args.cache.insert("images_path".to_string(), v.clone());
    }
    if let Some(v) = cfg.mosaic.get("vendor_type") {
        args.cache.insert("vendor_type".to_string(), v.clone());
    }
    if let Some(v) = cfg.mosaic.get("system_ota") {
        args.cache.insert("system_ota".to_string(), v.clone());
    }
    if let Some(v) = cfg.mosaic.get("vendor_ota") {
        args.cache.insert("vendor_ota".to_string(), v.clone());
    }
    args.cache.insert("session".to_string(), "".to_string());
}

fn migration(args: &MosaicArgs) {
    let old_ver = crate::helpers::props::file_get(
        &format!("{}/mosaic_base.prop", args.work),
        "mosaic.tools_version",
    )
    .unwrap_or_default();
    if crate::helpers::version::versiontuple(&old_ver)
        <= crate::helpers::version::versiontuple("1.3.4")
    {
        let chmod_paths = [
            "cache_http",
            "host-permissions",
            "lxc",
            "images",
            "rootfs",
            "data",
            "mosaic_base.prop",
            "mosaic.prop",
            "mosaic.cfg",
        ];
        let mut cmd = vec!["chmod".to_string(), "-R".to_string(), "g-w,o-w".to_string()];
        for f in chmod_paths {
            cmd.push(format!("{}/{}", args.work, f));
        }
        let _ = crate::helpers::run::user(args, &cmd, "log", false, Some(false));
        let _ = crate::helpers::run::user(
            args,
            &[
                "chmod".to_string(),
                "g-w,o-w".to_string(),
                args.work.clone(),
            ],
            "log",
            false,
            Some(false),
        );
        let _ = std::fs::remove_file(format!("{}/session.cfg", args.work));
    }
    if crate::helpers::version::versiontuple(&old_ver)
        <= crate::helpers::version::versiontuple("1.6.0")
    {
        let mut cfg = crate::config::load(&args.config);
        cfg.mosaic
            .insert("auto_adb".to_string(), "False".to_string());
        let _ = crate::config::save(&args.config, &cfg);
    }
}

pub fn upgrade(args: &MosaicArgs, offline: bool) -> anyhow::Result<()> {
    let mut args_mut = args.clone();
    get_config(&mut args_mut);

    let mut status = "STOPPED".to_string();
    if Path::new(&format!("{}/lxc/mosaic", args.work)).exists() {
        status = crate::helpers::lxc::status(args);
    }

    let mut session: Option<std::collections::HashMap<String, String>> = None;
    if status != "STOPPED" {
        log::info!("Stopping container");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        match rt.block_on(crate::helpers::ipc::container_get_session()) {
            Ok(s) => {
                session = Some(s.clone());
                let _ = rt.block_on(crate::helpers::ipc::container_stop(false));
            }
            Err(e) => {
                log::debug!("{:?}", e);
                let _ = crate::actions::container_manager::stop(args, false, None);
            }
        }
    }

    migration(args);

    // Load binder nodes
    let mut args_for_binder = args.clone();
    let _ = crate::helpers::drivers::load_binder_nodes(&mut args_for_binder);

    if !offline {
        let images_path = args_mut
            .cache
            .get("images_path")
            .cloned()
            .unwrap_or_else(|| format!("{}/images", args.work));
        let preinstalled = crate::config::Defaults::new().preinstalled_images_paths;
        if !preinstalled.contains(&images_path) {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            rt.block_on(crate::helpers::images::get(args))?;
        } else {
            log::info!("Upgrade refused because Mosaic was configured to load pre-installed image from {}.", images_path);
        }
    }

    crate::helpers::drivers::probe_ashmem_driver(args);
    crate::helpers::lxc::setup_host_perms(args).ok();
    crate::helpers::lxc::set_lxc_config(args).ok();
    crate::helpers::lxc::make_base_props(args).ok();

    if status != "STOPPED" {
        log::info!("Starting container");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let res = if let Some(sess) = session {
            rt.block_on(crate::helpers::ipc::container_start(sess))
        } else {
            Err(anyhow::anyhow!("No session"))
        };
        if res.is_err() {
            log::debug!("{:?}", res);
            log::error!("Failed to restart container. Please do so manually.");
        }
    }

    Ok(())
}
