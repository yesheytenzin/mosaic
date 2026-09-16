// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use crate::interfaces::gbinder::{Client, RemoteObject, ServiceManager};
use std::time::Duration;

const INTERFACE: &str = crate::guest::IFACE_PLATFORM;
const SERVICE_NAME: &str = crate::guest::SVC_PLATFORM;

const TRANSACTION_GETPROP: u32 = 1;
const TRANSACTION_SETPROP: u32 = 2;
const TRANSACTION_GETAPPSINFO: u32 = 3;
const TRANSACTION_GETAPPINFO: u32 = 4;
const TRANSACTION_INSTALLAPP: u32 = 5;
const TRANSACTION_REMOVEAPP: u32 = 6;
const TRANSACTION_LAUNCHAPP: u32 = 7;
const TRANSACTION_GETAPPNAME: u32 = 8;
const TRANSACTION_SETTINGSPUTSTRING: u32 = 9;
const TRANSACTION_SETTINGSGETSTRING: u32 = 10;
const TRANSACTION_SETTINGSPUTINT: u32 = 11;
const TRANSACTION_SETTINGSGETINT: u32 = 12;
const TRANSACTION_LAUNCHINTENT: u32 = 13;

#[derive(Debug, Clone)]
pub struct AppInfo {
    pub name: String,
    pub package_name: String,
    pub action: String,
    pub launch_intent: String,
    pub component_package_name: String,
    pub component_class_name: String,
    pub categories: Vec<String>,
}

pub struct IPlatform {
    client: Client,
}

impl IPlatform {
    pub fn new(remote: RemoteObject) -> Self {
        let client = Client::new(remote, INTERFACE);
        Self { client }
    }

    pub fn getprop(&self, arg1: &str, arg2: &str) -> Option<String> {
        let mut request = self.client.new_request();
        request.append_string16(arg1);
        request.append_string16(arg2);
        match self
            .client
            .transact_sync_reply(TRANSACTION_GETPROP, request)
        {
            Ok((mut reader, 0)) => {
                if let Ok((_, exception)) = reader.read_int32() {
                    if exception == 0 {
                        if let Ok(rep1) = reader.read_string16() {
                            return Some(rep1);
                        }
                    } else {
                        log::error!("Failed with code: {}", exception);
                    }
                }
            }
            Ok((_, status)) => log::error!("Sending reply failed with status {}", status),
            Err(e) => log::error!("Sending reply failed: {}", e),
        }
        None
    }

    pub fn setprop(&self, arg1: &str, arg2: &str) {
        let mut request = self.client.new_request();
        request.append_string16(arg1);
        request.append_string16(arg2);
        match self
            .client
            .transact_sync_reply(TRANSACTION_SETPROP, request)
        {
            Ok((mut reader, 0)) => {
                if let Ok((_, exception)) = reader.read_int32() {
                    if exception != 0 {
                        log::error!("Failed with code: {}", exception);
                    }
                }
            }
            Ok((_, status)) => log::error!("Sending reply failed with status {}", status),
            Err(e) => log::error!("Sending reply failed: {}", e),
        }
    }

    pub fn get_apps_info(&self) -> Vec<AppInfo> {
        let request = self.client.new_request();
        match self
            .client
            .transact_sync_reply(TRANSACTION_GETAPPSINFO, request)
        {
            Ok((mut reader, 0)) => {
                if let Ok((_, exception)) = reader.read_int32() {
                    if exception == 0 {
                        if let Ok((_, apps)) = reader.read_int32() {
                            let mut list = Vec::new();
                            for _ in 0..apps {
                                if let Ok((_, has_value)) = reader.read_int32() {
                                    if has_value == 1 {
                                        let app = AppInfo {
                                            name: reader.read_string16().unwrap_or_default(),
                                            package_name: reader
                                                .read_string16()
                                                .unwrap_or_default(),
                                            action: reader.read_string16().unwrap_or_default(),
                                            launch_intent: reader
                                                .read_string16()
                                                .unwrap_or_default(),
                                            component_package_name: reader
                                                .read_string16()
                                                .unwrap_or_default(),
                                            component_class_name: reader
                                                .read_string16()
                                                .unwrap_or_default(),
                                            categories: {
                                                let mut cats = Vec::new();
                                                if let Ok((_, len)) = reader.read_int32() {
                                                    for _ in 0..len {
                                                        if let Ok(s) = reader.read_string16() {
                                                            cats.push(s);
                                                        }
                                                    }
                                                }
                                                cats
                                            },
                                        };
                                        list.push(app);
                                    }
                                }
                            }
                            return list;
                        }
                    } else {
                        log::error!("Failed with code: {}", exception);
                    }
                }
            }
            Ok((_, status)) => log::error!("Sending reply failed with status {}", status),
            Err(e) => log::error!("Sending reply failed: {}", e),
        }
        vec![]
    }

    pub fn get_app_info(&self, package: &str) -> Option<AppInfo> {
        let mut request = self.client.new_request();
        request.append_string16(package);
        match self
            .client
            .transact_sync_reply(TRANSACTION_GETAPPINFO, request)
        {
            Ok((mut reader, 0)) => {
                if let Ok((_, exception)) = reader.read_int32() {
                    if exception == 0 {
                        if let Ok((_, has_value)) = reader.read_int32() {
                            if has_value == 1 {
                                let app = AppInfo {
                                    name: reader.read_string16().unwrap_or_default(),
                                    package_name: reader.read_string16().unwrap_or_default(),
                                    action: reader.read_string16().unwrap_or_default(),
                                    launch_intent: reader.read_string16().unwrap_or_default(),
                                    component_package_name: reader
                                        .read_string16()
                                        .unwrap_or_default(),
                                    component_class_name: reader
                                        .read_string16()
                                        .unwrap_or_default(),
                                    categories: {
                                        let mut cats = Vec::new();
                                        if let Ok((_, len)) = reader.read_int32() {
                                            for _ in 0..len {
                                                if let Ok(s) = reader.read_string16() {
                                                    cats.push(s);
                                                }
                                            }
                                        }
                                        cats
                                    },
                                };
                                return Some(app);
                            }
                        }
                    } else {
                        log::error!("Failed with code: {}", exception);
                    }
                }
            }
            Ok((_, status)) => log::error!("Sending reply failed with status {}", status),
            Err(e) => log::error!("Sending reply failed: {}", e),
        }
        None
    }

    pub fn install_app(&self, path: &str) -> Option<i32> {
        let mut request = self.client.new_request();
        request.append_string16(path);
        match self
            .client
            .transact_sync_reply(TRANSACTION_INSTALLAPP, request)
        {
            Ok((mut reader, 0)) => {
                if let Ok((_, exception)) = reader.read_int32() {
                    if exception == 0 {
                        if let Ok((_, ret)) = reader.read_int32() {
                            return Some(ret);
                        }
                    } else {
                        log::error!("Failed with code: {}", exception);
                    }
                }
            }
            Ok((_, status)) => log::error!("Sending reply failed with status {}", status),
            Err(e) => log::error!("Sending reply failed: {}", e),
        }
        None
    }

    pub fn remove_app(&self, package: &str) -> Option<i32> {
        let mut request = self.client.new_request();
        request.append_string16(package);
        match self
            .client
            .transact_sync_reply(TRANSACTION_REMOVEAPP, request)
        {
            Ok((mut reader, 0)) => {
                if let Ok((_, exception)) = reader.read_int32() {
                    if exception == 0 {
                        if let Ok((_, ret)) = reader.read_int32() {
                            return Some(ret);
                        }
                    } else {
                        log::error!("Failed with code: {}", exception);
                    }
                }
            }
            Ok((_, status)) => log::error!("Sending reply failed with status {}", status),
            Err(e) => log::error!("Sending reply failed: {}", e),
        }
        None
    }

    pub fn launch_app(&self, package: &str) {
        let mut request = self.client.new_request();
        request.append_string16(package);
        match self
            .client
            .transact_sync_reply(TRANSACTION_LAUNCHAPP, request)
        {
            Ok((mut reader, 0)) => {
                if let Ok((_, exception)) = reader.read_int32() {
                    if exception != 0 {
                        log::error!("Failed with code: {}", exception);
                    }
                }
            }
            Ok((_, status)) => log::error!("Sending reply failed with status {}", status),
            Err(e) => log::error!("Sending reply failed: {}", e),
        }
    }

    pub fn launch_intent(&self, action: &str, uri: &str) -> Option<String> {
        let mut request = self.client.new_request();
        request.append_string16(action);
        request.append_string16(uri);
        match self
            .client
            .transact_sync_reply(TRANSACTION_LAUNCHINTENT, request)
        {
            Ok((mut reader, 0)) => {
                if let Ok((_, exception)) = reader.read_int32() {
                    if exception == 0 {
                        if let Ok(rep1) = reader.read_string16() {
                            return Some(rep1);
                        }
                    } else {
                        log::error!("Failed with code: {}", exception);
                    }
                }
            }
            Ok((_, status)) => log::error!("Sending reply failed with status {}", status),
            Err(e) => log::error!("Sending reply failed: {}", e),
        }
        None
    }

    pub fn get_app_name(&self, package: &str) -> Option<String> {
        let mut request = self.client.new_request();
        request.append_string16(package);
        match self
            .client
            .transact_sync_reply(TRANSACTION_GETAPPNAME, request)
        {
            Ok((mut reader, 0)) => {
                if let Ok((_, exception)) = reader.read_int32() {
                    if exception == 0 {
                        if let Ok(rep1) = reader.read_string16() {
                            return Some(rep1);
                        }
                    } else {
                        log::error!("Failed with code: {}", exception);
                    }
                }
            }
            Ok((_, status)) => log::error!("Sending reply failed with status {}", status),
            Err(e) => log::error!("Sending reply failed: {}", e),
        }
        None
    }

    pub fn settings_put_string(&self, arg1: i32, arg2: &str, arg3: &str) {
        let mut request = self.client.new_request();
        request.append_int32(arg1);
        request.append_string16(arg2);
        request.append_string16(arg3);
        match self
            .client
            .transact_sync_reply(TRANSACTION_SETTINGSPUTSTRING, request)
        {
            Ok((mut reader, 0)) => {
                if let Ok((_, exception)) = reader.read_int32() {
                    if exception != 0 {
                        log::error!("Failed with code: {}", exception);
                    }
                }
            }
            Ok((_, status)) => log::error!("Sending reply failed with status {}", status),
            Err(e) => log::error!("Sending reply failed: {}", e),
        }
    }

    pub fn settings_get_string(&self, arg1: i32, arg2: &str) -> Option<String> {
        let mut request = self.client.new_request();
        request.append_int32(arg1);
        request.append_string16(arg2);
        match self
            .client
            .transact_sync_reply(TRANSACTION_SETTINGSGETSTRING, request)
        {
            Ok((mut reader, 0)) => {
                if let Ok((_, exception)) = reader.read_int32() {
                    if exception == 0 {
                        if let Ok(rep1) = reader.read_string16() {
                            return Some(rep1);
                        }
                    } else {
                        log::error!("Failed with code: {}", exception);
                    }
                }
            }
            Ok((_, status)) => log::error!("Sending reply failed with status {}", status),
            Err(e) => log::error!("Sending reply failed: {}", e),
        }
        None
    }

    pub fn settings_put_int(&self, arg1: i32, arg2: &str, arg3: i32) {
        let mut request = self.client.new_request();
        request.append_int32(arg1);
        request.append_string16(arg2);
        request.append_int32(arg3);
        match self
            .client
            .transact_sync_reply(TRANSACTION_SETTINGSPUTINT, request)
        {
            Ok((mut reader, 0)) => {
                if let Ok((_, exception)) = reader.read_int32() {
                    if exception != 0 {
                        log::error!("Failed with code: {}", exception);
                    }
                }
            }
            Ok((_, status)) => log::error!("Sending reply failed with status {}", status),
            Err(e) => log::error!("Sending reply failed: {}", e),
        }
    }

    pub fn settings_get_int(&self, arg1: i32, arg2: &str) -> Option<i32> {
        let mut request = self.client.new_request();
        request.append_int32(arg1);
        request.append_string16(arg2);
        match self
            .client
            .transact_sync_reply(TRANSACTION_SETTINGSGETINT, request)
        {
            Ok((mut reader, 0)) => {
                if let Ok((_, exception)) = reader.read_int32() {
                    if exception == 0 {
                        if let Ok((_, rep1)) = reader.read_int32() {
                            return Some(rep1);
                        }
                    } else {
                        log::error!("Failed with code: {}", exception);
                    }
                }
            }
            Ok((_, status)) => log::error!("Sending reply failed with status {}", status),
            Err(e) => log::error!("Sending reply failed: {}", e),
        }
        None
    }
}

pub fn get_service(args: &MosaicArgs) -> Option<IPlatform> {
    let (binder, _, _) = crate::interfaces::gbinder::load_binder_nodes(args).ok()?;
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
        Err(_) => {
            log::debug!("Failed to create ServiceManager for {}", device);
            return None;
        }
    };

    if !sm.is_present() {
        log::info!("Waiting for binder Service Manager...");
        // In real implementation, wait 60 seconds with GLib MainLoop
        // For now, just sleep and check
        std::thread::sleep(Duration::from_secs(1));
        if !sm.is_present() {
            log::error!("Service Manager never appeared");
            return None;
        }
    }

    let mut tries = 1000;
    let mut remote = sm.get_service_sync(SERVICE_NAME).ok().flatten();
    while remote.is_none() && tries > 0 {
        log::warn!("Failed to get service {}, trying again...", SERVICE_NAME);
        std::thread::sleep(Duration::from_secs(1));
        remote = sm.get_service_sync(SERVICE_NAME).ok().flatten();
        tries -= 1;
    }

    remote.map(IPlatform::new)
}
