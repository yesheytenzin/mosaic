// SPDX-License-Identifier: GPL-3.0-or-later

pub fn versiontuple(v: &str) -> Vec<u32> {
    v.split('.').filter_map(|s| s.parse::<u32>().ok()).collect()
}

pub fn kernel_version() -> (u32, u32) {
    let release = nix::sys::utsname::uname()
        .map(|u| u.release().to_string_lossy().to_string())
        .unwrap_or_else(|_| "5.0".to_string());
    let mut parts = release.split('.');
    let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(5);
    let minor = parts
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    (major, minor)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versiontuple_works() {
        assert_eq!(versiontuple("1.3.4"), vec![1, 3, 4]);
        assert_eq!(versiontuple("1.6.0"), vec![1, 6, 0]);
        assert!(versiontuple("1.3.4") <= versiontuple("1.3.4"));
        assert!(versiontuple("1.3.3") < versiontuple("1.3.4"));
    }
}
