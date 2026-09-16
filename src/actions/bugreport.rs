// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use std::path::Path;
use std::process::Command;

const TARBALL: &str = "mosaic-bugreport.tar.xz";

fn logcat(output_path: &str) -> Option<std::process::Child> {
    let file = std::fs::File::create(output_path).ok()?;
    Command::new("sudo")
        .args(["mosaic", "logcat"])
        .stdout(file)
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn()
        .ok()
}

fn dmesg(params: &[&str], output_path: &str) -> Option<std::process::Child> {
    let file = std::fs::File::create(output_path).ok()?;
    let mut cmd = Command::new("sudo");
    cmd.arg("dmesg").arg("-T");
    for p in params {
        cmd.arg(p);
    }
    cmd.stdout(file.try_clone().ok()?)
        .stderr(file)
        .stdin(std::process::Stdio::null())
        .spawn()
        .ok()
}

fn mosaic_session(output_path: &str) -> Option<std::process::Child> {
    let file = std::fs::File::create(output_path).ok()?;
    let mosaic = std::env::args()
        .next()
        .unwrap_or_else(|| "mosaic".to_string());
    Command::new(mosaic)
        .args(["session", "start"])
        .stdout(file.try_clone().ok()?)
        .stderr(file)
        .stdin(std::process::Stdio::null())
        .spawn()
        .ok()
}

fn sleep_progress(seconds: u64) {
    let bar_length: usize = 50;
    let mut slept: u64 = 0;
    while slept < seconds {
        let filled = (bar_length as u64 * slept / seconds) as usize;
        let spinner = ["|", "/", "-", "\\"][(slept % 4) as usize];
        print!(
            "\r[{}{}] {}",
            "=".repeat(filled),
            " ".repeat(bar_length - filled),
            spinner
        );
        let _ = std::io::Write::flush(&mut std::io::stdout());
        std::thread::sleep(std::time::Duration::from_secs(1));
        slept += 1;
    }
    print!("\r\x1b[2K\r");
    let _ = std::io::Write::flush(&mut std::io::stdout());
}

pub fn bugreport(args: &MosaicArgs) -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let tmp_path = tmp.path().to_string_lossy().to_string();

    println!(
        "The following information will be collected:\n  - System kernel logs (kmsg)\n  - Android system and user logs (logcat)\n  - Mosaic container manager logs (/var/lib/mosaic/mosaic.log)\n  - Mosaic configuration files (/var/lib/mosaic/*)\n\nThe information will be stored on your machine.\n\nPlease authenticate as administrator in order to read system logs.\n"
    );

    if Command::new("sudo").arg("-v").status().is_err() {
        println!("The 'sudo' command is not available. Cannot read system logs.");
        return Ok(());
    }
    if !Command::new("sudo")
        .arg("-v")
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        println!("Authentication failed or was cancelled.");
        return Ok(());
    }
    println!();

    let mut procs: Vec<std::process::Child> = Vec::new();
    let mut logfiles: Vec<String> = Vec::new();
    let mut logfile = |name: &str| {
        let p = format!("{}/{}", tmp_path, name);
        logfiles.push(p.clone());
        p
    };

    let session =
        { crate::helpers::runtime::block_on(crate::helpers::ipc::container_get_session()).ok() };

    let session = if session.is_none() || session.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
        println!("Mosaic session not found. Trying to start one...");
        if let Some(child) = mosaic_session(&logfile("session.txt")) {
            procs.push(child);
        }
        sleep_progress(10);
        crate::helpers::runtime::block_on(crate::helpers::ipc::container_get_session()).ok()
    } else {
        session
    };

    if session.is_some() && session.as_ref().map(|s| !s.is_empty()).unwrap_or(false) {
        println!("\n\x1b[1mPlease try to reproduce the problem now.\x1b[0m\nMosaic will collect logs for up to 5 minutes. You may interrupt this operation earlier.\n");
        if let Some(child) = logcat(&logfile("logcat.txt")) {
            procs.push(child);
        }
        if let Some(child) = dmesg(&["-w"], &logfile("dmesg.txt")) {
            procs.push(child);
        }
        sleep_progress(5 * 60);
    } else {
        println!("Session did not start\n");
        if let Some(child) = dmesg(&[], &logfile("dmesg.txt")) {
            procs.push(child);
        }
    }

    for mut p in procs {
        let _ = p.kill();
        let _ = p.wait();
    }

    println!("Creating archive...");

    let work = &args.work;
    let files = [
        format!("{}/mosaic.log", work),
        format!("{}/mosaic.cfg", work),
        format!("{}/{}", work, crate::guest::BASE_PROP_FILE),
        format!("{}/{}", work, crate::guest::PROP_FILE),
        format!("{}/lxc", work),
    ];

    let tar_file = std::fs::File::create(TARBALL)?;
    let enc = xz2::write::XzEncoder::new(tar_file, 9);
    let mut tar = tar::Builder::new(enc);

    for f in files.iter().chain(logfiles.iter()) {
        if Path::new(f).exists() {
            if Path::new(f).is_dir() {
                let _ = tar.append_dir_all(Path::new(f).file_name().unwrap(), f);
            } else {
                let _ = tar.append_path_with_name(f, Path::new(f).file_name().unwrap());
            }
        }
    }

    let enc = tar.into_inner()?;
    enc.finish()?;

    println!("Created \x1b[1m{}\x1b[0m", TARBALL);

    Ok(())
}
