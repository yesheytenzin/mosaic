// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use crate::interfaces::gbinder::ServiceManager;

const SERVICE_NAME: &str = crate::guest::SVC_HARDWARE;

pub fn add_service<F1, F2, F3, F4, F5, F6>(
    args: &MosaicArgs,
    _enable_nfc: F1,
    _enable_bluetooth: F2,
    _suspend: F3,
    _reboot: F4,
    _upgrade: F5,
    _shutdown_request: F6,
) where
    F1: Fn(bool) + Send + Sync + 'static,
    F2: Fn(bool) + Send + Sync + 'static,
    F3: Fn() + Send + Sync + 'static,
    F4: Fn() + Send + Sync + 'static,
    F5: Fn(String, i32, String, i32) + Send + Sync + 'static,
    F6: Fn(String) + Send + Sync + 'static,
{
    let (binder, _, _) = match crate::interfaces::gbinder::load_binder_nodes(args) {
        Ok(v) => v,
        Err(e) => {
            log::debug!("Failed to load binder nodes: {}", e);
            return;
        }
    };
    let cfg = crate::config::load(&args.config);
    let binder_protocol = cfg.mosaic.get("binder_protocol").cloned();
    let service_protocol = cfg.mosaic.get("service_manager_protocol").cloned();
    let device = format!("/dev/{}", binder);
    let sm = match ServiceManager::new(
        &device,
        service_protocol.as_deref(),
        binder_protocol.as_deref(),
    ) {
        Ok(sm) => sm,
        Err(e) => {
            log::debug!("Failed to create ServiceManager: {}", e);
            return;
        }
    };

    // In real implementation, we would create a LocalObject and add service
    // For now, just log and simulate presence handling
    if sm.is_present() {
        log::debug!("Hardware service would be registered as {}", SERVICE_NAME);
        // Simulate MainLoop run with sleep
        std::thread::sleep(std::time::Duration::from_millis(100));
    } else {
        log::error!("Binder ServiceManager not present for hardware service");
    }
}
