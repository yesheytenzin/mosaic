// SPDX-License-Identifier: GPL-3.0-or-later

//! DRM render node and Vulkan driver detection, matching the host's GPU so the
//! guest picks the right gralloc and EGL path.

use crate::args::MosaicArgs;
use crate::helpers::props;
use std::path::Path;

/// Kernel drivers Waydroid can not drive.
const UNSUPPORTED: [&str; 1] = ["nvidia"];

pub fn get_minor(dev: &str) -> String {
    props::file_get(&format!("/sys/class/drm/{}/uevent", dev), "MINOR").unwrap_or_default()
}

pub fn get_kernel_driver(dev: &str) -> String {
    props::file_get(&format!("/sys/class/drm/{}/device/uevent", dev), "DRIVER").unwrap_or_default()
}

pub fn get_card_from_render(dev: &str) -> String {
    let pattern = format!("/sys/class/drm/{}/device/drm/card*", dev);
    let mut nodes: Vec<String> = glob::glob(&pattern)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    nodes.sort();
    match nodes.first().and_then(|p| Path::new(p).file_name()) {
        Some(name) => format!("/dev/dri/{}", name.to_string_lossy()),
        None => String::new(),
    }
}

/// Returns the render node and its card node. Both are empty when no usable
/// GPU is present, matching the fallback to software rendering.
pub fn get_dri_node(args: &MosaicArgs) -> anyhow::Result<(String, String)> {
    let cfg = crate::config::load(&args.config);
    if let Some(node) = cfg.mosaic.get("drm_device") {
        if !node.is_empty() {
            if !Path::new(node).exists() {
                anyhow::bail!("The specified drm_device {} does not exist", node);
            }
            let render_dev = Path::new(node)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if !UNSUPPORTED.contains(&get_kernel_driver(&render_dev).as_str()) {
                return Ok((node.clone(), get_card_from_render(&render_dev)));
            }
            return Ok((String::new(), String::new()));
        }
    }

    let mut nodes: Vec<String> = glob::glob("/dev/dri/renderD*")
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    nodes.sort();
    for node in nodes {
        let render_dev = Path::new(&node)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if !UNSUPPORTED.contains(&get_kernel_driver(&render_dev).as_str()) {
            return Ok((node, get_card_from_render(&render_dev)));
        }
    }
    Ok((String::new(), String::new()))
}

/// Maps the kernel driver behind `dev` to the Vulkan driver name the guest
/// expects.
pub fn get_vulkan_driver(args: &MosaicArgs, dev: &str) -> String {
    let mapping: [(&str, &str); 8] = [
        ("i915", "intel"),
        ("xe", "intel"),
        ("amdgpu", "radeon"),
        ("panfrost", "panfrost"),
        ("msm", "freedreno"),
        ("msm_dpu", "freedreno"),
        ("vc4", "broadcom"),
        ("nouveau", "nouveau"),
    ];
    let _ = args;
    let kernel_driver = get_kernel_driver(dev);

    if kernel_driver == "i915" {
        let card = get_card_from_render(dev);
        let card = Path::new(&card)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let minor = get_minor(&card);
        if let Ok(caps) =
            std::fs::read_to_string(format!("/sys/kernel/debug/dri/{}/i915_capabilities", minor))
        {
            for line in caps.lines() {
                let line = line.trim();
                if line.starts_with("graphics version:") || line.starts_with("gen:") {
                    if let Some(value) = line.split_whitespace().last() {
                        if let Ok(gen) = value.parse::<i32>() {
                            if gen < 9 {
                                return "intel_hasvk".to_string();
                            }
                        }
                    }
                }
            }
        }
    }

    for (k, v) in mapping {
        if k == kernel_driver {
            return v.to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_from_render_is_empty_for_unknown_device() {
        assert_eq!(get_card_from_render("renderD999"), "");
    }

    #[test]
    fn kernel_driver_is_empty_for_unknown_device() {
        assert_eq!(get_kernel_driver("renderD999"), "");
    }
}
