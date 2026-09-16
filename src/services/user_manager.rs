// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use crate::config::SessionDefaults;
use crate::interfaces::{i_platform, i_user_monitor};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const DESKTOP_PREFIX: &str = "mosaic.";
const DESKTOP_CATEGORY: &str = "X-Mosaic-App";

static STOPPING: AtomicBool = AtomicBool::new(true);

const SYSTEM_APPS: [&str; 15] = [
    "com.android.calculator2",
    "com.android.camera2",
    "com.android.contacts",
    "com.android.deskclock",
    "com.android.documentsui",
    "com.android.email",
    "com.android.gallery3d",
    "com.android.inputmethod.latin",
    "com.android.settings",
    "com.google.android.gms",
    "org.lineageos.aperture",
    "org.lineageos.eleven",
    "org.lineageos.etar",
    "org.lineageos.jelly",
    "org.lineageos.recorder",
];

/// Line based desktop entry editor. Preserves comments and unknown keys, the
/// same intent as the GLib KEEP_COMMENTS flag the original used.
struct DesktopFile {
    lines: Vec<String>,
}

impl DesktopFile {
    fn load(path: &Path) -> Self {
        let lines = std::fs::read_to_string(path)
            .map(|s| s.lines().map(|l| l.to_string()).collect())
            .unwrap_or_default();
        Self { lines }
    }

    fn section_bounds(&self, section: &str) -> (Option<usize>, usize) {
        let header = format!("[{}]", section);
        let start = self.lines.iter().position(|l| l.trim() == header);
        let end = match start {
            Some(start) => self
                .lines
                .iter()
                .skip(start + 1)
                .position(|l| l.trim().starts_with('['))
                .map(|off| start + 1 + off)
                .unwrap_or(self.lines.len()),
            None => self.lines.len(),
        };
        (start, end)
    }

    fn get(&self, section: &str, key: &str) -> Option<String> {
        let (start, end) = self.section_bounds(section);
        let start = start?;
        for line in &self.lines[start + 1..end] {
            let line = line.trim();
            if let Some(eq) = line.find('=') {
                if line[..eq].trim() == key {
                    return Some(line[eq + 1..].trim().to_string());
                }
            }
        }
        None
    }

    fn has(&self, section: &str, key: &str) -> bool {
        self.get(section, key).is_some()
    }

    fn set(&mut self, section: &str, key: &str, value: &str) {
        let (start, end) = self.section_bounds(section);
        if let Some(start) = start {
            for index in start + 1..end {
                let line = self.lines[index].trim();
                if let Some(eq) = line.find('=') {
                    if line[..eq].trim() == key {
                        self.lines[index] = format!("{}={}", key, value);
                        return;
                    }
                }
            }
            self.lines.insert(end, format!("{}={}", key, value));
        } else {
            if !self.lines.is_empty() {
                self.lines.push(String::new());
            }
            self.lines.push(format!("[{}]", section));
            self.lines.push(format!("{}={}", key, value));
        }
    }

    fn remove_section(&mut self, section: &str) {
        let (Some(start), end) = self.section_bounds(section) else {
            return;
        };
        // Also drop a single blank separator line before the header.
        let from = if start > 0 && self.lines[start - 1].trim().is_empty() {
            start - 1
        } else {
            start
        };
        self.lines.drain(from..end);
    }

    /// Prepend entries to a list value, keeping existing ones and order.
    fn prepend_list(&mut self, section: &str, key: &str, items: &[&str]) {
        let mut current: Vec<String> = self
            .get(section, key)
            .map(|v| {
                v.split(';')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();
        for item in items.iter().rev() {
            if !current.iter().any(|c| c == item) {
                current.insert(0, (*item).to_string());
            }
        }
        let value = if current.is_empty() {
            String::new()
        } else {
            format!("{};", current.join(";"))
        };
        self.set(section, key, &value);
    }

    fn remove_from_list(&mut self, section: &str, key: &str, item: &str) {
        if let Some(value) = self.get(section, key) {
            let kept: Vec<&str> = value
                .split(';')
                .filter(|s| !s.is_empty() && *s != item)
                .collect();
            let value = if kept.is_empty() {
                String::new()
            } else {
                format!("{};", kept.join(";"))
            };
            self.set(section, key, &value);
        }
    }

    fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut body = self.lines.join("\n");
        body.push('\n');
        std::fs::write(path, body)
    }
}

fn prepend_categories(file: &mut DesktopFile) {
    file.prepend_list("Desktop Entry", "Categories", &[DESKTOP_CATEGORY]);
}

fn update_desktop_file(apps_dir: &Path, icons_dir: &Path, app: &i_platform::AppInfo) {
    let package = &app.package_name;
    let path = apps_dir.join(format!("{}{}.desktop", DESKTOP_PREFIX, package));

    let show_app = app
        .categories
        .iter()
        .any(|c| c.trim() == "android.intent.category.LAUNCHER");
    if !show_app {
        let _ = std::fs::remove_file(&path);
        return;
    }

    let mut file = DesktopFile::load(&path);
    file.set("Desktop Entry", "Type", "Application");
    file.set("Desktop Entry", "Name", &app.name);
    file.set(
        "Desktop Entry",
        "Exec",
        &format!("mosaic app launch {}", package),
    );
    file.set(
        "Desktop Entry",
        "Icon",
        &icons_dir.join(format!("{}.png", package)).to_string_lossy(),
    );
    prepend_categories(&mut file);
    file.set(
        "Desktop Entry",
        "X-Purism-FormFactor",
        "Workstation;Mobile;",
    );
    file.prepend_list("Desktop Entry", "Actions", &["app-settings"]);
    if SYSTEM_APPS.contains(&package.as_str()) && !file.has("Desktop Entry", "NoDisplay") {
        file.set("Desktop Entry", "NoDisplay", "true");
    }

    file.set("Desktop Action app-settings", "Name", "App Settings");
    file.set(
        "Desktop Action app-settings",
        "Exec",
        &format!(
            "mosaic app intent android.settings.APPLICATION_DETAILS_SETTINGS package:{}",
            package
        ),
    );
    file.set(
        "Desktop Action app-settings",
        "Icon",
        &icons_dir.join("com.android.settings.png").to_string_lossy(),
    );

    if let Err(e) = file.save(&path) {
        log::debug!("Failed to write {}: {}", path.display(), e);
    }
}

/// Remove desktop files that no longer have a launcher entry.
fn prune_desktop_files(apps_dir: &Path, apps: &[i_platform::AppInfo]) {
    let expected: Vec<String> = apps
        .iter()
        .map(|a| format!("{}{}.desktop", DESKTOP_PREFIX, a.package_name))
        .collect();
    if let Ok(entries) = std::fs::read_dir(apps_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(DESKTOP_PREFIX)
                && name.ends_with(".desktop")
                && !expected.contains(&name)
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Drop the `app_settings` action the old layout added.
fn user_migration(apps_dir: &Path, user_state_dir: &Path) {
    let has_legacy = std::fs::read_dir(apps_dir)
        .map(|entries| {
            entries.filter_map(|e| e.ok()).any(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                n.starts_with("waydroid.") && n.ends_with(".desktop")
            })
        })
        .unwrap_or(false);
    if !has_legacy {
        return;
    }

    let migrated_main = user_state_dir.join(".migrated-main-desktop-file");
    if !migrated_main.exists() {
        let _ = std::fs::remove_file(apps_dir.join("Waydroid.desktop"));
        let _ = std::fs::remove_file(apps_dir.join("Mosaic.desktop"));
        let _ = std::fs::File::create(&migrated_main);
    }

    let migrated_apps = user_state_dir.join(".migrated-app-settings-desktop-action");
    if !migrated_apps.exists() {
        if let Ok(entries) = std::fs::read_dir(apps_dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.starts_with("waydroid.") || !name.ends_with(".desktop") {
                    continue;
                }
                let path = entry.path();
                let mut file = DesktopFile::load(&path);
                file.remove_section("Desktop Action app_settings");
                file.remove_from_list("Desktop Entry", "Actions", "app_settings");
                let _ = file.save(&path);
            }
        }
        let _ = std::fs::File::create(&migrated_apps);
    }
}

fn user_unlocked(
    args: &MosaicArgs,
    apps_dir: &Path,
    icons_dir: &Path,
    unlocked_cb: &Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
) {
    let cfg = crate::config::load(&args.config);
    log::info!("Android user is ready");

    if cfg.mosaic.get("auto_adb").map(|v| v.as_str()) == Some("True") {
        let _ = crate::helpers::net::adb_connect(args);
    }

    if let Some(platform) = i_platform::get_service(args) {
        let apps = platform.get_apps_info();
        for app in &apps {
            update_desktop_file(apps_dir, icons_dir, app);
        }
        prune_desktop_files(apps_dir, &apps);
    }
    if let Some(cb) = unlocked_cb {
        cb();
    }
}

pub fn start(
    args: &MosaicArgs,
    session: &SessionDefaults,
    unlocked_cb: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
) -> anyhow::Result<()> {
    let apps_dir: PathBuf = Path::new(&session.xdg_data_home).join("applications");
    std::fs::create_dir_all(&apps_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&apps_dir, std::fs::Permissions::from_mode(0o700));
    }

    let user_state_dir = PathBuf::from(&session.mosaic_user_state);
    std::fs::create_dir_all(&user_state_dir)?;
    let icons_dir = Path::new(&session.mosaic_data).join("icons");

    user_migration(&apps_dir, &user_state_dir);

    STOPPING.store(false, Ordering::SeqCst);
    let args = args.clone();
    let unlocked_cb = std::sync::Arc::new(unlocked_cb);
    std::thread::spawn(move || {
        while !STOPPING.load(Ordering::SeqCst) {
            let unlocked_args = args.clone();
            let unlocked_apps = apps_dir.clone();
            let unlocked_icons = icons_dir.clone();
            let unlocked_cb = unlocked_cb.clone();
            let package_args = args.clone();
            let package_apps = apps_dir.clone();
            let package_icons = icons_dir.clone();
            i_user_monitor::add_service(
                &args,
                move |_uid| {
                    user_unlocked(
                        &unlocked_args,
                        &unlocked_apps,
                        &unlocked_icons,
                        &unlocked_cb,
                    )
                },
                move |mode, package_name, _uid| {
                    if mode == i_user_monitor::PACKAGE_REMOVED {
                        let path = package_apps
                            .join(format!("{}{}.desktop", DESKTOP_PREFIX, package_name));
                        let _ = std::fs::remove_file(path);
                    } else if let Some(platform) = i_platform::get_service(&package_args) {
                        if let Some(app) = platform.get_app_info(&package_name) {
                            update_desktop_file(&package_apps, &package_icons, &app);
                        }
                    }
                },
                &STOPPING,
            );
        }
    });
    Ok(())
}

pub fn stop(_args: &MosaicArgs) -> anyhow::Result<()> {
    STOPPING.store(true, Ordering::SeqCst);
    Ok(())
}
