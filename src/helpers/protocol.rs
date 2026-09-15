// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;

pub fn set_aidl_version(args: &MosaicArgs) -> anyhow::Result<()> {
    let mut cfg = crate::config::load(&args.config);
    let defaults = crate::config::Defaults::new();
    let build_prop = format!("{}/system/build.prop", defaults.rootfs);
    let android_api: u32 = crate::helpers::props::file_get(&build_prop, "ro.build.version.sdk")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    if android_api == 0 {
        log::error!("Failed to parse android version from system.img");
    }

    let (binder_protocol, sm_protocol) = if android_api < 28 {
        ("aidl", "aidl")
    } else if android_api < 30 {
        ("aidl2", "aidl2")
    } else if android_api < 31 {
        ("aidl3", "aidl3")
    } else if android_api < 33 {
        ("aidl4", "aidl3")
    } else if android_api < 35 {
        ("aidl3", "aidl3")
    } else if android_api < 36 {
        ("aidl3", "aidl5")
    } else {
        ("aidl3", "aidl6")
    };

    cfg.mosaic
        .insert("binder_protocol".to_string(), binder_protocol.to_string());
    cfg.mosaic.insert(
        "service_manager_protocol".to_string(),
        sm_protocol.to_string(),
    );
    crate::config::save(&args.config, &cfg)?;
    Ok(())
}
