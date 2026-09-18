// SPDX-License-Identifier: GPL-3.0-or-later

//! Configuration for the native-execution model.
//!
//! The file lives at `<work>/mosaic.cfg` and holds only host-side settings: the
//! runtime bundle version to use and the reserved UID range for per-app
//! isolation (ADR-0007). There is no image path, no LXC path and no guest
//! contract any more.

pub mod load;

pub use load::load;

/// Single source of truth for the version, taken from the package metadata so
/// it cannot drift from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Keys persisted in the config file.
pub const CONFIG_KEYS: &[&str] = &[
    "bundle_channel",
    "bundle_version",
    "uid_range_start",
    "uid_range_end",
];

#[derive(Debug, Clone)]
pub struct Defaults {
    pub work: String,
    pub apps_dir: String,
    pub runtime_dir: String,
    /// Where the runtime bundle is fetched from, minus the version segment.
    pub bundle_channel: String,
    pub bundle_version: String,
    /// Reserved range for per-app system users. Real UIDs, per ADR-0007.
    pub uid_range_start: u32,
    pub uid_range_end: u32,
}

/// Where Mosaic keeps its own state: the registry, the broker socket and the
/// runtime bundle. The packaged install uses `/var/lib/mosaic`; an unprivileged
/// run without that directory falls back to the user's data directory, so
/// `mosaic` works from a checkout before any packaging step.
fn default_work() -> String {
    if let Ok(work) = std::env::var("MOSAIC_WORK") {
        return work;
    }
    let system = "/var/lib/mosaic";
    if writable(system) {
        return system.to_string();
    }
    format!("{}/mosaic", data_home())
}

/// Whether a directory can actually be written in.
///
/// Not `access(W_OK)`: inside a systemd sandbox with `ProtectSystem=strict` the
/// mount is read-only and `access` still succeeds on it, because it checks
/// permissions and not the mount. The broker is a user service, so it runs inside
/// exactly that sandbox, picks `/var/lib/mosaic` on the strength of the check, and
/// then fails to write the registry with `EROFS`. Creating a file is the only
/// question worth asking.
fn writable(dir: &str) -> bool {
    let probe = format!("{}/.mosaic-write-probe", dir);
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn data_home() -> String {
    if let Ok(home) = std::env::var("XDG_DATA_HOME") {
        if !home.is_empty() {
            return home;
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    format!("{}/.local/share", home)
}

impl Defaults {
    pub fn new() -> Self {
        let work = default_work();
        Self {
            apps_dir: format!("{}/apps", work),
            runtime_dir: format!("{}/runtime", work),
            // Where a published bundle is fetched from. The project's own releases by
            // default, so that `runtime fetch` works with no configuration; publishing
            // one is `tools/publish-runtime.sh`. Overridable, because the artifact is
            // per release and may be hosted anywhere -- a mirror, or a directory on a
            // local network while an image is being moved between machines.
            bundle_channel: std::env::var("MOSAIC_BUNDLE_CHANNEL").unwrap_or_else(|_| {
                "https://github.com/yesheytenzin/mosaic/releases/latest/download".to_string()
            }),
            // What `pack` names the archive after, from the bundle's own version
            // marker. Overridable, because a release may be tagged differently.
            bundle_version: "local".to_string(),
            uid_range_start: 5000,
            uid_range_end: 5999,
            work,
        }
    }
}
impl Default for Defaults {
    fn default() -> Self {
        Self::new()
    }
}

/// Session values the app process needs: where to find the Wayland socket and
/// whose desktop it belongs to.
#[derive(Debug, Clone)]
pub struct SessionDefaults {
    pub user_name: String,
    pub user_id: String,
    pub group_id: String,
    pub host_user: String,
    pub xdg_data_home: String,
    pub xdg_runtime_dir: String,
    pub wayland_display: String,
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
        Self {
            user_name,
            user_id: uid.to_string(),
            group_id: gid.to_string(),
            host_user,
            xdg_data_home,
            xdg_runtime_dir: std::env::var("XDG_RUNTIME_DIR").unwrap_or_default(),
            wayland_display: std::env::var("WAYLAND_DISPLAY").unwrap_or_default(),
        }
    }
}

impl Default for SessionDefaults {
    fn default() -> Self {
        Self::new()
    }
}
