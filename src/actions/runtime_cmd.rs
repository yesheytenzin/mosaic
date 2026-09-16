// SPDX-License-Identifier: GPL-3.0-or-later

//! `mosaic runtime` — manage the host-native ART and Bionic bundle (ADR-0010).

use crate::args::{MosaicArgs, RuntimeSubaction};
use crate::config::Defaults;

pub async fn dispatch(args: &MosaicArgs, subaction: &RuntimeSubaction) -> anyhow::Result<()> {
    match subaction {
        RuntimeSubaction::Fetch => fetch(args).await,
        RuntimeSubaction::Status => status(args),
    }
}

pub async fn fetch(args: &MosaicArgs) -> anyhow::Result<()> {
    let defaults = Defaults::new();
    let config = crate::config::load(&args.config);
    let version = config.bundle_version();
    let dir = crate::runtime::fetch(args, &defaults.bundle_channel, &version).await?;
    println!("Runtime bundle {} installed at {}", version, dir);
    Ok(())
}

pub fn status(args: &MosaicArgs) -> anyhow::Result<()> {
    match crate::runtime::installed_version(&args.work) {
        Some(version) => {
            let dir = crate::runtime::version_dir(&args.work, &version);
            println!(
                "Runtime bundle {} ({}) at {}",
                version,
                crate::runtime::host_arch(),
                dir
            );
        }
        None => {
            println!("No runtime bundle installed. Run 'mosaic runtime fetch' to download one.")
        }
    }
    Ok(())
}
