// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::Path;

#[derive(Debug, Clone)]
pub struct ServiceManager {
    pub device: String,
    pub protocol: String,
    pub binder_protocol: String,
}

impl ServiceManager {
    pub fn new(
        device: &str,
        protocol: Option<&str>,
        binder_protocol: Option<&str>,
    ) -> anyhow::Result<Self> {
        if !Path::new(device).exists() {
            anyhow::bail!("Binder device {} not found", device);
        }
        Ok(Self {
            device: device.to_string(),
            protocol: protocol.unwrap_or("aidl").to_string(),
            binder_protocol: binder_protocol.unwrap_or("aidl").to_string(),
        })
    }

    pub fn is_present(&self) -> bool {
        Path::new(&self.device).exists()
    }

    pub fn get_service_sync(&self, _name: &str) -> anyhow::Result<Option<RemoteObject>> {
        // In real implementation, this would call gbinder_servicemanager_get_service_sync
        // For now, return None to indicate service not available (as when container not running)
        // The caller will retry and eventually fail gracefully
        Ok(None)
    }

    pub fn add_service_sync(&self, _name: &str, _object: LocalObject) -> anyhow::Result<()> {
        Ok(())
    }

    pub fn new_local_object<F>(&self, _interface: &str, _handler: F) -> LocalObject
    where
        F: Fn(&[u8], u32, u32) -> (Vec<u8>, i32) + Send + Sync + 'static,
    {
        LocalObject {}
    }

    pub fn add_presence_handler<F>(&self, _handler: F) -> u32
    where
        F: Fn() + Send + Sync + 'static,
    {
        0
    }

    pub fn remove_handler(&self, _id: u32) {}

    pub fn list_sync(&self) -> Vec<String> {
        vec![]
    }
}

#[derive(Debug, Clone)]
pub struct RemoteObject {
    pub service: String,
}

#[derive(Debug, Clone)]
pub struct LocalObject {}

#[derive(Debug, Clone)]
pub struct Client {
    pub remote: RemoteObject,
    pub interface: String,
}

impl Client {
    pub fn new(remote: RemoteObject, interface: &str) -> Self {
        Self {
            remote,
            interface: interface.to_string(),
        }
    }

    pub fn new_request(&self) -> Writer {
        Writer { data: Vec::new() }
    }

    pub fn transact_sync_reply(
        &self,
        _code: u32,
        _request: Writer,
    ) -> anyhow::Result<(Reader, i32)> {
        anyhow::bail!("Binder transact not available (no container)")
    }
}

#[derive(Debug, Clone)]
pub struct Writer {
    pub data: Vec<u8>,
}

impl Writer {
    pub fn append_string16(&mut self, _s: &str) {}
    pub fn append_int32(&mut self, _v: i32) {}
    pub fn append_int64(&mut self, _v: i64) {}
    pub fn append_bool(&mut self, _v: bool) {}
    pub fn append_byte(&mut self, _v: u8) {}
    pub fn append_byte_array(&mut self, _data: &[u8]) {}
    pub fn append_object(&mut self, _obj: Option<&RemoteObject>) {}
}

#[derive(Debug, Clone)]
pub struct Reader {
    pub data: Vec<u8>,
    pub pos: usize,
}

impl Reader {
    pub fn init_reader(_data: &[u8]) -> Self {
        Self {
            data: vec![],
            pos: 0,
        }
    }

    pub fn read_int32(&mut self) -> anyhow::Result<(i32, i32)> {
        anyhow::bail!("No data")
    }

    pub fn read_int64(&mut self) -> anyhow::Result<(i32, i64)> {
        anyhow::bail!("No data")
    }

    pub fn read_string16(&mut self) -> anyhow::Result<String> {
        anyhow::bail!("No data")
    }

    pub fn read_bool(&mut self) -> anyhow::Result<(i32, bool)> {
        anyhow::bail!("No data")
    }

    pub fn read_byte(&mut self) -> anyhow::Result<(i32, u8)> {
        anyhow::bail!("No data")
    }

    pub fn read_byte_array(&mut self) -> anyhow::Result<Vec<u8>> {
        anyhow::bail!("No data")
    }

    pub fn read_object(&mut self) -> anyhow::Result<Option<RemoteObject>> {
        Ok(None)
    }
}

pub fn load_binder_nodes(
    args: &crate::args::MosaicArgs,
) -> anyhow::Result<(String, String, String)> {
    let cfg = crate::config::load(&args.config);
    let binder = cfg
        .mosaic
        .get("binder")
        .cloned()
        .unwrap_or_else(|| "binder".to_string());
    let vnd = cfg
        .mosaic
        .get("vndbinder")
        .cloned()
        .unwrap_or_else(|| "vndbinder".to_string());
    let hw = cfg
        .mosaic
        .get("hwbinder")
        .cloned()
        .unwrap_or_else(|| "hwbinder".to_string());
    Ok((binder, vnd, hw))
}

pub fn wait_for_binder_service<F>(
    _device: &str,
    _service: &str,
    _timeout_secs: u64,
    _predicate: F,
) -> bool
where
    F: Fn() -> bool,
{
    // Stub: wait 60 seconds for service manager presence, but in test we just return false
    false
}
