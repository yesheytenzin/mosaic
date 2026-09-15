// SPDX-License-Identifier: GPL-3.0-or-later

use log::LevelFilter;
use std::path::Path;

pub fn init(
    verbose: bool,
    quiet: bool,
    log_path: &str,
    action: Option<&str>,
    details_to_stdout: bool,
) {
    let _level = if verbose {
        LevelFilter::Trace
    } else {
        LevelFilter::Debug
    };

    // Quiet suppresses INFO to stdout but not file
    let stdout_level = if quiet {
        LevelFilter::Warn
    } else {
        LevelFilter::Info
    };

    let builder = fern::Dispatch::new()
        .format(|out, message, _record| {
            out.finish(format_args!(
                "[{}] {}",
                chrono::Local::now().format("%H:%M:%S"),
                message
            ))
        })
        .level(stdout_level)
        .chain(std::io::stdout());

    // Only container action logs to file unless details_to_stdout
    let should_log_file = if let Some(act) = action {
        act == "container" && !details_to_stdout
    } else {
        false
    };

    if should_log_file {
        if let Some(parent) = Path::new(log_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Ensure file exists and is 644
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(log_path, std::fs::Permissions::from_mode(0o644));
        }

        let file_dispatch = fern::Dispatch::new()
            .format(|out, message, _record| {
                out.finish(format_args!(
                    "({}) [{}] {}",
                    std::process::id(),
                    chrono::Local::now().format("%a, %d %b %Y %H:%M:%S"),
                    message
                ))
            })
            .level(LevelFilter::Debug)
            .chain(fern::log_file(log_path).unwrap_or_else(|_| {
                // Fallback to stdout if file can't be opened
                fern::log_file("/tmp/mosaic.log").expect("failed to open fallback log")
            }));

        // Combine stdout + file
        let _ = fern::Dispatch::new()
            .level(LevelFilter::Debug)
            .chain(builder)
            .chain(file_dispatch)
            .apply();
    } else {
        let _ = builder.apply();
    }

    if verbose {
        log::set_max_level(LevelFilter::Trace);
    } else {
        log::set_max_level(LevelFilter::Debug);
    }
}

pub fn init_for_test() {
    let _ = fern::Dispatch::new()
        .level(log::LevelFilter::Debug)
        .chain(fern::Output::call(|_record| {
            // no-op in tests, just ensure logger initialized
        }))
        .apply();
}
