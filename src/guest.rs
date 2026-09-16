// SPDX-License-Identifier: GPL-3.0-or-later
//! Names shared with the Android system image.
//!
//! The guest is a stock Waydroid/LineageOS image, so every name the guest
//! declares or reads keeps Waydroid's spelling even though the project is
//! called Mosaic. Only host-side names use Mosaic: the binary, the config and
//! log files, the D-Bus bus names, and the host directories under
//! `/var/lib/mosaic`.
//!
//! Changing anything in this module breaks interoperability with the
//! downloadable images and requires rebuilding the guest.

/// Binder interfaces declared by the guest.
pub const IFACE_PLATFORM: &str = "lineageos.waydroid.IPlatform";
pub const IFACE_STATUS_BAR: &str = "lineageos.waydroid.IStatusBarService";
pub const IFACE_HARDWARE: &str = "lineageos.waydroid.IHardware";
pub const IFACE_CLIPBOARD: &str = "lineageos.waydroid.IClipboard";
pub const IFACE_NOTIFICATIONS: &str = "lineageos.waydroid.INotifications";
pub const IFACE_USER_MONITOR: &str = "lineageos.waydroid.IUserMonitor";
pub const IFACE_NOTIFICATION_CALLBACK: &str =
    "lineageos.waydroid.INotifications.INotificationCallback";

/// Binder service names the guest looks up and the host publishes.
pub const SVC_PLATFORM: &str = "waydroidplatform";
pub const SVC_STATUS_BAR: &str = "waydroidstatusbar";
pub const SVC_HARDWARE: &str = "waydroidhardware";
pub const SVC_CLIPBOARD: &str = "waydroidclipboard";
pub const SVC_NOTIFICATIONS: &str = "waydroidnotifications";
pub const SVC_USER_MONITOR: &str = "waydroidusermonitor";

/// Props the guest reads.
pub const PROP_HOST_USER: &str = "waydroid.host.user";
pub const PROP_HOST_UID: &str = "waydroid.host.uid";
pub const PROP_HOST_GID: &str = "waydroid.host.gid";
pub const PROP_HOST_DATA_PATH: &str = "waydroid.host_data_path";
pub const PROP_BACKGROUND_START: &str = "waydroid.background_start";
pub const PROP_XDG_RUNTIME_DIR: &str = "waydroid.xdg_runtime_dir";
pub const PROP_PULSE_RUNTIME_PATH: &str = "waydroid.pulse_runtime_path";
pub const PROP_WAYLAND_DISPLAY: &str = "waydroid.wayland_display";
pub const PROP_STUB_SENSORS_HAL: &str = "waydroid.stub_sensors_hal";
pub const PROP_SYSTEM_OTA: &str = "waydroid.system_ota";
pub const PROP_VENDOR_OTA: &str = "waydroid.vendor_ota";
pub const PROP_UPDATER_DISABLED: &str = "waydroid.updater.disabled";
pub const PROP_TOOLS_VERSION: &str = "waydroid.tools_version";
pub const PROP_ACTIVE_APPS: &str = "waydroid.active_apps";
pub const PROP_MULTI_WINDOWS: &str = "persist.waydroid.multi_windows";

/// Prop files. The guest loads the guest one from `/vendor`.
pub const PROP_FILE: &str = "waydroid.prop";
pub const BASE_PROP_FILE: &str = "waydroid_base.prop";
pub const GUEST_VENDOR_PROP: &str = "/vendor/waydroid.prop";

/// Prop prefix carrying host product identity into the guest.
pub const PRODUCT_PREFIX: &str = "ro.product.waydroid.";

/// Image channel path segment. The OTA server serves `waydroid_<arch>`.
pub const OTA_PATH_PREFIX: &str = "waydroid_";

/// Directory the guest reads a pushed APK from.
pub const TMP_DIR: &str = "waydroid_tmp";
pub const GUEST_TMP_APK: &str = "/data/waydroid_tmp/base.apk";

/// Optional host sensor daemon the guest-side HAL replaces.
pub const SENSORD_BIN: &str = "waydroid-sensord";

/// Locations that stock image packages install into.
pub const EXTRA_IMAGES_PATHS: [&str; 2] = [
    "/etc/waydroid-extra/images",
    "/usr/share/waydroid-extra/images",
];
pub const EXTRA_CHANNELS_CFG: &str = "/usr/share/waydroid-extra/channels.cfg";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_contract_uses_waydroid_names() {
        // These are the values the downloadable system image expects. If a
        // rename changes them, the port stops talking to the guest.
        assert_eq!(IFACE_PLATFORM, "lineageos.waydroid.IPlatform");
        assert_eq!(IFACE_STATUS_BAR, "lineageos.waydroid.IStatusBarService");
        assert_eq!(IFACE_HARDWARE, "lineageos.waydroid.IHardware");
        assert_eq!(IFACE_CLIPBOARD, "lineageos.waydroid.IClipboard");
        assert_eq!(IFACE_NOTIFICATIONS, "lineageos.waydroid.INotifications");
        assert_eq!(IFACE_USER_MONITOR, "lineageos.waydroid.IUserMonitor");
        assert_eq!(
            IFACE_NOTIFICATION_CALLBACK,
            "lineageos.waydroid.INotifications.INotificationCallback"
        );

        assert_eq!(SVC_PLATFORM, "waydroidplatform");
        assert_eq!(SVC_STATUS_BAR, "waydroidstatusbar");
        assert_eq!(SVC_HARDWARE, "waydroidhardware");
        assert_eq!(SVC_CLIPBOARD, "waydroidclipboard");
        assert_eq!(SVC_NOTIFICATIONS, "waydroidnotifications");
        assert_eq!(SVC_USER_MONITOR, "waydroidusermonitor");

        for prop in [
            PROP_HOST_USER,
            PROP_HOST_UID,
            PROP_HOST_GID,
            PROP_HOST_DATA_PATH,
            PROP_BACKGROUND_START,
            PROP_XDG_RUNTIME_DIR,
            PROP_PULSE_RUNTIME_PATH,
            PROP_WAYLAND_DISPLAY,
            PROP_STUB_SENSORS_HAL,
            PROP_SYSTEM_OTA,
            PROP_VENDOR_OTA,
            PROP_UPDATER_DISABLED,
            PROP_TOOLS_VERSION,
            PROP_ACTIVE_APPS,
        ] {
            assert!(
                prop.starts_with("waydroid."),
                "{} must keep the guest name",
                prop
            );
        }
        assert_eq!(PROP_MULTI_WINDOWS, "persist.waydroid.multi_windows");

        assert_eq!(PROP_FILE, "waydroid.prop");
        assert_eq!(BASE_PROP_FILE, "waydroid_base.prop");
        assert_eq!(GUEST_VENDOR_PROP, "/vendor/waydroid.prop");
        assert_eq!(GUEST_TMP_APK, "/data/waydroid_tmp/base.apk");
        assert_eq!(SENSORD_BIN, "waydroid-sensord");
        assert_eq!(PRODUCT_PREFIX, "ro.product.waydroid.");
        assert_eq!(OTA_PATH_PREFIX, "waydroid_");
    }
}
