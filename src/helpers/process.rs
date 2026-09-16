// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

static SUDO_TIMER_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Recursively kill a pid and its children, so a timeout does not leave
/// grandchildren (sh -c, helpers) running.
pub fn kill_process_tree(args: &MosaicArgs, pid: u32, ppids: &[(String, String)], sudo: bool) {
    let cmd = vec!["kill".to_string(), "-9".to_string(), pid.to_string()];
    if sudo {
        let _ = crate::helpers::run::root(args, &cmd, "log", false, Some(false));
    } else {
        let _ = crate::helpers::run::user(args, &cmd, "log", false, Some(false));
    }

    for (child_pid, child_ppid) in ppids {
        if *child_ppid == pid.to_string() {
            if let Ok(child) = child_pid.parse::<u32>() {
                kill_process_tree(args, child, ppids, sudo);
            }
        }
    }
}

/// Kill a command and everything it spawned.
pub fn kill_command(args: &MosaicArgs, pid: u32, sudo: bool) {
    let mut ppids = Vec::new();
    if let Ok(out) = std::process::Command::new("ps")
        .args(["-e", "-o", "pid,ppid"])
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        for row in text.lines().skip(1) {
            let items: Vec<&str> = row.split_whitespace().collect();
            if items.len() != 2 {
                continue;
            }
            ppids.push((items[0].to_string(), items[1].to_string()));
        }
    }
    kill_process_tree(args, pid, &ppids, sudo);
}

/// Keep the sudo timestamp fresh so the password is only asked once.
pub fn sudo_timer_start() {
    if SUDO_TIMER_ACTIVE.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| loop {
        let _ = std::process::Command::new("sudo").arg("-v").status();
        std::thread::sleep(Duration::from_secs(60));
    });
}

#[derive(Debug, Clone, PartialEq)]
pub enum OutputMode {
    Log,
    Stdout,
    Interactive,
    Tui,
    Background,
    Pipe,
}

impl OutputMode {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "log" => Ok(Self::Log),
            "stdout" => Ok(Self::Stdout),
            "interactive" => Ok(Self::Interactive),
            "tui" => Ok(Self::Tui),
            "background" => Ok(Self::Background),
            "pipe" => Ok(Self::Pipe),
            _ => anyhow::bail!("Invalid output value: {}", s),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn core(
    args: &MosaicArgs,
    log_message: &str,
    cmd: &[String],
    working_dir: Option<&str>,
    output: &str,
    output_return: bool,
    check: bool,
    sudo: bool,
    disable_timeout: bool,
) -> anyhow::Result<Option<String>> {
    let mode = OutputMode::from_str(output)?;

    if mode == OutputMode::Background && output_return {
        anyhow::bail!("Can't use output_return with output: background");
    }
    if mode == OutputMode::Tui && output_return {
        anyhow::bail!("Can't use output_return with output: tui");
    }

    if args.sudo_timer && sudo {
        sudo_timer_start();
    }

    log::debug!("{}", log_message);
    log::trace!("run: {:?}", cmd);

    if cmd.is_empty() {
        anyhow::bail!("Empty command");
    }

    match mode {
        OutputMode::Background => {
            let mut command = std::process::Command::new(&cmd[0]);
            command.args(&cmd[1..]);
            if let Some(wd) = working_dir {
                command.current_dir(wd);
            }
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
            let mut child = command.spawn()?;
            let pid = child.id();
            log::debug!("New background process: pid={}, output=background", pid);
            // Spawn threads to forward output to log
            if let Some(stdout) = child.stdout.take() {
                std::thread::spawn(move || {
                    use std::io::BufRead;
                    let reader = std::io::BufReader::new(stdout);
                    for line in reader.lines().map_while(Result::ok) {
                        log::debug!("{}", line);
                    }
                });
            }
            if let Some(stderr) = child.stderr.take() {
                std::thread::spawn(move || {
                    use std::io::BufRead;
                    let reader = std::io::BufReader::new(stderr);
                    for line in reader.lines().map_while(Result::ok) {
                        log::debug!("{}", line);
                    }
                });
            }
            // Don't wait
            std::mem::forget(child);
            return Ok(None);
        }
        OutputMode::Pipe => {
            let mut command = std::process::Command::new(&cmd[0]);
            command.args(&cmd[1..]);
            if let Some(wd) = working_dir {
                command.current_dir(wd);
            }
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
            let child = command.spawn()?;
            log::debug!("New background process: pid={}, output=pipe", child.id());
            std::mem::forget(child);
            return Ok(None);
        }
        _ => {}
    }

    let output_to_stdout =
        !args.details_to_stdout && matches!(mode, OutputMode::Stdout | OutputMode::Interactive);
    let output_timeout = matches!(mode, OutputMode::Log | OutputMode::Stdout) && !disable_timeout;

    if mode == OutputMode::Tui {
        let mut command = std::process::Command::new(&cmd[0]);
        command.args(&cmd[1..]);
        if let Some(wd) = working_dir {
            command.current_dir(wd);
        }
        let status = command.status()?;
        if check && !status.success() {
            anyhow::bail!("Command failed: {}", log_message);
        }
        return Ok(None);
    }

    // Foreground with pipe, implement timeout
    let mut command = std::process::Command::new(&cmd[0]);
    command.args(&cmd[1..]);
    if let Some(wd) = working_dir {
        command.current_dir(wd);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = command.spawn()?;

    let timeout_duration = Duration::from_secs(args.timeout);

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let output_buffer = Arc::new(Mutex::new(Vec::<u8>::new()));
    let output_buffer_clone = output_buffer.clone();

    // Shared timestamp of the most recent output, so the supervising loop kills
    // only a process that has actually gone silent.
    let last_output = Arc::new(Mutex::new(std::time::Instant::now()));
    let last_output_out = last_output.clone();
    let last_output_err = last_output.clone();

    let should_capture = output_return;

    let handle_out = std::thread::spawn(move || {
        use std::io::Read;
        let mut reader = std::io::BufReader::new(stdout);
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    *last_output_out.lock().unwrap() = std::time::Instant::now();
                    let chunk = &buf[..n];
                    for line in chunk.split(|&b| b == b'\n') {
                        if !line.is_empty() {
                            let s = String::from_utf8_lossy(line);
                            log::debug!("{}", s);
                            if output_to_stdout {
                                println!("{}", s);
                            }
                        }
                    }
                    if should_capture {
                        output_buffer_clone.lock().unwrap().extend_from_slice(chunk);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });

    let handle_err = std::thread::spawn(move || {
        use std::io::Read;
        let mut reader = std::io::BufReader::new(stderr);
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    *last_output_err.lock().unwrap() = std::time::Instant::now();
                    let chunk = &buf[..n];
                    for line in chunk.split(|&b| b == b'\n') {
                        if !line.is_empty() {
                            log::debug!("{}", String::from_utf8_lossy(line));
                        }
                    }
                }
                Err(_) => break,
            }
        }
    });

    loop {
        match child.try_wait()? {
            Some(status) => {
                handle_out.join().ok();
                handle_err.join().ok();
                let output_str = if output_return {
                    String::from_utf8_lossy(&output_buffer.lock().unwrap()).to_string()
                } else {
                    String::new()
                };
                if check && !status.success() {
                    log::debug!("{}", "^".repeat(70));
                    log::info!(
                        "NOTE: The failed command's output is above the ^^^ line in the log file: {}",
                        args.log
                    );
                    anyhow::bail!("Command failed: {}", log_message);
                }
                if output_return {
                    return Ok(Some(output_str));
                } else {
                    return Ok(Some(String::new()));
                }
            }
            None => {
                let silent_for = last_output.lock().unwrap().elapsed();
                if output_timeout && silent_for >= timeout_duration {
                    log::info!(
                        "Process did not write any output for {} seconds. Killing it.",
                        args.timeout
                    );
                    log::info!("NOTE: The timeout can be increased with 'mosaic -t'.");
                    kill_command(args, child.id(), sudo);
                    let _ = child.wait();
                    handle_out.join().ok();
                    handle_err.join().ok();
                    anyhow::bail!("Command failed: {} (timeout)", log_message);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{Cli, MosaicArgs};
    use clap::Parser;

    fn test_args(timeout: u64) -> MosaicArgs {
        let cli = Cli::try_parse_from(["mosaic", "-t", &timeout.to_string(), "status"]).unwrap();
        MosaicArgs::from_cli(cli)
    }

    #[test]
    fn foreground_command_returns_stdout() {
        let args = test_args(30);
        let out = core(
            &args,
            "% echo hello",
            &["echo".to_string(), "hello".to_string()],
            None,
            "log",
            true,
            true,
            false,
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(out.trim(), "hello");
    }

    #[test]
    fn foreground_command_fails_on_nonzero_exit() {
        let args = test_args(30);
        let err = core(
            &args,
            "% false",
            &["false".to_string()],
            None,
            "log",
            false,
            true,
            false,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("Command failed"));
    }

    #[test]
    fn silent_process_is_killed_after_timeout() {
        let args = test_args(1);
        let start = std::time::Instant::now();
        let err = core(
            &args,
            "% sleep 60",
            &["sleep".to_string(), "60".to_string()],
            None,
            "log",
            false,
            true,
            false,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("timeout"));
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn chatty_process_survives_past_timeout() {
        // Emits output every 200ms for ~2s while the silence timeout is 1s.
        // A process that keeps talking must not be killed.
        let args = test_args(1);
        let script = "for i in $(seq 1 10); do echo tick $i; sleep 0.2; done";
        let out = core(
            &args,
            "% ticker",
            &["sh".to_string(), "-c".to_string(), script.to_string()],
            None,
            "log",
            true,
            true,
            false,
            false,
        )
        .unwrap()
        .unwrap();
        assert!(
            out.contains("tick 10"),
            "expected full output, got: {}",
            out
        );
    }
}
