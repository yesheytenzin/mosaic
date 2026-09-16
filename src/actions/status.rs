// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;

pub async fn print_status(args: &MosaicArgs) -> anyhow::Result<()> {
    let cfg = crate::config::load(&args.config);
    let print_stopped = || {
        println!("Session:\tSTOPPED");
        println!(
            "Vendor type:\t{}",
            cfg.mosaic.get("vendor_type").cloned().unwrap_or_default()
        );
    };

    match crate::helpers::ipc::container_get_session().await {
        Ok(session) if !session.is_empty() => {
            println!("Session:\tRUNNING");
            println!(
                "Container:\t{}",
                session.get("state").cloned().unwrap_or_default()
            );
            println!(
                "Vendor type:\t{}",
                cfg.mosaic.get("vendor_type").cloned().unwrap_or_default()
            );
            println!(
                "IP address:\t{}",
                crate::helpers::net::get_device_ip_address()
                    .unwrap_or_else(|| "UNKNOWN".to_string())
            );
            println!(
                "Session user:\t{}({})",
                session.get("user_name").cloned().unwrap_or_default(),
                session.get("user_id").cloned().unwrap_or_default()
            );
            println!(
                "Wayland display:\t{}",
                session.get("wayland_display").cloned().unwrap_or_default()
            );
        }
        _ => print_stopped(),
    }
    Ok(())
}

pub fn print_status_blocking(args: &MosaicArgs) -> anyhow::Result<()> {
    crate::helpers::runtime::block_on(print_status(args))
}
