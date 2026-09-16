// SPDX-License-Identifier: GPL-3.0-or-later

//! Bridges the guest clipboard to the host, via wl-clipboard on Wayland or
//! xclip on X11.

use crate::args::MosaicArgs;
use crate::interfaces::i_clipboard;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

static STOPPING: AtomicBool = AtomicBool::new(true);

fn copy(value: &str) -> anyhow::Result<()> {
    let (program, args) = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        ("wl-copy", vec![])
    } else {
        ("xclip", vec!["-selection", "clipboard"])
    };
    let mut child = Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    if let Some(stdin) = child.stdin.as_mut() {
        use std::io::Write;
        stdin.write_all(value.as_bytes())?;
    }
    let _ = child.wait();
    Ok(())
}

fn paste() -> String {
    let (program, args) = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        ("wl-paste", vec!["--no-newline"])
    } else {
        ("xclip", vec!["-selection", "clipboard", "-o"])
    };
    match Command::new(program).args(args).output() {
        Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
        Err(e) => {
            log::debug!("{}", e);
            String::new()
        }
    }
}

fn available() -> bool {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        which::which("wl-copy").is_ok() && which::which("wl-paste").is_ok()
    } else {
        which::which("xclip").is_ok()
    }
}

pub fn start(args: &MosaicArgs) -> anyhow::Result<()> {
    if !available() {
        log::debug!("Skipping clipboard manager service because no clipboard tool is available");
        return Ok(());
    }
    STOPPING.store(false, Ordering::SeqCst);
    let args = args.clone();
    std::thread::spawn(move || {
        while !STOPPING.load(Ordering::SeqCst) {
            i_clipboard::add_service(
                &args,
                |value| {
                    if let Err(e) = copy(&value) {
                        log::debug!("{}", e);
                    }
                },
                paste,
                &STOPPING,
            );
        }
    });
    Ok(())
}

pub fn stop(_args: &MosaicArgs) -> anyhow::Result<()> {
    STOPPING.store(true, Ordering::SeqCst);
    Ok(())
}
