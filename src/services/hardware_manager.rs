// SPDX-License-Identifier: GPL-3.0-or-later

//! Serves `IHardware` to the guest: suspend, reboot, upgrade and shutdown.

use crate::args::MosaicArgs;
use crate::interfaces::i_hardware;
use std::sync::atomic::{AtomicBool, Ordering};

static STOPPING: AtomicBool = AtomicBool::new(true);

pub fn start(args: &MosaicArgs) -> anyhow::Result<()> {
    STOPPING.store(false, Ordering::SeqCst);
    let args = args.clone();
    std::thread::spawn(move || {
        while !STOPPING.load(Ordering::SeqCst) {
            let suspend_args = args.clone();
            let reboot_args = args.clone();
            let upgrade_args = args.clone();
            let shutdown_args = args.clone();
            i_hardware::add_service(
                &args,
                |_enable| {
                    log::debug!("Function enableNFC not implemented");
                    0
                },
                |_enable| {
                    log::debug!("Function enableBluetooth not implemented");
                    0
                },
                move || suspend(&suspend_args),
                move || reboot(&reboot_args),
                move |archive, time, vendor, vendor_time| {
                    upgrade(&upgrade_args, &archive, time, &vendor, vendor_time)
                },
                move |reason| shutdown_request(&shutdown_args, &reason),
                &STOPPING,
            );
        }
    });
    Ok(())
}

fn suspend(args: &MosaicArgs) {
    let cfg = crate::config::load(&args.config);
    let action = cfg
        .mosaic
        .get("suspend_action")
        .cloned()
        .unwrap_or_else(|| "freeze".to_string());
    if action == "stop" {
        let _ = crate::actions::session_manager::stop(args);
    } else {
        let _ = crate::actions::container_manager::freeze(args);
    }
}

fn reboot(args: &MosaicArgs) {
    let _ = crate::helpers::lxc::stop(args);
    let _ = crate::helpers::lxc::start(args);
}

fn upgrade(
    args: &MosaicArgs,
    system_zip: &str,
    system_time: i64,
    vendor_zip: &str,
    vendor_time: i64,
) {
    let _ = crate::helpers::lxc::stop(args);
    let _ = crate::helpers::images::umount_rootfs(args);
    let _ = crate::helpers::images::replace(
        args,
        system_zip,
        &system_time.to_string(),
        vendor_zip,
        &vendor_time.to_string(),
    );
    let cfg = crate::config::load(&args.config);
    let images_path = cfg
        .mosaic
        .get("images_path")
        .cloned()
        .unwrap_or_else(|| format!("{}/images", args.work));
    let session = crate::config::SessionDefaults::new();
    let _ = crate::helpers::images::mount_rootfs(args, &images_path, &session);
    let _ = crate::helpers::protocol::set_aidl_version(args);
    let _ = crate::helpers::lxc::start(args);
}

fn shutdown_request(args: &MosaicArgs, reason: &str) {
    let is_reboot = reason.starts_with('1');
    let mut tries = 0;
    while crate::helpers::lxc::status(args) != "STOPPED" {
        if tries >= 30 {
            log::debug!(
                "Android is still not stopped, give up waiting after {} seconds",
                tries
            );
            return;
        }
        log::debug!("Waiting for Android to shutdown");
        std::thread::sleep(std::time::Duration::from_secs(1));
        tries += 1;
    }

    if is_reboot {
        let _ = crate::helpers::lxc::start(args);
    } else {
        let _ = crate::actions::container_manager::stop(args, true, None);
    }
}

pub fn stop(_args: &MosaicArgs) -> anyhow::Result<()> {
    STOPPING.store(true, Ordering::SeqCst);
    Ok(())
}
