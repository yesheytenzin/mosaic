// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use crate::config::Defaults;
use std::path::Path;

pub fn get_lxc_version(args: &MosaicArgs) -> u32 {
    if which::which("lxc-info").is_ok() {
        if let Ok(out) = crate::helpers::run::user(
            args,
            &["lxc-info".to_string(), "--version".to_string()],
            "log",
            true,
            Some(false),
        ) {
            if let Some(major) = out.trim().split('.').next() {
                if let Ok(v) = major.parse::<u32>() {
                    return v;
                }
            }
            // fallback: try first char
            if let Some(c) = out.trim().chars().next() {
                if let Some(d) = c.to_digit(10) {
                    return d;
                }
            }
        }
    }
    0
}

fn add_node_entry(
    nodes: &mut Vec<String>,
    src: &str,
    dist: Option<&str>,
    mnt_type: &str,
    options: &str,
    check: bool,
) -> bool {
    if check && !Path::new(src).exists() {
        return false;
    }
    let dist = dist.unwrap_or_else(|| src.trim_start_matches('/'));
    let entry = format!(
        "lxc.mount.entry = {} {} {} {}",
        src, dist, mnt_type, options
    );
    nodes.push(entry);
    true
}

pub fn generate_nodes_lxc_config(args: &MosaicArgs) -> anyhow::Result<Vec<String>> {
    let mut nodes = Vec::new();

    add_node_entry(
        &mut nodes,
        "tmpfs",
        Some("dev"),
        "tmpfs",
        "nosuid 0 0",
        false,
    );
    add_node_entry(
        &mut nodes,
        "/dev/zero",
        None,
        "none",
        "bind,create=file,optional 0 0",
        true,
    );
    add_node_entry(
        &mut nodes,
        "/dev/null",
        None,
        "none",
        "bind,create=file,optional 0 0",
        true,
    );
    add_node_entry(
        &mut nodes,
        "/dev/full",
        None,
        "none",
        "bind,create=file,optional 0 0",
        true,
    );
    add_node_entry(
        &mut nodes,
        "/dev/ashmem",
        None,
        "none",
        "bind,create=file,optional 0 0",
        true,
    );
    add_node_entry(
        &mut nodes,
        "/dev/fuse",
        None,
        "none",
        "bind,create=file,optional 0 0",
        true,
    );
    add_node_entry(
        &mut nodes,
        "/dev/ion",
        None,
        "none",
        "bind,create=file,optional 0 0",
        true,
    );
    add_node_entry(
        &mut nodes,
        "/dev/tty",
        None,
        "none",
        "bind,create=file,optional 0 0",
        true,
    );
    add_node_entry(
        &mut nodes,
        "/dev/char",
        None,
        "none",
        "bind,create=dir,optional 0 0",
        true,
    );

    add_node_entry(
        &mut nodes,
        "/dev/kgsl-3d0",
        None,
        "none",
        "bind,create=file,optional 0 0",
        true,
    );
    add_node_entry(
        &mut nodes,
        "/dev/mali0",
        None,
        "none",
        "bind,create=file,optional 0 0",
        true,
    );
    add_node_entry(
        &mut nodes,
        "/dev/pvr_sync",
        None,
        "none",
        "bind,create=file,optional 0 0",
        true,
    );
    add_node_entry(
        &mut nodes,
        "/dev/pmsg0",
        None,
        "none",
        "bind,create=file,optional 0 0",
        true,
    );
    add_node_entry(
        &mut nodes,
        "/dev/dxg",
        None,
        "none",
        "bind,create=file,optional 0 0",
        true,
    );
    let (render, _) = crate::helpers::gpu::get_dri_node(args)?;
    if !render.is_empty() {
        add_node_entry(
            &mut nodes,
            &render,
            None,
            "none",
            "bind,create=file,optional 0 0",
            true,
        );
    }

    for n in glob::glob("/dev/fb*")
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
    {
        add_node_entry(
            &mut nodes,
            &n.to_string_lossy(),
            None,
            "none",
            "bind,create=file,optional 0 0",
            true,
        );
    }
    for n in glob::glob("/dev/graphics/fb*")
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
    {
        add_node_entry(
            &mut nodes,
            &n.to_string_lossy(),
            None,
            "none",
            "bind,create=file,optional 0 0",
            true,
        );
    }
    for n in glob::glob("/dev/video*")
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
    {
        add_node_entry(
            &mut nodes,
            &n.to_string_lossy(),
            None,
            "none",
            "bind,create=file,optional 0 0",
            true,
        );
    }
    for n in glob::glob("/dev/dma_heap/*")
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
    {
        add_node_entry(
            &mut nodes,
            &n.to_string_lossy(),
            None,
            "none",
            "bind,create=file,optional 0 0",
            true,
        );
    }

    let binder = args
        .cache
        .get("BINDER_DRIVER")
        .map(|s| s.as_str())
        .unwrap_or("binder");
    let vnd = args
        .cache
        .get("VNDBINDER_DRIVER")
        .map(|s| s.as_str())
        .unwrap_or("vndbinder");
    let hw = args
        .cache
        .get("HWBINDER_DRIVER")
        .map(|s| s.as_str())
        .unwrap_or("hwbinder");

    add_node_entry(
        &mut nodes,
        &format!("/dev/{}", binder),
        Some("dev/binder"),
        "none",
        "bind,create=file,optional 0 0",
        false,
    );
    add_node_entry(
        &mut nodes,
        &format!("/dev/{}", vnd),
        Some("dev/vndbinder"),
        "none",
        "bind,create=file,optional 0 0",
        false,
    );
    add_node_entry(
        &mut nodes,
        &format!("/dev/{}", hw),
        Some("dev/hwbinder"),
        "none",
        "bind,create=file,optional 0 0",
        false,
    );

    let vendor_type = crate::config::load(&args.config)
        .mosaic
        .get("vendor_type")
        .cloned()
        .unwrap_or_else(|| "MAINLINE".to_string());
    if vendor_type != "MAINLINE" {
        if !add_node_entry(
            &mut nodes,
            "/dev/hwbinder",
            Some("dev/host_hwbinder"),
            "none",
            "bind,create=file,optional 0 0",
            true,
        ) {
            log::warn!("Binder node \"hwbinder\" of host not found");
        }
        add_node_entry(
            &mut nodes,
            "/vendor",
            Some("vendor_extra"),
            "none",
            "rbind,optional 0 0",
            true,
        );
    }

    add_node_entry(
        &mut nodes,
        "none",
        Some("dev/pts"),
        "devpts",
        "defaults,mode=644,ptmxmode=666,create=dir 0 0",
        false,
    );
    add_node_entry(
        &mut nodes,
        "/dev/uhid",
        None,
        "none",
        "bind,create=file,optional 0 0",
        true,
    );
    add_node_entry(
        &mut nodes,
        "/dev/net/tun",
        Some("dev/tun"),
        "none",
        "bind,create=file,optional 0 0",
        true,
    );
    add_node_entry(
        &mut nodes,
        "/sys/module/lowmemorykiller",
        None,
        "none",
        "bind,create=dir,optional 0 0",
        true,
    );

    let host_perms = format!("{}/host-permissions", args.work);
    add_node_entry(
        &mut nodes,
        &host_perms,
        Some("vendor/etc/host-permissions"),
        "none",
        "bind,optional 0 0",
        true,
    );

    add_node_entry(
        &mut nodes,
        "/dev/sw_sync",
        None,
        "none",
        "bind,create=file,optional 0 0",
        true,
    );
    add_node_entry(
        &mut nodes,
        "/sys/kernel/debug",
        None,
        "none",
        "rbind,create=dir,optional 0 0",
        true,
    );

    add_node_entry(
        &mut nodes,
        "/sys/class/leds/vibrator",
        None,
        "none",
        "bind,create=dir,optional 0 0",
        true,
    );
    add_node_entry(
        &mut nodes,
        "/sys/devices/virtual/timed_output/vibrator",
        None,
        "none",
        "bind,create=dir,optional 0 0",
        true,
    );

    add_node_entry(
        &mut nodes,
        "/dev/Vcodec",
        None,
        "none",
        "bind,create=file,optional 0 0",
        true,
    );
    add_node_entry(
        &mut nodes,
        "/dev/MTK_SMI",
        None,
        "none",
        "bind,create=file,optional 0 0",
        true,
    );
    add_node_entry(
        &mut nodes,
        "/dev/mdp_sync",
        None,
        "none",
        "bind,create=file,optional 0 0",
        true,
    );
    add_node_entry(
        &mut nodes,
        "/dev/mtk_cmdq",
        None,
        "none",
        "bind,create=file,optional 0 0",
        true,
    );

    add_node_entry(
        &mut nodes,
        "tmpfs",
        Some("mnt_extra"),
        "tmpfs",
        "nodev 0 0",
        false,
    );
    add_node_entry(
        &mut nodes,
        "/mnt/wslg",
        Some("mnt_extra/wslg"),
        "none",
        "rbind,create=dir,optional 0 0",
        true,
    );

    add_node_entry(
        &mut nodes,
        "tmpfs",
        Some("tmp"),
        "tmpfs",
        "nodev 0 0",
        false,
    );
    add_node_entry(
        &mut nodes,
        "tmpfs",
        Some("var"),
        "tmpfs",
        "nodev 0 0",
        false,
    );
    add_node_entry(
        &mut nodes,
        "tmpfs",
        Some("run"),
        "tmpfs",
        "nodev 0 0",
        false,
    );

    add_node_entry(
        &mut nodes,
        "/system/etc/libnfc-nci.conf",
        None,
        "none",
        "bind,optional 0 0",
        true,
    );

    Ok(nodes)
}

pub const LXC_APPARMOR_PROFILE: &str = "lxc-mosaic";

pub fn get_apparmor_status(args: &MosaicArgs) -> bool {
    let mut enabled = false;
    if which::which("aa-enabled").is_ok() {
        enabled = crate::helpers::run::user(
            args,
            &["aa-enabled".to_string(), "--quiet".to_string()],
            "log",
            false,
            Some(false),
        )
        .is_ok();
    }
    if !enabled && which::which("systemctl").is_ok() {
        enabled = crate::helpers::run::user(
            args,
            &[
                "systemctl".to_string(),
                "is-active".to_string(),
                "-q".to_string(),
                "apparmor".to_string(),
            ],
            "log",
            false,
            Some(false),
        )
        .is_ok();
    }
    if let Ok(content) = std::fs::read_to_string("/sys/kernel/security/apparmor/profiles") {
        enabled &= content.contains(LXC_APPARMOR_PROFILE);
    } else {
        enabled = false;
    }
    enabled
}

pub fn set_lxc_config(args: &MosaicArgs) -> anyhow::Result<()> {
    let lxc_path = format!("{}/lxc/mosaic", args.work);
    let lxc_ver = get_lxc_version(args);
    if lxc_ver == 0 {
        anyhow::bail!("LXC is not installed");
    }
    let tools_src = Defaults::tools_src();
    let config_paths = format!("{}/data/configs/config_", tools_src);
    let seccomp_profile = format!("{}/data/configs/mosaic.seccomp", tools_src);

    let mut snippets = vec![format!("{}base", config_paths)];
    if lxc_ver <= 2 {
        snippets.push(format!("{}1", config_paths));
    } else {
        for ver in 3..5 {
            let snippet = format!("{}{}", config_paths, ver);
            if lxc_ver >= ver && Path::new(&snippet).exists() {
                snippets.push(snippet);
            }
        }
    }

    crate::helpers::run::user(
        args,
        &["mkdir".to_string(), "-p".to_string(), lxc_path.clone()],
        "log",
        false,
        Some(true),
    )?;

    let cat_cmd = format!(
        "cat {} > \"{}/config\"",
        snippets
            .iter()
            .map(|s| format!("\"{}\"", s))
            .collect::<Vec<_>>()
            .join(" "),
        lxc_path
    );
    crate::helpers::run::user(
        args,
        &["sh".to_string(), "-c".to_string(), cat_cmd],
        "log",
        false,
        Some(true),
    )?;

    let arch = crate::helpers::arch::host()?;
    crate::helpers::run::user(
        args,
        &[
            "sed".to_string(),
            "-i".to_string(),
            format!("s/LXCARCH/{}/", arch),
            format!("{}/config", lxc_path),
        ],
        "log",
        false,
        Some(true),
    )?;

    let post_stop = format!("{}/data/scripts/mosaic-post-stop.sh", tools_src);
    crate::helpers::run::user(
        args,
        &[
            "sed".to_string(),
            "-i".to_string(),
            format!("s#LXCPOSTSTOP#{}#", post_stop),
            format!("{}/config", lxc_path),
        ],
        "log",
        false,
        Some(true),
    )?;

    crate::helpers::run::user(
        args,
        &[
            "cp".to_string(),
            "-fpr".to_string(),
            seccomp_profile,
            format!("{}/mosaic.seccomp", lxc_path),
        ],
        "log",
        false,
        Some(true),
    )?;

    if get_apparmor_status(args) {
        crate::helpers::run::user(
            args,
            &[
                "sed".to_string(),
                "-i".to_string(),
                "-E".to_string(),
                format!(
                    "/lxc.aa_profile|lxc.apparmor.profile/ s/unconfined/{}/g",
                    LXC_APPARMOR_PROFILE
                ),
                format!("{}/config", lxc_path),
            ],
            "log",
            false,
            Some(true),
        )?;
    }

    let nodes = generate_nodes_lxc_config(args)?;
    let tmp_path = format!("{}/config_nodes", args.work);
    std::fs::write(&tmp_path, nodes.join("\n") + "\n")?;
    crate::helpers::run::user(
        args,
        &["mv".to_string(), tmp_path, lxc_path.clone()],
        "log",
        false,
        Some(true),
    )?;

    std::fs::File::create(format!("{}/config_session", lxc_path))?;
    Ok(())
}

pub fn generate_session_lxc_config(
    args: &MosaicArgs,
    session: &crate::config::SessionDefaults,
) -> anyhow::Result<()> {
    let mut nodes = Vec::new();
    let mut make_entry = |src: &str, dist: Option<&str>, mnt_type: &str, options: &str| -> bool {
        if src.contains('\n') || src.contains('\r') {
            log::warn!(
                "User-provided mount path contains illegal character: {}",
                src
            );
            return false;
        }
        // check ownership if dist is None
        if dist.is_none() {
            if !Path::new(src).exists() {
                log::warn!("User-provided mount path is not owned by user: {}", src);
                return false;
            }
            if let Ok(meta) = std::fs::metadata(src) {
                use std::os::unix::fs::MetadataExt;
                if meta.uid().to_string() != session.user_id {
                    log::warn!("User-provided mount path is not owned by user: {}", src);
                    return false;
                }
            }
        }
        add_node_entry(&mut nodes, src, dist, mnt_type, options, false)
    };

    let defaults = Defaults::new();
    if !make_entry(
        "tmpfs",
        Some(&defaults.container_xdg_runtime_dir),
        "tmpfs",
        "create=dir 0 0",
    ) {
        anyhow::bail!("Failed to create XDG_RUNTIME_DIR mount point");
    }

    let wayland_host =
        std::fs::canonicalize(Path::new(&session.xdg_runtime_dir).join(&session.wayland_display))
            .unwrap_or_else(|_| Path::new(&session.xdg_runtime_dir).join(&session.wayland_display))
            .to_string_lossy()
            .to_string();
    let wayland_container = format!(
        "{}/{}",
        defaults.container_xdg_runtime_dir, defaults.container_wayland_display
    );
    if !make_entry(
        &wayland_host,
        Some(wayland_container.trim_start_matches('/')),
        "none",
        "rbind,create=file 0 0",
    ) {
        anyhow::bail!("Failed to bind Wayland socket");
    }

    let pulse_host = format!("{}/native", session.pulse_runtime_path);
    let pulse_container = format!("{}/native", defaults.container_pulse_runtime_path);
    make_entry(
        &pulse_host,
        Some(pulse_container.trim_start_matches('/')),
        "none",
        "rbind,create=file 0 0",
    );

    if !make_entry(&session.mosaic_data, Some("data"), "none", "rbind 0 0") {
        anyhow::bail!("Failed to bind userdata");
    }

    let lxc_path = format!("{}/lxc/mosaic", args.work);
    let tmp_path = format!("{}/config_session", args.work);
    std::fs::write(&tmp_path, nodes.join("\n") + "\n")?;
    crate::helpers::run::user(
        args,
        &["mv".to_string(), tmp_path, lxc_path],
        "log",
        false,
        Some(true),
    )?;
    Ok(())
}

pub fn setup_host_perms(args: &MosaicArgs) -> anyhow::Result<()> {
    let host_perms = format!("{}/host-permissions", args.work);
    if !Path::new(&host_perms).exists() {
        std::fs::create_dir_all(&host_perms)?;
    }

    let treble = crate::helpers::props::host_get("ro.treble.enabled");
    if treble != "true" {
        return Ok(());
    }

    let sku = crate::helpers::props::host_get("ro.boot.product.hardware.sku");
    let mut copy_list = Vec::new();

    for entry in glob::glob("/vendor/etc/permissions/android.hardware.nfc.*")
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
    {
        copy_list.push(entry);
    }
    if Path::new("/vendor/etc/permissions/android.hardware.consumerir.xml").exists() {
        copy_list.push(
            Path::new("/vendor/etc/permissions/android.hardware.consumerir.xml").to_path_buf(),
        );
    }
    for entry in glob::glob("/odm/etc/permissions/android.hardware.nfc.*")
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
    {
        copy_list.push(entry);
    }
    if Path::new("/odm/etc/permissions/android.hardware.consumerir.xml").exists() {
        copy_list
            .push(Path::new("/odm/etc/permissions/android.hardware.consumerir.xml").to_path_buf());
    }
    if !sku.is_empty() {
        for entry in glob::glob(&format!(
            "/odm/etc/permissions/sku_{}/android.hardware.nfc.*",
            sku
        ))
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        {
            copy_list.push(entry);
        }
        let sku_path = format!(
            "/odm/etc/permissions/sku_{}/android.hardware.consumerir.xml",
            sku
        );
        if Path::new(&sku_path).exists() {
            copy_list.push(Path::new(&sku_path).to_path_buf());
        }
    }

    for src in copy_list {
        let dest = Path::new(&host_perms).join(src.file_name().unwrap());
        let _ = std::fs::copy(&src, &dest);
    }
    Ok(())
}

pub fn make_base_props(args: &MosaicArgs) -> anyhow::Result<()> {
    use crate::interfaces::gbinder::ServiceManager;
    use std::collections::HashMap;

    let cfg = crate::config::load(&args.config);
    let vendor_type = cfg
        .mosaic
        .get("vendor_type")
        .cloned()
        .unwrap_or_else(|| "MAINLINE".to_string());
    let binder_driver = cfg
        .mosaic
        .get("binder")
        .cloned()
        .unwrap_or_else(|| "binder".to_string());
    let sm_protocol = cfg.mosaic.get("service_manager_protocol").cloned();
    let binder_protocol = cfg.mosaic.get("binder_protocol").cloned();

    // Find the HAL blobs the host ships for a hardware type.
    let find_hal = |hardware: &str| -> String {
        let hardware_props = [
            format!("ro.hardware.{}", hardware),
            "ro.hardware".to_string(),
            "ro.product.board".to_string(),
            "ro.arch".to_string(),
            "ro.board.platform".to_string(),
        ];
        for p in hardware_props {
            let prop = crate::helpers::props::host_get(&p);
            if prop.is_empty() {
                continue;
            }
            for lib in [
                "/odm/lib",
                "/odm/lib64",
                "/vendor/lib",
                "/vendor/lib64",
                "/system/lib",
                "/system/lib64",
            ] {
                if Path::new(&format!("{}/hw/{}.{}.so", lib, hardware, prop)).is_file() {
                    return prop;
                }
            }
        }
        String::new()
    };

    let find_hidl = |intf: &str| -> bool {
        if vendor_type == "MAINLINE" {
            return false;
        }
        let device = format!("/dev/{}", binder_driver);
        match ServiceManager::new(&device, sm_protocol.as_deref(), binder_protocol.as_deref()) {
            Ok(sm) => sm.list_sync().iter().any(|s| s == intf),
            Err(_) => false,
        }
    };

    let find_aidl = |intf: &str| -> bool {
        if vendor_type == "MAINLINE" {
            return false;
        }
        match ServiceManager::new("/dev/binder", None, None) {
            Ok(sm) => sm.list_sync().iter().any(|s| s == intf),
            Err(_) => false,
        }
    };

    let mut props: Vec<String> = Vec::new();

    if !Path::new("/dev/ashmem").exists() {
        props.push("sys.use_memfd=true".to_string());
    }

    // Added for security reasons
    props.push("ro.adb.secure=1".to_string());
    props.push("ro.debuggable=0".to_string());

    let mut egl = crate::helpers::props::host_get("ro.hardware.egl");
    let (dri, _) = crate::helpers::gpu::get_dri_node(args)?;

    let mut gralloc = find_hal("gralloc");
    if gralloc.is_empty()
        && (find_hidl("android.hardware.graphics.allocator@4.0::IAllocator/default")
            || find_aidl("android.hardware.graphics.allocator.IAllocator/default"))
    {
        gralloc = "android".to_string();
    }
    if gralloc.is_empty() {
        if !dri.is_empty() {
            gralloc = "gbm".to_string();
            egl = "mesa".to_string();
            props.push(format!("gralloc.gbm.device={}", dri));
        } else {
            gralloc = "default".to_string();
            egl = "swiftshader".to_string();
        }
        props.push("debug.stagefright.ccodec=0".to_string());
    }
    props.push(format!("ro.hardware.gralloc={}", gralloc));

    if !egl.is_empty() {
        props.push(format!("ro.hardware.egl={}", egl));
    }

    let mut media_profiles = crate::helpers::props::host_get("media.settings.xml");
    if !media_profiles.is_empty() {
        media_profiles = media_profiles.replace("vendor/", "vendor_extra/");
        media_profiles = media_profiles.replace("odm/", "odm_extra/");
        props.push(format!("media.settings.xml={}", media_profiles));
    }

    let ccodec = crate::helpers::props::host_get("debug.stagefright.ccodec");
    if !ccodec.is_empty() {
        props.push(format!("debug.stagefright.ccodec={}", ccodec));
    }

    let mut ext_library = crate::helpers::props::host_get("ro.vendor.extension_library");
    if !ext_library.is_empty() {
        ext_library = ext_library.replace("vendor/", "vendor_extra/");
        ext_library = ext_library.replace("odm/", "odm_extra/");
        props.push(format!("ro.vendor.extension_library={}", ext_library));
    }

    let mut vulkan = find_hal("vulkan");
    if vulkan.is_empty() && !dri.is_empty() {
        let base = Path::new(&dri)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        vulkan = crate::helpers::gpu::get_vulkan_driver(args, &base);
    }
    if !vulkan.is_empty() {
        props.push(format!("ro.hardware.vulkan={}", vulkan));
    }

    let treble = crate::helpers::props::host_get("ro.treble.enabled");
    if treble != "true" {
        let camera = find_hal("camera");
        if !camera.is_empty() {
            props.push(format!("ro.hardware.camera={}", camera));
        } else if vendor_type == "MAINLINE" {
            props.push("ro.hardware.camera=v4l2".to_string());
        }
    }

    let mut opengles = crate::helpers::props::host_get("ro.opengles.version");
    if opengles.is_empty() {
        opengles = "196610".to_string();
    }
    props.push(format!("ro.opengles.version={}", opengles));

    // Some Mali devices require ro.vendor.arm.egl.* props from the host.
    let arm_egl: HashMap<String, String> = crate::helpers::props::host_list("ro.vendor.arm.egl.");
    for (k, v) in arm_egl {
        props.push(format!("{}={}", k, v));
    }

    let images_path = cfg
        .mosaic
        .get("images_path")
        .cloned()
        .unwrap_or_else(|| format!("{}/images", args.work));
    let preinstalled = crate::config::Defaults::new().preinstalled_images_paths;
    if !preinstalled.contains(&images_path) {
        if let Some(v) = cfg.mosaic.get("system_ota") {
            props.push(format!("{}={}", crate::guest::PROP_SYSTEM_OTA, v));
        }
        if let Some(v) = cfg.mosaic.get("vendor_ota") {
            props.push(format!("{}={}", crate::guest::PROP_VENDOR_OTA, v));
        }
    } else {
        props.push(format!("{}=true", crate::guest::PROP_UPDATER_DISABLED));
    }
    props.push(format!(
        "{}={}",
        crate::guest::PROP_TOOLS_VERSION,
        crate::config::VERSION
    ));

    if vendor_type == "MAINLINE" {
        props.push("ro.vndk.lite=true".to_string());
    }

    for product in ["brand", "device", "manufacturer", "model", "name"] {
        let prop_product =
            crate::helpers::props::host_get(&format!("ro.product.vendor.{}", product));
        if !prop_product.is_empty() {
            props.push(format!(
                "{}{}={}",
                crate::guest::PRODUCT_PREFIX,
                product,
                prop_product
            ));
        } else {
            let dt = format!("/proc/device-tree/{}", product);
            if Path::new(&dt).is_file() {
                if let Ok(raw) = std::fs::read(&dt) {
                    let value = String::from_utf8_lossy(&raw)
                        .trim_matches(char::from(0))
                        .trim()
                        .to_string();
                    if !value.is_empty() {
                        props.push(format!(
                            "{}{}={}",
                            crate::guest::PRODUCT_PREFIX,
                            product,
                            value
                        ));
                    }
                }
            }
        }
    }

    let prop_fp = crate::helpers::props::host_get("ro.vendor.build.fingerprint");
    if !prop_fp.is_empty() {
        props.push(format!("ro.build.fingerprint={}", prop_fp));
    }

    // Append or override with the [properties] section of the config.
    for (k, v) in &cfg.properties {
        props.retain(|p| !p.starts_with(&format!("{}=", k)));
        props.push(format!("{}={}", k, v));
    }

    std::fs::write(
        format!("{}/{}", args.work, crate::guest::BASE_PROP_FILE),
        props.join("\n") + "\n",
    )?;
    Ok(())
}

pub fn status(args: &MosaicArgs) -> String {
    let cmd = vec![
        "lxc-info".to_string(),
        "-P".to_string(),
        format!("{}/lxc", args.work),
        "-n".to_string(),
        "mosaic".to_string(),
        "-sH".to_string(),
    ];
    crate::helpers::run::user(args, &cmd, "log", true, Some(false))
        .unwrap_or_else(|_| "STOPPED".to_string())
        .trim()
        .to_string()
}

pub fn wait_for_running(args: &MosaicArgs) -> anyhow::Result<()> {
    let mut timeout = 10;
    let mut state = status(args);
    while state != "RUNNING" && timeout > 0 {
        state = status(args);
        log::info!("waiting {} seconds for container to start...", timeout);
        timeout -= 1;
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    if state != "RUNNING" {
        anyhow::bail!("container failed to start");
    }
    Ok(())
}

pub fn start(args: &MosaicArgs) -> anyhow::Result<()> {
    let cmd = vec![
        "lxc-start".to_string(),
        "-P".to_string(),
        format!("{}/lxc", args.work),
        "-F".to_string(),
        "-n".to_string(),
        "mosaic".to_string(),
        "--".to_string(),
        "/init".to_string(),
    ];
    crate::helpers::run::user(args, &cmd, "background", false, Some(true))?;
    wait_for_running(args)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&args.log, std::fs::Permissions::from_mode(0o666));
    }
    Ok(())
}

pub fn stop(args: &MosaicArgs) -> anyhow::Result<()> {
    crate::helpers::run::user(
        args,
        &[
            "lxc-stop".to_string(),
            "-P".to_string(),
            format!("{}/lxc", args.work),
            "-n".to_string(),
            "mosaic".to_string(),
            "-k".to_string(),
        ],
        "log",
        false,
        Some(true),
    )?;
    Ok(())
}

pub fn freeze(args: &MosaicArgs) -> anyhow::Result<()> {
    crate::helpers::run::user(
        args,
        &[
            "lxc-freeze".to_string(),
            "-P".to_string(),
            format!("{}/lxc", args.work),
            "-n".to_string(),
            "mosaic".to_string(),
        ],
        "log",
        false,
        Some(true),
    )?;
    Ok(())
}

pub fn unfreeze(args: &MosaicArgs) -> anyhow::Result<()> {
    crate::helpers::run::user(
        args,
        &[
            "lxc-unfreeze".to_string(),
            "-P".to_string(),
            format!("{}/lxc", args.work),
            "-n".to_string(),
            "mosaic".to_string(),
        ],
        "log",
        false,
        Some(true),
    )?;
    Ok(())
}
