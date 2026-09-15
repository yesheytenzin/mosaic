// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use crate::interfaces::gbinder::ServiceManager;

const SERVICE_NAME: &str = "mosaicclipboard";

pub fn add_service<F1, F2>(args: &MosaicArgs, _send_clipboard: F1, _get_clipboard: F2)
where
    F1: Fn(String) + Send + Sync + 'static,
    F2: Fn() -> String + Send + Sync + 'static,
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
    if sm.is_present() {
        log::debug!("Clipboard service would be registered as {}", SERVICE_NAME);
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
