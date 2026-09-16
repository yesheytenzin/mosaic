// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use crate::interfaces::gbinder::{Client, RemoteObject, ServiceManager};

const INTERFACE: &str = crate::guest::IFACE_STATUS_BAR;
const SERVICE_NAME: &str = crate::guest::SVC_STATUS_BAR;

pub struct IStatusBarService {
    client: Client,
}

impl IStatusBarService {
    pub fn new(remote: RemoteObject) -> Self {
        let client = Client::new(remote, INTERFACE);
        Self { client }
    }

    pub fn expand(&self) {
        let request = self.client.new_request();
        let _ = self.client.transact_sync_reply(1, request);
    }

    pub fn collapse(&self) {
        let request = self.client.new_request();
        let _ = self.client.transact_sync_reply(2, request);
    }
}

pub fn get_service(args: &MosaicArgs) -> Option<IStatusBarService> {
    let (binder, _, _) = crate::interfaces::gbinder::load_binder_nodes(args).ok()?;
    let cfg = crate::config::load(&args.config);
    let binder_protocol = cfg.mosaic.get("binder_protocol").cloned();
    let service_protocol = cfg.mosaic.get("service_manager_protocol").cloned();
    let device = format!("/dev/{}", binder);
    let sm = ServiceManager::new(
        &device,
        service_protocol.as_deref(),
        binder_protocol.as_deref(),
    )
    .ok()?;
    if !sm.is_present() {
        return None;
    }
    let remote = sm.get_service_sync(SERVICE_NAME).ok().flatten()?;
    Some(IStatusBarService::new(remote))
}
