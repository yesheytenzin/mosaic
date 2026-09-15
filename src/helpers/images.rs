// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use sha2::{Digest, Sha256};
use std::path::Path;

pub fn sha256sum(path: &str) -> anyhow::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hex::encode(hasher.finalize()))
}

pub async fn get(args: &MosaicArgs) -> anyhow::Result<()> {
    let cfg = crate::config::load(&args.config);
    let system_ota = cfg.mosaic.get("system_ota").cloned().unwrap_or_default();
    let (status, body) = crate::helpers::http::retrieve(&system_ota).await?;
    if status != 200 {
        anyhow::bail!(
            "Failed to get system OTA channel: {}, error: {}",
            system_ota,
            status
        );
    }
    let json: serde_json::Value = serde_json::from_slice(&body)?;
    let responses = json["response"].as_array().cloned().unwrap_or_default();
    if responses.is_empty() {
        anyhow::bail!("No images found on system channel");
    }
    let current_dt: i64 = cfg
        .mosaic
        .get("system_datetime")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    for resp in responses {
        let dt = resp["datetime"].as_i64().unwrap_or(0);
        if dt > current_dt {
            let url = resp["url"].as_str().unwrap_or("");
            let filename = resp["filename"].as_str().unwrap_or("system.zip");
            let expected_id = resp["id"].as_str().unwrap_or("");
            let images_path = cfg
                .mosaic
                .get("images_path")
                .cloned()
                .unwrap_or_else(|| format!("{}/images", args.work));
            let dest = crate::helpers::http::download(url, filename, false, &args.work).await?;
            log::info!("Validating system image");
            let sum = sha256sum(&dest)?;
            if sum != expected_id {
                let _ = std::fs::remove_file(&dest);
                anyhow::bail!(
                    "Downloaded system image hash doesn't match, expected: {}",
                    expected_id
                );
            }
            log::info!("Extracting to {}", images_path);
            let file = std::fs::File::open(&dest)?;
            let mut zip = zip::ZipArchive::new(file)?;
            zip.extract(&images_path)?;
            let mut cfg2 = crate::config::load(&args.config);
            cfg2.mosaic
                .insert("system_datetime".to_string(), dt.to_string());
            crate::config::save(&args.config, &cfg2)?;
            let _ = std::fs::remove_file(&dest);
            break;
        }
    }

    let cfg2 = crate::config::load(&args.config);
    let vendor_ota = cfg2.mosaic.get("vendor_ota").cloned().unwrap_or_default();
    let (status, body) = crate::helpers::http::retrieve(&vendor_ota).await?;
    if status != 200 {
        anyhow::bail!(
            "Failed to get vendor OTA channel: {}, error: {}",
            vendor_ota,
            status
        );
    }
    let json: serde_json::Value = serde_json::from_slice(&body)?;
    let responses = json["response"].as_array().cloned().unwrap_or_default();
    if responses.is_empty() {
        anyhow::bail!("No images found on vendor channel");
    }
    let current_dt: i64 = cfg2
        .mosaic
        .get("vendor_datetime")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    for resp in responses {
        let dt = resp["datetime"].as_i64().unwrap_or(0);
        if dt > current_dt {
            let url = resp["url"].as_str().unwrap_or("");
            let filename = resp["filename"].as_str().unwrap_or("vendor.zip");
            let expected_id = resp["id"].as_str().unwrap_or("");
            let images_path = cfg2
                .mosaic
                .get("images_path")
                .cloned()
                .unwrap_or_else(|| format!("{}/images", args.work));
            let dest = crate::helpers::http::download(url, filename, false, &args.work).await?;
            log::info!("Validating vendor image");
            let sum = sha256sum(&dest)?;
            if sum != expected_id {
                let _ = std::fs::remove_file(&dest);
                anyhow::bail!(
                    "Downloaded vendor image hash doesn't match, expected: {}",
                    expected_id
                );
            }
            log::info!("Extracting to {}", images_path);
            let file = std::fs::File::open(&dest)?;
            let mut zip = zip::ZipArchive::new(file)?;
            zip.extract(&images_path)?;
            let mut cfg3 = crate::config::load(&args.config);
            cfg3.mosaic
                .insert("vendor_datetime".to_string(), dt.to_string());
            crate::config::save(&args.config, &cfg3)?;
            let _ = std::fs::remove_file(&dest);
            break;
        }
    }

    remove_overlay(args)?;
    Ok(())
}

pub fn validate(args: &MosaicArgs, channel: &str, path: &str) -> bool {
    let cfg = crate::config::load(&args.config);
    let channel_url = cfg.mosaic.get(channel).cloned().unwrap_or_default();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    if let Ok(rt) = rt {
        if let Ok((status, body)) = rt.block_on(crate::helpers::http::retrieve(&channel_url)) {
            if status != 200 {
                return false;
            }
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body) {
                if let Some(arr) = json["response"].as_array() {
                    if let Ok(sum) = sha256sum(path) {
                        for build in arr {
                            if build["id"].as_str() == Some(&sum) {
                                return true;
                            }
                        }
                        log::warn!(
                            "Could not verify the image {} against {}",
                            path,
                            channel_url
                        );
                    }
                }
            }
        }
    }
    false
}

pub fn replace(
    args: &MosaicArgs,
    system_zip: &str,
    system_time: &str,
    vendor_zip: &str,
    vendor_time: &str,
) -> anyhow::Result<()> {
    let mut cfg = crate::config::load(&args.config);
    let images_path = cfg
        .mosaic
        .get("images_path")
        .cloned()
        .unwrap_or_else(|| format!("{}/images", args.work));
    if Path::new(system_zip).exists() {
        if validate(args, "system_ota", system_zip) {
            let file = std::fs::File::open(system_zip)?;
            let mut zip = zip::ZipArchive::new(file)?;
            zip.extract(&images_path)?;
            cfg.mosaic
                .insert("system_datetime".to_string(), system_time.to_string());
        } else {
            log::warn!("Failed to validate update system image, ignoring");
        }
        let _ = std::fs::remove_file(system_zip);
    }
    if Path::new(vendor_zip).exists() {
        if validate(args, "vendor_ota", vendor_zip) {
            let file = std::fs::File::open(vendor_zip)?;
            let mut zip = zip::ZipArchive::new(file)?;
            zip.extract(&images_path)?;
            cfg.mosaic
                .insert("vendor_datetime".to_string(), vendor_time.to_string());
        } else {
            log::warn!("Failed to validate update vendor image, ignoring");
        }
        let _ = std::fs::remove_file(vendor_zip);
    }
    crate::config::save(&args.config, &cfg)?;
    remove_overlay(args)?;
    Ok(())
}

pub fn remove_overlay(args: &MosaicArgs) -> anyhow::Result<()> {
    for p in [
        format!("{}/overlay_rw", args.work),
        format!("{}/overlay_work", args.work),
    ] {
        if Path::new(&p).exists() {
            std::fs::remove_dir_all(&p)?;
        }
    }
    Ok(())
}

pub fn make_prop(
    args: &MosaicArgs,
    session: &crate::config::SessionDefaults,
    full_props_path: &str,
) -> anyhow::Result<()> {
    let base = std::fs::read_to_string(format!("{}/mosaic_base.prop", args.work))?;
    let mut props: Vec<String> = base
        .lines()
        .map(|l| l.to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if props.is_empty() {
        anyhow::bail!("mosaic_base.prop is broken!!?");
    }
    let cfg = crate::config::load(&args.config);
    let mut add = |key: &str, cfg_key: &str| {
        let value = match cfg_key {
            "user_name" => session.user_name.clone(),
            "user_id" => session.user_id.clone(),
            "group_id" => session.group_id.clone(),
            "mosaic_data" => session.mosaic_data.clone(),
            "background_start" => session.background_start.clone(),
            _ => cfg
                .mosaic
                .get(cfg_key)
                .cloned()
                .unwrap_or_else(|| "None".to_string()),
        };
        if value != "None" {
            let value = value.replace("/mnt/", "/mnt_extra/");
            props.push(format!("{}={}", key, value));
        }
    };
    add("mosaic.host.user", "user_name");
    add("mosaic.host.uid", "user_id");
    add("mosaic.host.gid", "group_id");
    add("mosaic.host_data_path", "mosaic_data");
    add("mosaic.background_start", "background_start");
    props.push(format!(
        "mosaic.xdg_runtime_dir={}",
        crate::config::Defaults::new().container_xdg_runtime_dir
    ));
    props.push(format!(
        "mosaic.pulse_runtime_path={}",
        crate::config::Defaults::new().container_pulse_runtime_path
    ));
    props.push(format!(
        "mosaic.wayland_display={}",
        crate::config::Defaults::new().container_wayland_display
    ));
    if which::which("mosaic-sensord").is_err() {
        props.push("mosaic.stub_sensors_hal=1".to_string());
    }
    let dpi = session.lcd_density.clone();
    if dpi != "0" {
        props.push(format!("ro.sf.lcd_density={}", dpi));
    }
    std::fs::write(full_props_path, props.join("\n") + "\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(full_props_path, std::fs::Permissions::from_mode(0o644));
    }
    Ok(())
}

pub fn mount_rootfs(
    args: &MosaicArgs,
    images_dir: &str,
    session: &crate::config::SessionDefaults,
) -> anyhow::Result<()> {
    let cfg = crate::config::load(&args.config);
    let defaults = crate::config::Defaults::new();
    crate::helpers::mount::mount(
        args,
        &format!("{}/system.img", images_dir),
        &defaults.rootfs,
        true,
        true,
        true,
        None,
        None,
        true,
    )?;
    if cfg.mosaic.get("mount_overlays").map(|s| s.as_str()) == Some("True") {
        let res = crate::helpers::mount::mount_overlay(
            args,
            &[defaults.overlay.clone(), defaults.rootfs.clone()],
            &defaults.rootfs,
            Some(&format!("{}/system", defaults.overlay_rw)),
            Some(&format!("{}/system", defaults.overlay_work)),
            true,
            true,
        );
        if res.is_err() {
            let mut cfg2 = cfg;
            cfg2.mosaic
                .insert("mount_overlays".to_string(), "False".to_string());
            crate::config::save(&args.config, &cfg2)?;
            log::warn!("Mounting overlays failed. The feature has been disabled.");
        }
    }
    crate::helpers::mount::mount(
        args,
        &format!("{}/vendor.img", images_dir),
        &format!("{}/vendor", defaults.rootfs),
        true,
        false,
        true,
        None,
        None,
        true,
    )?;
    if crate::config::load(&args.config)
        .mosaic
        .get("mount_overlays")
        .map(|s| s.as_str())
        == Some("True")
    {
        crate::helpers::mount::mount_overlay(
            args,
            &[
                format!("{}/vendor", defaults.overlay),
                format!("{}/vendor", defaults.rootfs),
            ],
            &format!("{}/vendor", defaults.rootfs),
            Some(&format!("{}/vendor", defaults.overlay_rw)),
            Some(&format!("{}/vendor", defaults.overlay_work)),
            true,
            true,
        )?;
    }
    for egl_path in ["/vendor/lib/egl", "/vendor/lib64/egl"] {
        if Path::new(egl_path).is_dir() {
            crate::helpers::mount::bind(
                args,
                egl_path,
                &format!("{}{}", defaults.rootfs, egl_path),
                true,
                false,
            )?;
        }
    }
    if crate::helpers::mount::is_mount("/odm") {
        crate::helpers::mount::bind(
            args,
            "/odm",
            &format!("{}/odm_extra", defaults.rootfs),
            true,
            false,
        )?;
    } else if Path::new("/vendor/odm").is_dir() {
        crate::helpers::mount::bind(
            args,
            "/vendor/odm",
            &format!("{}/odm_extra", defaults.rootfs),
            true,
            false,
        )?;
    }
    make_prop(args, session, &format!("{}/mosaic.prop", args.work))?;
    crate::helpers::mount::bind_file(
        args,
        &format!("{}/mosaic.prop", args.work),
        &format!("{}/vendor/mosaic.prop", defaults.rootfs),
        false,
    )?;
    Ok(())
}

pub fn umount_rootfs(args: &MosaicArgs) -> anyhow::Result<()> {
    crate::helpers::mount::umount_all(args, &crate::config::Defaults::new().rootfs)
}
