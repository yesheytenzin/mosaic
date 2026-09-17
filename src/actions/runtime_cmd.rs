// SPDX-License-Identifier: GPL-3.0-or-later

//! `mosaic runtime` — manage the host-native ART and Bionic bundle (ADR-0010).

use crate::args::{MosaicArgs, RuntimeSubaction};

/// Where a Mosaic build keeps the system image it was built from.
const DEFAULT_IMAGE: &str = "/var/lib/mosaic/images/system.img";
use crate::config::Defaults;

pub async fn dispatch(args: &MosaicArgs, subaction: &RuntimeSubaction) -> anyhow::Result<()> {
    match subaction {
        RuntimeSubaction::Fetch => fetch(args).await,
        RuntimeSubaction::Status => status(args),
        RuntimeSubaction::Install { path, version } => install(args, path.as_deref(), version),
        RuntimeSubaction::Verify => verify(args),
    }
}

pub async fn fetch(args: &MosaicArgs) -> anyhow::Result<()> {
    let defaults = Defaults::new();
    let config = crate::config::load(&args.config);
    let version = config.bundle_version();
    let dir = crate::runtime::fetch(args, &defaults.bundle_channel, &version)
        .await
        .map_err(|e| {
            // The published bundle is per release and may not exist for the version
            // this build asks for. Saying only "404" leaves a person with a bundle
            // they built themselves and no idea it can be used.
            anyhow::anyhow!(
                "{}. If you built a bundle yourself, use it instead: \
                 mosaic runtime use <bundle directory>",
                e
            )
        })?;
    println!("Runtime bundle {} installed at {}", version, dir);
    Ok(())
}

/// Install a runtime bundle, from whatever was pointed at.
///
/// A bundle directory is linked as it is. A system image is built into a bundle
/// first, which is what a machine with an image and no published bundle needs --
/// and that needs the builder, which lives in a checkout rather than in the package.
pub fn install(args: &MosaicArgs, path: Option<&str>, version: &str) -> anyhow::Result<()> {
    let path = match path {
        Some(path) => path.to_string(),
        None => {
            // The image a Mosaic build leaves behind, so that the common case is
            // one word: `mosaic runtime install`.
            let image = std::env::var("MOSAIC_IMAGE").unwrap_or_else(|_| DEFAULT_IMAGE.to_string());
            anyhow::ensure!(
                std::path::Path::new(&image).exists(),
                "no system image at {}. Pass one: mosaic runtime install <bundle directory | image>",
                image
            );
            image
        }
    };
    let source =
        std::fs::canonicalize(&path).map_err(|e| anyhow::anyhow!("cannot read {}: {}", path, e))?;

    let directory = if source.is_dir() {
        source.to_string_lossy().to_string()
    } else {
        build_from_image(args, &source)?
    };

    let link = crate::runtime::use_local(&args.work, &directory, version)?;
    println!("Runtime bundle {} installed", version);
    println!("  from    {}", directory);
    println!("  linked  {}", link);
    Ok(())
}

/// Build a bundle from a system image, by running the builder this repository
/// ships. Refuses with the reason when there is no builder to run.
fn build_from_image(args: &MosaicArgs, image: &std::path::Path) -> anyhow::Result<String> {
    let script = std::env::var("MOSAIC_BUNDLE_SCRIPT")
        .ok()
        .filter(|s| std::path::Path::new(s).exists())
        .or_else(|| {
            // Where a checkout keeps it, relative to this binary:
            // target/<profile>/mosaic -> ../../tools/bundle/bundle.sh
            let exe = std::env::current_exe().ok()?;
            let root = exe.parent()?.parent()?.parent()?;
            let candidate = root.join("tools/bundle/bundle.sh");
            candidate
                .exists()
                .then(|| candidate.to_string_lossy().to_string())
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{} is an image, not a bundle, and building from one needs \
                 tools/bundle/bundle.sh -- which a checkout has and an installed \
                 package does not. Build it yourself:\n    bundle.sh build {} <directory>\n\
                 then:\n    mosaic runtime install <directory>",
                image.display(),
                image.display()
            )
        })?;

    let runtime = crate::runtime::runtime_dir(&args.work);
    std::fs::create_dir_all(&runtime)?;
    let out = format!("{}/bundle", runtime);
    log::info!("Building a runtime bundle from {}", image.display());
    let status = std::process::Command::new("bash")
        .arg(&script)
        .arg("build")
        .arg(image)
        .arg(&out)
        .status()
        .map_err(|e| anyhow::anyhow!("could not run {}: {}", script, e))?;
    anyhow::ensure!(status.success(), "{} build failed", script);
    Ok(out)
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
