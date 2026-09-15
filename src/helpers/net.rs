// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;

pub fn get_device_ip_address() -> Option<String> {
    use std::io::Read;
    if let Ok(mut file) = std::fs::File::open("/var/lib/misc/dnsmasq.mosaic0.leases") {
        let mut content = String::new();
        if file.read_to_string(&mut content).is_ok() {
            for token in content.split_whitespace() {
                if token.chars().filter(|c| *c == '.').count() == 3
                    && token.split('.').all(|part| part.parse::<u8>().is_ok())
                {
                    return Some(token.to_string());
                }
            }
        }
    }
    None
}

pub fn adb_connect(args: &MosaicArgs) -> anyhow::Result<()> {
    if which::which("adb").is_err() {
        anyhow::bail!("Could not find adb");
    }
    crate::helpers::run::user(
        args,
        &["adb".to_string(), "start-server".to_string()],
        "log",
        false,
        Some(true),
    )?;
    let ip = get_device_ip_address()
        .ok_or_else(|| anyhow::anyhow!("Unknown container IP address. Is Mosaic running?"))?;
    crate::helpers::run::user(
        args,
        &["adb".to_string(), "connect".to_string(), ip.clone()],
        "log",
        false,
        Some(true),
    )?;
    log::info!("Established ADB connection to Mosaic device at {}.", ip);
    Ok(())
}

pub fn adb_disconnect(args: &MosaicArgs) -> anyhow::Result<()> {
    if which::which("adb").is_err() {
        anyhow::bail!("Could not find adb");
    }
    let ip = get_device_ip_address()
        .ok_or_else(|| anyhow::anyhow!("Unknown container IP address. Was Mosaic ever running?"))?;
    crate::helpers::run::user(
        args,
        &["adb".to_string(), "disconnect".to_string(), ip.clone()],
        "log",
        false,
        Some(true),
    )?;
    Ok(())
}
