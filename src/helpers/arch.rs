// SPDX-License-Identifier: GPL-3.0-or-later

pub fn host() -> String {
    let machine = std::process::Command::new("uname")
        .arg("-m")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "x86_64".to_string());

    match machine.as_str() {
        "x86_64" | "amd64" => "x86_64".to_string(),
        "aarch64" | "arm64" => "arm64".to_string(),
        "armv7l" | "arm" => "arm".to_string(),
        "x86" | "i686" | "i386" => "x86".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_arch_not_empty() {
        assert!(!host().is_empty());
    }
}
