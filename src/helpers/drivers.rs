// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use std::path::Path;

pub const BINDER_DRIVERS: &[&str] = &["anbox-binder", "puddlejumper", "bonder", "binder"];
pub const VNDBINDER_DRIVERS: &[&str] = &[
    "anbox-vndbinder",
    "vndpuddlejumper",
    "vndbonder",
    "vndbinder",
];
pub const HWBINDER_DRIVERS: &[&str] = &["anbox-hwbinder", "hwpuddlejumper", "hwbonder", "hwbinder"];

pub fn is_binderfs_loaded() -> bool {
    if let Ok(content) = std::fs::read_to_string("/proc/filesystems") {
        for line in content.lines() {
            let words: Vec<&str> = line.split_whitespace().collect();
            if words.len() >= 2 && words[1] == "binder" {
                return true;
            }
        }
    }
    false
}

pub fn probe_binder_driver(args: &MosaicArgs) -> anyhow::Result<()> {
    let mut needed = Vec::new();
    let mut has_binder = false;
    let mut has_vndbinder = false;
    let mut has_hwbinder = false;

    for node in BINDER_DRIVERS {
        if Path::new(&format!("/dev/{}", node)).exists() {
            has_binder = true;
        }
    }
    if !has_binder {
        needed.push(BINDER_DRIVERS[0]);
    }
    for node in VNDBINDER_DRIVERS {
        if Path::new(&format!("/dev/{}", node)).exists() {
            has_vndbinder = true;
        }
    }
    if !has_vndbinder {
        needed.push(VNDBINDER_DRIVERS[0]);
    }
    for node in HWBINDER_DRIVERS {
        if Path::new(&format!("/dev/{}", node)).exists() {
            has_hwbinder = true;
        }
    }
    if !has_hwbinder {
        needed.push(HWBINDER_DRIVERS[0]);
    }

    if needed.is_empty() {
        return Ok(());
    }

    if !is_binderfs_loaded() {
        let devices = needed.join(",");
        let cmd = vec![
            "modprobe".to_string(),
            "binder_linux".to_string(),
            format!("devices=\"{}\"", devices),
        ];
        let out = crate::helpers::run::user(args, &cmd, "log", true, Some(false))?;
        if !out.is_empty() {
            log::error!("Failed to load binder driver");
            log::error!("{}", out.trim());
        }
    }

    if is_binderfs_loaded() {
        crate::helpers::run::user(
            args,
            &[
                "mkdir".to_string(),
                "-p".to_string(),
                "/dev/binderfs".to_string(),
            ],
            "log",
            false,
            Some(false),
        )?;
        crate::helpers::run::user(
            args,
            &[
                "mount".to_string(),
                "-t".to_string(),
                "binder".to_string(),
                "binder".to_string(),
                "/dev/binderfs".to_string(),
            ],
            "log",
            false,
            Some(false),
        )?;
        // alloc binder nodes via ioctl would require unsafe FFI; for now, try symlink
        if let Ok(entries) = std::fs::read_dir("/dev/binderfs") {
            let nodes: Vec<String> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.path().to_string_lossy().to_string())
                .collect();
            if !nodes.is_empty() {
                let mut cmd = vec!["ln".to_string(), "-s".to_string()];
                cmd.extend(nodes);
                cmd.push("/dev/".to_string());
                crate::helpers::run::user(args, &cmd, "log", false, Some(false))?;
            }
        }
    }

    Ok(())
}

pub fn probe_ashmem_driver(args: &MosaicArgs) -> i32 {
    if !Path::new("/dev/ashmem").exists() {
        let _ = crate::helpers::run::user(
            args,
            &[
                "modprobe".to_string(),
                "-q".to_string(),
                "ashmem_linux".to_string(),
            ],
            "log",
            false,
            Some(false),
        );
    }
    if !Path::new("/dev/ashmem").exists() {
        -1
    } else {
        0
    }
}

pub fn setup_binder_nodes(args: &mut MosaicArgs, vendor_type: &str) -> anyhow::Result<()> {
    if vendor_type == "MAINLINE" {
        probe_binder_driver(args)?;
        for node in BINDER_DRIVERS {
            if Path::new(&format!("/dev/{}", node)).exists() {
                args.cache
                    .insert("BINDER_DRIVER".to_string(), node.to_string());
                break;
            }
        }
        if !args.cache.contains_key("BINDER_DRIVER") {
            anyhow::bail!("Binder node \"binder\" for mosaic not found");
        }
        for node in VNDBINDER_DRIVERS {
            if Path::new(&format!("/dev/{}", node)).exists() {
                args.cache
                    .insert("VNDBINDER_DRIVER".to_string(), node.to_string());
                break;
            }
        }
        if !args.cache.contains_key("VNDBINDER_DRIVER") {
            anyhow::bail!("Binder node \"vndbinder\" for mosaic not found");
        }
        for node in HWBINDER_DRIVERS {
            if Path::new(&format!("/dev/{}", node)).exists() {
                args.cache
                    .insert("HWBINDER_DRIVER".to_string(), node.to_string());
                break;
            }
        }
        if !args.cache.contains_key("HWBINDER_DRIVER") {
            anyhow::bail!("Binder node \"hwbinder\" for mosaic not found");
        }
    } else {
        // HALIUM: skip last entry (binder) that is kernel binder
        for node in &BINDER_DRIVERS[..BINDER_DRIVERS.len() - 1] {
            if Path::new(&format!("/dev/{}", node)).exists() {
                args.cache
                    .insert("BINDER_DRIVER".to_string(), node.to_string());
                break;
            }
        }
        if !args.cache.contains_key("BINDER_DRIVER") {
            anyhow::bail!("Binder node \"binder\" for mosaic not found");
        }
        for node in &VNDBINDER_DRIVERS[..VNDBINDER_DRIVERS.len() - 1] {
            if Path::new(&format!("/dev/{}", node)).exists() {
                args.cache
                    .insert("VNDBINDER_DRIVER".to_string(), node.to_string());
                break;
            }
        }
        if !args.cache.contains_key("VNDBINDER_DRIVER") {
            anyhow::bail!("Binder node \"vndbinder\" for mosaic not found");
        }
        for node in &HWBINDER_DRIVERS[..HWBINDER_DRIVERS.len() - 1] {
            if Path::new(&format!("/dev/{}", node)).exists() {
                args.cache
                    .insert("HWBINDER_DRIVER".to_string(), node.to_string());
                break;
            }
        }
        if !args.cache.contains_key("HWBINDER_DRIVER") {
            anyhow::bail!("Binder node \"hwbinder\" for mosaic not found");
        }
    }
    Ok(())
}

pub fn load_binder_nodes(args: &mut MosaicArgs) -> anyhow::Result<()> {
    let cfg = crate::config::load(&args.config);
    if let Some(v) = cfg.mosaic.get("binder") {
        args.cache.insert("BINDER_DRIVER".to_string(), v.clone());
    }
    if let Some(v) = cfg.mosaic.get("vndbinder") {
        args.cache.insert("VNDBINDER_DRIVER".to_string(), v.clone());
    }
    if let Some(v) = cfg.mosaic.get("hwbinder") {
        args.cache.insert("HWBINDER_DRIVER".to_string(), v.clone());
    }
    if let Some(v) = cfg.mosaic.get("binder_protocol") {
        args.cache.insert("BINDER_PROTOCOL".to_string(), v.clone());
    }
    if let Some(v) = cfg.mosaic.get("service_manager_protocol") {
        args.cache
            .insert("SERVICE_MANAGER_PROTOCOL".to_string(), v.clone());
    }
    Ok(())
}
