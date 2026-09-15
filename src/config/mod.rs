// SPDX-License-Identifier: GPL-3.0-or-later

pub mod load;
pub mod save;

pub use load::{load, load_channels};
pub use save::save;

pub const VERSION: &str = "0.0.1";

pub const CONFIG_KEYS: &[&str] = &[
    "arch",
    "images_path",
    "vendor_type",
    "system_datetime",
    "vendor_datetime",
    "suspend_action",
    "mount_overlays",
    "auto_adb",
];

pub const CHANNELS_CONFIG_KEYS: &[&str] = &[
    "system_channel",
    "vendor_channel",
    "rom_type",
    "system_type",
];

#[derive(Debug, Clone)]
pub struct Defaults {
    pub work: String,
    pub images_path: String,
    pub rootfs: String,
    pub overlay: String,
    pub overlay_rw: String,
    pub overlay_work: String,
    pub data: String,
    pub lxc: String,
    pub host_perms: String,
    pub container_xdg_runtime_dir: String,
    pub container_wayland_display: String,
    pub container_pulse_runtime_path: String,
    pub preinstalled_images_paths: Vec<String>,
    pub arch: String,
    pub vendor_type: String,
    pub system_datetime: String,
    pub vendor_datetime: String,
    pub suspend_action: String,
    pub mount_overlays: String,
    pub auto_adb: String,
}

impl Defaults {
    pub fn new() -> Self {
        let work = "/var/lib/mosaic".to_string();
        Self {
            images_path: format!("{}/images", work),
            rootfs: format!("{}/rootfs", work),
            overlay: format!("{}/overlay", work),
            overlay_rw: format!("{}/overlay_rw", work),
            overlay_work: format!("{}/overlay_work", work),
            data: format!("{}/data", work),
            lxc: format!("{}/lxc", work),
            host_perms: format!("{}/host-permissions", work),
            container_xdg_runtime_dir: "/run/xdg".to_string(),
            container_wayland_display: "wayland-0".to_string(),
            container_pulse_runtime_path: "/run/xdg/pulse".to_string(),
            preinstalled_images_paths: vec![
                "/etc/mosaic-extra/images".to_string(),
                "/usr/share/mosaic-extra/images".to_string(),
            ],
            work,
            arch: "arm64".to_string(),
            vendor_type: "MAINLINE".to_string(),
            system_datetime: "0".to_string(),
            vendor_datetime: "0".to_string(),
            suspend_action: "freeze".to_string(),
            mount_overlays: "True".to_string(),
            auto_adb: "False".to_string(),
        }
    }

    pub fn tools_src() -> String {
        // Prefer env MOSAIC_TOOLS_SRC, then exe parent, then /usr/lib/mosaic.
        if let Ok(v) = std::env::var("MOSAIC_TOOLS_SRC") {
            return v;
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                let candidate = parent.join("../lib/mosaic");
                if candidate.exists() {
                    if let Ok(p) = candidate.canonicalize() {
                        return p.to_string_lossy().to_string();
                    }
                }
            }
        }
        "/usr/lib/mosaic".to_string()
    }
}

impl Default for Defaults {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct SessionDefaults {
    pub user_name: String,
    pub user_id: String,
    pub group_id: String,
    pub host_user: String,
    pub pid: String,
    pub xdg_data_home: String,
    pub xdg_runtime_dir: String,
    pub wayland_display: String,
    pub pulse_runtime_path: String,
    pub state: String,
    pub lcd_density: String,
    pub background_start: String,
    pub mosaic_user_state: String,
    pub mosaic_data: String,
}

impl SessionDefaults {
    pub fn new() -> Self {
        let uid = nix::unistd::getuid();
        let gid = nix::unistd::getgid();
        let user_name = nix::unistd::User::from_uid(uid)
            .ok()
            .flatten()
            .map(|u| u.name)
            .unwrap_or_else(|| "unknown".to_string());
        let host_user = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        let xdg_data_home = std::env::var("XDG_DATA_HOME")
            .unwrap_or_else(|_| format!("{}/.local/share", host_user));
        let xdg_runtime_dir =
            std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "None".to_string());
        let wayland_display =
            std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "None".to_string());
        let mut pulse_runtime_path =
            std::env::var("PULSE_RUNTIME_PATH").unwrap_or_else(|_| "None".to_string());
        if pulse_runtime_path == "None" {
            if xdg_runtime_dir != "None" {
                pulse_runtime_path = format!("{}/pulse", xdg_runtime_dir);
            } else {
                pulse_runtime_path = "None".to_string();
            }
        }
        let mosaic_user_state = format!("{}/mosaic", xdg_data_home);
        let mosaic_data = format!("{}/data", mosaic_user_state);
        Self {
            user_name,
            user_id: uid.to_string(),
            group_id: gid.to_string(),
            host_user,
            pid: std::process::id().to_string(),
            xdg_data_home,
            xdg_runtime_dir,
            wayland_display,
            pulse_runtime_path,
            state: "STOPPED".to_string(),
            lcd_density: "0".to_string(),
            background_start: "true".to_string(),
            mosaic_user_state,
            mosaic_data,
        }
    }
}

impl Default for SessionDefaults {
    fn default() -> Self {
        Self::new()
    }
}

pub fn channels_defaults() -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    m.insert(
        "config_path".to_string(),
        "/usr/share/mosaic-extra/channels.cfg".to_string(),
    );
    m.insert(
        "system_channel".to_string(),
        "https://ota.waydro.id/system".to_string(),
    );
    m.insert(
        "vendor_channel".to_string(),
        "https://ota.waydro.id/vendor".to_string(),
    );
    m.insert("rom_type".to_string(), "lineage".to_string());
    m.insert("system_type".to_string(), "VANILLA".to_string());
    m
}
