// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use std::collections::HashMap;

pub fn host_get(key: &str) -> String {
    // Prefer the host `getprop` binary, then fall back to build.prop files.
    if let Ok(output) = std::process::Command::new("getprop").arg(key).output() {
        let val = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !val.is_empty() {
            return val;
        }
    }
    for path in ["/system/build.prop", "/vendor/build.prop"] {
        if let Some(v) = file_get(path, key) {
            return v;
        }
    }
    String::new()
}

pub fn host_list(prefix: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Ok(output) = std::process::Command::new("getprop").output() {
        let out = String::from_utf8_lossy(&output.stdout);
        for line in out.lines() {
            // getprop output is like [ro.build.fingerprint]: [value]
            if let Some((k, v)) = parse_getprop_line(line) {
                if k.starts_with(prefix) {
                    map.insert(k, v);
                }
            }
        }
    }
    map
}

pub fn host_set(args: &MosaicArgs, prop: &str, value: &str) {
    if which::which("setprop").is_ok() {
        let _ = crate::helpers::run::user(
            args,
            &["setprop".to_string(), prop.to_string(), value.to_string()],
            "log",
            false,
            Some(true),
        );
    }
}

fn parse_getprop_line(line: &str) -> Option<(String, String)> {
    // line: [key]: [value]
    let line = line.trim();
    if !line.starts_with('[') {
        return None;
    }
    let key_end = line.find("]:")?;
    let key = line[1..key_end].to_string();
    let val_start = line[key_end + 2..].trim();
    if val_start.starts_with('[') && val_start.ends_with(']') {
        let val = val_start[1..val_start.len() - 1].to_string();
        Some((key, val))
    } else {
        None
    }
}

pub fn file_get(path: &str, key: &str) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(eq) = line.find('=') {
            let k = line[..eq].trim();
            let v = line[eq + 1..].trim();
            if k == key {
                return Some(v.to_string());
            }
        }
    }
    None
}

pub fn file_get_all(path: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Ok(content) = std::fs::read_to_string(path) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(eq) = line.find('=') {
                let k = line[..eq].trim().to_string();
                let v = line[eq + 1..].trim().to_string();
                map.insert(k, v);
            }
        }
    }
    map
}

pub fn get(args: &crate::args::MosaicArgs, prop: &str) -> Option<String> {
    if let Some(platform) = crate::interfaces::i_platform::get_service(args) {
        return platform.getprop(prop, "");
    }
    log::error!("Failed to access IPlatform service");
    None
}

pub fn set(args: &crate::args::MosaicArgs, prop: &str, value: &str) {
    if let Some(platform) = crate::interfaces::i_platform::get_service(args) {
        platform.setprop(prop, value);
    } else {
        log::error!("Failed to access IPlatform service");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn file_get_reads_value() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "ro.build.version.sdk=33").unwrap();
        writeln!(f, "ro.product.device=mosaic").unwrap();
        let path = f.path().to_str().unwrap();
        assert_eq!(file_get(path, "ro.build.version.sdk").unwrap(), "33");
        assert_eq!(file_get(path, "ro.product.device").unwrap(), "mosaic");
        assert!(file_get(path, "missing").is_none());
    }

    #[test]
    fn parse_getprop_line_works() {
        assert_eq!(
            parse_getprop_line("[ro.build.fingerprint]: [google/foo/bar]"),
            Some((
                "ro.build.fingerprint".to_string(),
                "google/foo/bar".to_string()
            ))
        );
    }
}
