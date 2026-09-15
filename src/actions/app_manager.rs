// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use crate::config::SessionDefaults;

async fn get_session() -> Option<std::collections::HashMap<String, String>> {
    crate::helpers::ipc::container_get_session().await.ok()
}

async fn container_freeze() -> anyhow::Result<()> {
    crate::helpers::ipc::container_freeze().await
}

async fn container_unfreeze() -> anyhow::Result<()> {
    crate::helpers::ipc::container_unfreeze().await
}

pub async fn install(args: &MosaicArgs, package: &str) -> anyhow::Result<()> {
    let session = get_session()
        .await
        .ok_or_else(|| anyhow::anyhow!("Mosaic session is stopped"))?;
    let state = session.get("state").cloned().unwrap_or_default();
    if state == "FROZEN" {
        let _ = container_unfreeze().await;
    }

    let mosaic_data = SessionDefaults::new().mosaic_data;
    let mosaic_data = session.get("mosaic_data").cloned().unwrap_or(mosaic_data);
    let tmp_dir = format!("{}/mosaic_tmp", mosaic_data);
    std::fs::create_dir_all(&tmp_dir)?;

    let dest = format!("{}/base.apk", tmp_dir);
    std::fs::copy(package, &dest)?;

    if let Some(platform) = crate::interfaces::i_platform::get_service(args) {
        platform.install_app("/data/mosaic_tmp/base.apk");
    } else {
        log::error!("Failed to access IPlatform service");
    }

    let _ = std::fs::remove_file(&dest);

    if state == "FROZEN" {
        let _ = container_freeze().await;
    }
    Ok(())
}

pub async fn remove(args: &MosaicArgs, package: &str) -> anyhow::Result<()> {
    let session = get_session()
        .await
        .ok_or_else(|| anyhow::anyhow!("Mosaic session is stopped"))?;
    let state = session.get("state").cloned().unwrap_or_default();
    if state == "FROZEN" {
        let _ = container_unfreeze().await;
    }

    if let Some(platform) = crate::interfaces::i_platform::get_service(args) {
        if let Some(ret) = platform.remove_app(package) {
            if ret != 0 {
                log::error!("Failed to uninstall package: {}", package);
            }
        }
    } else {
        log::error!("Failed to access IPlatform service");
    }

    if state == "FROZEN" {
        let _ = container_freeze().await;
    }
    Ok(())
}

async fn maybe_launch_later<F>(args: &MosaicArgs, launch_now: F) -> anyhow::Result<()>
where
    F: FnOnce() -> anyhow::Result<()> + Send,
{
    let session = get_session().await;
    if session.is_some() {
        let _ = container_unfreeze().await;
        launch_now()
    } else {
        log::error!("Starting mosaic session");
        crate::actions::session_manager::start(args.clone()).await?;
        launch_now()
    }
}

pub async fn launch(args: &MosaicArgs, package: &str) -> anyhow::Result<()> {
    let package = package.to_string();
    let args_clone = args.clone();
    maybe_launch_later(args, move || {
        if let Some(platform) = crate::interfaces::i_platform::get_service(&args_clone) {
            platform.setprop("mosaic.active_apps", &package);
            platform.launch_app(&package);
            let multiwin = platform
                .getprop("persist.mosaic.multi_windows", "false")
                .unwrap_or_else(|| "false".to_string());
            if multiwin == "false" {
                platform.settings_put_string(2, "policy_control", "immersive.status=*");
            } else {
                platform.settings_put_string(2, "policy_control", "immersive.full=*");
            }
        } else {
            log::error!("Failed to access IPlatform service");
        }
        Ok(())
    })
    .await
}

pub async fn list(args: &MosaicArgs) -> anyhow::Result<()> {
    let _ = get_session()
        .await
        .ok_or_else(|| anyhow::anyhow!("Mosaic session is stopped"))?;
    let session = get_session().await.unwrap_or_default();
    let state = session.get("state").cloned().unwrap_or_default();
    if state == "FROZEN" {
        let _ = container_unfreeze().await;
    }

    if let Some(platform) = crate::interfaces::i_platform::get_service(args) {
        let apps = platform.get_apps_info();
        for app in apps {
            println!("Name: {}", app.name);
            println!("packageName: {}", app.package_name);
            println!("categories:");
            for cat in app.categories {
                println!("\t{}", cat);
            }
        }
    } else {
        log::error!("Failed to access IPlatform service");
    }

    if state == "FROZEN" {
        let _ = container_freeze().await;
    }
    Ok(())
}

pub async fn show_full_ui(args: &MosaicArgs) -> anyhow::Result<()> {
    let args_clone = args.clone();
    maybe_launch_later(args, move || {
        if let Some(platform) = crate::interfaces::i_platform::get_service(&args_clone) {
            platform.setprop("mosaic.active_apps", "Mosaic");
            platform.settings_put_string(2, "policy_control", "null*");
            if let Some(status_bar) = crate::interfaces::i_status_bar::get_service(&args_clone) {
                status_bar.expand();
                std::thread::sleep(std::time::Duration::from_millis(500));
                status_bar.collapse();
            }
        } else {
            log::error!("Failed to access IPlatform service");
        }
        Ok(())
    })
    .await
}

pub async fn intent(args: &MosaicArgs, action: &str, uri: &str) -> anyhow::Result<()> {
    let action = action.to_string();
    let uri = uri.to_string();
    let args_clone = args.clone();
    maybe_launch_later(args, move || {
        if let Some(platform) = crate::interfaces::i_platform::get_service(&args_clone) {
            if let Some(ret) = platform.launch_intent(&action, &uri) {
                if ret.is_empty() {
                    return Ok(());
                }
                let pkg = if ret == "android" {
                    "Mosaic".to_string()
                } else {
                    ret
                };
                platform.setprop("mosaic.active_apps", &pkg);
                let multiwin = platform
                    .getprop("persist.mosaic.multi_windows", "false")
                    .unwrap_or_else(|| "false".to_string());
                if multiwin == "false" {
                    platform.settings_put_string(2, "policy_control", "immersive.status=*");
                } else {
                    platform.settings_put_string(2, "policy_control", "immersive.full=*");
                }
            }
        } else {
            log::error!("Failed to access IPlatform service");
        }
        Ok(())
    })
    .await
}
