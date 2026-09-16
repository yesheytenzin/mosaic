// SPDX-License-Identifier: GPL-3.0-or-later

//! `mosaic runtime` — manage the host-native ART and Bionic bundle (ADR-0010).

use crate::args::{MosaicArgs, RuntimeSubaction};
use crate::config::Defaults;

pub async fn dispatch(args: &MosaicArgs, subaction: &RuntimeSubaction) -> anyhow::Result<()> {
    match subaction {
        RuntimeSubaction::Fetch => fetch(args).await,
        RuntimeSubaction::Status => status(args),
        RuntimeSubaction::Verify => verify(args),
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

/// Start ART out of the installed bundle and report what it says.
///
/// This is the smallest end-to-end check that the bundle is usable: it is the
/// same path the broker takes to start an app process (ADR-0012), so a bundle
/// that passes here has a working linker, linker configuration, Bionic, and
/// ART runtime. It is also what proves a freshly fetched bundle on a host.
pub fn verify(args: &MosaicArgs) -> anyhow::Result<()> {
    let dir = crate::runtime::require(args)?;
    let dalvikvm = crate::runtime::dalvikvm_path(&dir);
    if !std::path::Path::new(&dalvikvm).exists() {
        anyhow::bail!(
            "the bundle at {} has no bin/dalvikvm64. Rebuild it with \
             tools/bundle/bundle.sh build",
            dir
        );
    }

    println!("Verifying the runtime bundle at {}", dir);
    let dalvikvm = crate::runtime::dalvikvm_path(&dir);
    // dalvikvm dlopens libart.so by name and, with no property service to tell
    // it where, has to be pointed at it.
    let art = format!("{}/lib64/libart.so", dir);
    let output = crate::runtime::run_in_bundle(
        args,
        &dir,
        &dalvikvm,
        &[format!("-XXlib:{}", art), "-showversion".to_string()],
    )?;

    let version = output
        .lines()
        .find(|line| line.contains("ART version"))
        .map(|line| line.trim().to_string());
    match version {
        Some(version) => {
            println!("{}", version);
            Ok(())
        }
        None => anyhow::bail!(
            "ART did not report a version. Output was:\n{}",
            output.trim()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn args_for(work: &str) -> MosaicArgs {
        let cli =
            crate::args::Cli::try_parse_from(["mosaic", "-w", work, "runtime", "verify"]).unwrap();
        MosaicArgs::from_cli(cli)
    }

    #[test]
    fn verify_reports_how_to_get_a_bundle_when_there_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let err = verify(&args_for(dir.path().to_str().unwrap()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("mosaic runtime fetch"), "got: {}", err);
    }

    #[test]
    fn verify_rejects_a_bundle_without_art() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().to_str().unwrap();
        let version = crate::config::Defaults::new().bundle_version;
        let bundle = crate::runtime::version_dir(work, &version);
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(crate::runtime::runtime_dir(work) + "/version", &version).unwrap();

        let err = verify(&args_for(work)).unwrap_err().to_string();
        assert!(err.contains("dalvikvm64"), "got: {}", err);
    }
}
