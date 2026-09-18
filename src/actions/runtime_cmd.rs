// SPDX-License-Identifier: GPL-3.0-or-later

//! `mosaic runtime` — manage the host-native ART and Bionic bundle (ADR-0010).

use crate::args::{MosaicArgs, RuntimeSubaction};

/// Where a Mosaic build keeps the system image it was built from.
const DEFAULT_IMAGE: &str = "/var/lib/mosaic/images/system.img";

pub async fn dispatch(args: &MosaicArgs, subaction: &RuntimeSubaction) -> anyhow::Result<()> {
    match subaction {
        RuntimeSubaction::Fetch => fetch(args).await,
        RuntimeSubaction::Status => status(args),
        RuntimeSubaction::Install { path, version } => {
            install(args, path.as_deref(), version).await
        }
        RuntimeSubaction::Verify => verify(args),
    }
}

pub async fn fetch(args: &MosaicArgs) -> anyhow::Result<()> {
    let config = crate::config::load(&args.config);
    let version = config.bundle_version();
    let channel = config.bundle_channel();
    let dir = crate::runtime::fetch(args, &channel, &version)
        .await
        .map_err(|e| {
            // A 404 has several causes that look identical, and they need different
            // things from whoever reads it. The one worth naming is a release in a
            // private repository: github.com's download URLs answer 404 to everything,
            // token or no token, because they are browser-facing.
            let private = channel.starts_with("https://github.com/")
                && std::env::var("MOSAIC_BUNDLE_TOKEN").is_err();
            anyhow::anyhow!(
                "{}.{} Either build one from a system image -- mosaic runtime install \
                 <image> -- or install a bundle from wherever it is published: \
                 mosaic runtime install <url>",
                e,
                if private {
                    " If that release is in a private repository, github.com's download \
                     URLs are not reachable at all: set MOSAIC_BUNDLE_TOKEN to a token \
                     that can read it, and the release is resolved through the API."
                } else {
                    ""
                }
            )
        })?;
    println!("Runtime bundle {} installed at {}", version, dir);
    Ok(())
}

/// Install a runtime, from whatever was pointed at.
///
/// A directory is linked as it is. A bundle archive is unpacked, and its sha256 is
/// checked when one is published beside it. A system image is built into a bundle
/// first. Any of those may be an http(s) URL, which is the point: a machine with no
/// checkout and no image can still get a runtime with one command.
pub async fn install(args: &MosaicArgs, path: Option<&str>, version: &str) -> anyhow::Result<()> {
    let given = match path {
        Some(path) => path.to_string(),
        None => {
            // The image a Mosaic build leaves behind, so that the common case is one
            // word: `mosaic runtime install`.
            let image = std::env::var("MOSAIC_IMAGE").unwrap_or_else(|_| DEFAULT_IMAGE.to_string());
            anyhow::ensure!(
                std::path::Path::new(&image).exists(),
                "no system image at {}. Pass one: mosaic runtime install <bundle | image | url>",
                image
            );
            image
        }
    };

    // A URL is fetched first; what comes back is a file on disk, like any other. A URL
    // that names a release asset is resolved first: over a private repository the URL
    // itself is not reachable, and the API is.
    let is_url = given.starts_with("http://") || given.starts_with("https://");
    let (fetch_url, sidecar) = if is_url {
        crate::runtime::reachable(&given).await?
    } else {
        (given.clone(), None)
    };

    let local = if is_url {
        log::info!("Fetching {}", fetch_url);
        crate::helpers::http::download_with(
            args,
            &fetch_url,
            "runtime-install",
            true,
            false,
            crate::runtime::bundle_asset_headers(),
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("{} could not be fetched", fetch_url))?
    } else {
        std::fs::canonicalize(&given)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {}", given, e))?
            .to_string_lossy()
            .to_string()
    };

    let version = if version != "local" {
        version.to_string()
    } else {
        std::path::Path::new(&local)
            .file_name()
            .and_then(|name| version_name(&name.to_string_lossy()))
            .unwrap_or_else(|| "local".to_string())
    };

    // A fetched archive is checked against the hash published beside it, the same as
    // `runtime fetch` does: what arrives over a network is what is worth verifying.
    if is_url {
        crate::runtime::verify_checksum(sidecar.as_deref(), &local).await?;
    }

    if std::path::Path::new(&local).is_dir() {
        let link = crate::runtime::use_local(&args.work, &local, &version)?;
        println!("Runtime bundle {} installed", version);
        println!("  from    {}", local);
        println!("  linked  {}", link);
        return Ok(());
    }

    // Classified by what was given, not by where the download landed: the cache path
    // has no extension, so an archive fetched by URL looked like an image and the
    // builder was handed a tar file to read as a filesystem.
    if is_archive(&given) || is_archive(&local) {
        let dir = crate::runtime::unpack_archive(args, &local, &version)?;
        println!("Runtime bundle {} installed", version);
        println!("  from    {}", given);
        println!("  at      {}", dir);
        return Ok(());
    }

    let directory = build_from_image(args, std::path::Path::new(&local))?;
    let link = crate::runtime::use_local(&args.work, &directory, &version)?;
    println!("Runtime bundle {} installed", version);
    println!("  built from {}", given);
    println!("  linked     {}", link);
    Ok(())
}

/// Whether a file is an unpackable bundle rather than a system image.
fn is_archive(path: &str) -> bool {
    path.ends_with(".tar.xz") || path.ends_with(".tar.zst") || path.ends_with(".tar.gz")
}

/// The version in a bundle archive's name, if it is named the way `pack` names it:
/// `runtime-<version>-<arch>.tar.xz`.
fn version_name(name: &str) -> Option<String> {
    let rest = name.strip_prefix("runtime-")?;
    let rest = rest.split(".tar.").next()?;
    let (version, _arch) = rest.rsplit_once('-')?;
    (!version.is_empty()).then(|| version.to_string())
}

/// Where the bundle builder is, if this machine has one./// Where the bundle builder is, if this machine has one.
///
/// Three places, in order: what the environment says, where the package puts it, and
/// where a source checkout keeps it. The package one is why this exists at all --
/// without it a user whose machine has an installed Mosaic cannot build a runtime,
/// and has to wait for the administrator's machine-wide one or a published artifact.
fn bundle_script() -> Option<String> {
    let candidates = [
        std::env::var("MOSAIC_BUNDLE_SCRIPT").ok(),
        Some("/usr/lib/mosaic/bundle.sh".to_string()),
        // A checkout, relative to this binary: target/<profile>/mosaic is three
        // levels below the root that holds tools/.
        std::env::current_exe().ok().and_then(|exe| {
            let root = exe.parent()?.parent()?.parent()?;
            Some(
                root.join("tools/bundle/bundle.sh")
                    .to_string_lossy()
                    .to_string(),
            )
        }),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|path| std::path::Path::new(path).exists())
}

/// Build a bundle from a system image, by running the builder this repository
/// ships. Refuses with the reason when there is no builder to run.
fn build_from_image(args: &MosaicArgs, image: &std::path::Path) -> anyhow::Result<String> {
    let script = bundle_script().ok_or_else(|| {
        anyhow::anyhow!(
            "{} is a system image, so it has to be built into a bundle, and the builder \
             is not here. Either:\n\
             \x20 ask an administrator to install the machine-wide runtime once --\n\
             \x20     sudo mosaic runtime install\n\
             \x20 fetch a published bundle --\n\
             \x20     MOSAIC_BUNDLE_CHANNEL=<where it is served> mosaic runtime fetch\n\
             \x20 or install the package's builder, which a checkout has at {}",
            image.display(),
            "tools/bundle/bundle.sh"
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
    match crate::runtime::resolve(&args.work) {
        Some((dir, version)) => {
            let scope = if std::path::Path::new(&dir)
                .starts_with(crate::runtime::runtime_dir(&args.work))
            {
                "this user's"
            } else {
                "the machine's"
            };
            println!(
                "Runtime bundle {} ({}, {}) at {}",
                version,
                crate::runtime::host_arch(),
                scope,
                dir
            );
        }
        None => {
            println!(
                "No runtime bundle installed. Run 'mosaic runtime install' to build one \n\
                 from this machine's system image."
            )
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

    /// An archive's own name carries its version, which is how a URL installs under
    /// the same version whichever machine fetched it.
    #[test]
    fn a_bundle_archive_names_its_version() {
        assert_eq!(
            version_name("runtime-local-x86_64.tar.xz"),
            Some("local".to_string())
        );
        assert_eq!(
            version_name("runtime-lineage-20-x86_64.tar.zst"),
            Some("lineage-20".to_string()),
            "a version with a dash in it survives"
        );
        assert_eq!(version_name("system.img"), None);
        assert_eq!(version_name("runtime-.tar.xz"), None);
    }

    /// An image is not an archive: one is built into a bundle, the other unpacked.
    #[test]
    fn an_image_is_not_an_archive() {
        assert!(is_archive("/x/runtime-local-x86_64.tar.xz"));
        assert!(is_archive("/x/thing.tar.zst"));
        assert!(!is_archive("/var/lib/mosaic/images/system.img"));
        assert!(!is_archive("/x/vendor.img"));
    }

    /// The builder is looked for in three places, and the package's is the one that
    /// matters for a user whose machine has an installed Mosaic: without it nobody
    /// without a checkout can build a runtime.
    #[test]
    fn the_bundle_builder_is_found_where_it_is_put() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("bundle.sh");
        std::fs::write(&script, b"#!/bin/bash\n").unwrap();
        std::env::set_var("MOSAIC_BUNDLE_SCRIPT", &script);
        assert_eq!(bundle_script(), Some(script.to_string_lossy().to_string()));

        // One that does not exist is skipped rather than used, which is how the
        // package's copy gets its turn.
        std::env::set_var("MOSAIC_BUNDLE_SCRIPT", dir.path().join("absent.sh"));
        assert_ne!(
            bundle_script(),
            Some(dir.path().join("absent.sh").to_string_lossy().to_string())
        );
        std::env::remove_var("MOSAIC_BUNDLE_SCRIPT");
    }
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
        assert!(err.contains("runtime install"), "got: {}", err);
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
