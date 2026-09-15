// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use crate::interfaces::gbinder::ServiceManager;

const SERVICE_NAME: &str = "mosaicnotifications";

pub const ID_NONE: i32 = 0;

pub struct Action {
    pub id: String,
    pub label: String,
}

pub struct ImageData {
    pub width: i32,
    pub height: i32,
    pub rowstride: i32,
    pub has_alpha: bool,
    pub data: Vec<u8>,
}

pub fn add_service<F1, F2, F3>(
    args: &MosaicArgs,
    _register_listener: F1,
    _notify: F2,
    _close_notification: F3,
) where
    F1: Fn(String) + Send + Sync + 'static,
    F2: Fn(
            i32,
            String,
            String,
            String,
            String,
            Vec<Action>,
            Option<ImageData>,
            String,
            bool,
            i32,
            bool,
            bool,
            u8,
        ) -> i32
        + Send
        + Sync
        + 'static,
    F3: Fn(i32) + Send + Sync + 'static,
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
        log::debug!(
            "Notifications service would be registered as {}",
            SERVICE_NAME
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
