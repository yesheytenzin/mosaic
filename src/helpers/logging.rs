// SPDX-License-Identifier: GPL-3.0-or-later

//! Logging setup. stdout gets INFO and above, the container log file gets
//! DEBUG and above and rotates at 5 MB keeping one backup, matching the
//! original RotatingFileHandler bounds.

use log::LevelFilter;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const MAX_BYTES: u64 = 5 * 1024 * 1024;
const BACKUPS: usize = 1;

fn rotated_path(path: &Path, index: usize) -> PathBuf {
    PathBuf::from(format!("{}.{}", path.display(), index))
}

/// Append-only file that rotates itself once it exceeds MAX_BYTES.
struct RotatingFile {
    path: PathBuf,
    file: File,
    size: u64,
}

impl RotatingFile {
    fn open(path: &str) -> io::Result<Self> {
        let path = PathBuf::from(path);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self { path, file, size })
    }

    fn rotate(&mut self) -> io::Result<()> {
        for index in (1..BACKUPS).rev() {
            let from = rotated_path(&self.path, index);
            let to = rotated_path(&self.path, index + 1);
            let _ = std::fs::rename(from, to);
        }
        let _ = std::fs::rename(&self.path, rotated_path(&self.path, 1));
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        self.size = 0;
        Ok(())
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.size + buf.len() as u64 > MAX_BYTES {
            let _ = self.rotate();
        }
        let written = self.file.write(buf)?;
        self.size += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

pub fn init(
    verbose: bool,
    quiet: bool,
    log_path: &str,
    action: Option<&str>,
    details_to_stdout: bool,
) {
    let stdout_level = if quiet {
        LevelFilter::Warn
    } else {
        LevelFilter::Info
    };

    let stdout_dispatch = fern::Dispatch::new()
        .format(|out, message, _record| {
            out.finish(format_args!(
                "[{}] {}",
                chrono::Local::now().format("%H:%M:%S"),
                message
            ))
        })
        .level(stdout_level)
        .chain(std::io::stdout());

    let should_log_file = action == Some("container") && !details_to_stdout;

    if should_log_file {
        if let Some(parent) = Path::new(log_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(log_path, std::fs::Permissions::from_mode(0o644));
        }

        let writer = RotatingFile::open(log_path)
            .or_else(|_| RotatingFile::open("/tmp/mosaic.log"))
            .ok();

        let _ = match writer {
            Some(file) => fern::Dispatch::new()
                .level(LevelFilter::Debug)
                .chain(stdout_dispatch)
                .chain(
                    fern::Dispatch::new()
                        .format(|out, message, _record| {
                            out.finish(format_args!(
                                "({}) [{}] {}",
                                std::process::id(),
                                chrono::Local::now().format("%a, %d %b %Y %H:%M:%S"),
                                message
                            ))
                        })
                        .level(LevelFilter::Debug)
                        .chain(fern::Output::writer(Box::new(file), "\n")),
                )
                .apply(),
            None => stdout_dispatch.apply(),
        };
    } else {
        let _ = stdout_dispatch.apply();
    }

    if verbose {
        log::set_max_level(LevelFilter::Trace);
    } else {
        log::set_max_level(LevelFilter::Debug);
    }
}

pub fn disable() {
    log::set_max_level(LevelFilter::Off);
}

pub fn init_for_test() {
    let _ = fern::Dispatch::new()
        .level(LevelFilter::Debug)
        .chain(fern::Output::call(|_record| {}))
        .apply();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_once_past_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mosaic.log");
        let path_str = path.to_str().unwrap();

        let mut file = RotatingFile::open(path_str).unwrap();
        // Two writes that together exceed the 5 MB limit.
        let chunk = vec![b'x'; MAX_BYTES as usize];
        file.write_all(&chunk).unwrap();
        file.write_all(b"second").unwrap();
        file.flush().unwrap();

        assert!(path.exists(), "current log must still exist");
        assert!(
            rotated_path(&path, 1).exists(),
            "the first backup must be created after rotating"
        );
    }

    #[test]
    fn stays_put_below_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mosaic.log");
        let mut file = RotatingFile::open(path.to_str().unwrap()).unwrap();
        file.write_all(b"small").unwrap();
        file.flush().unwrap();
        assert!(!rotated_path(&path, 1).exists());
    }
}
