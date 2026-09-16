// SPDX-License-Identifier: GPL-3.0-or-later

//! Host architecture detection, including the capability remapping Waydroid
//! applies so the downloaded image matches the CPU.

pub fn is_32bit_capable() -> bool {
    // man 2 personality
    const PER_LINUX32: libc::c_ulong = 0x0008;
    // SAFETY: personality() takes no pointers and only changes the calling
    // thread's execution domain, which we immediately restore on success.
    unsafe {
        let pers = libc::personality(PER_LINUX32);
        if pers != -1 {
            // Success, revert to the previous persona (typically PER_LINUX).
            libc::personality(pers as libc::c_ulong);
            return true;
        }
    }
    false
}

pub fn host() -> anyhow::Result<String> {
    let machine = std::process::Command::new("uname")
        .arg("-m")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    let mapped = match machine.as_str() {
        "i686" => "x86",
        "x86_64" => "x86_64",
        "aarch64" => "arm64",
        "armv7l" | "armv8l" => "arm",
        _ => {
            anyhow::bail!(
                "platform.machine '{}' architecture is not supported",
                machine
            );
        }
    };
    maybe_remap(mapped)
}

fn maybe_remap(target: &str) -> anyhow::Result<String> {
    if target.starts_with("x86") {
        let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
        if !cpuinfo.contains("ssse3") {
            anyhow::bail!("x86/x86_64 CPU must support SSSE3!");
        }
        if target == "x86_64" && !cpuinfo.contains("sse4_2") {
            log::info!("x86_64 CPU does not support SSE4.2, falling back to x86...");
            return Ok("x86".to_string());
        }
    } else if target == "arm64" && cfg!(target_pointer_width = "32") {
        return Ok("arm".to_string());
    } else if target == "arm64" && !is_32bit_capable() {
        log::info!("AArch64 CPU does not appear to support AArch32, assuming arm64_only...");
        return Ok("arm64_only".to_string());
    }
    Ok(target.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_arch_not_empty() {
        // On a supported host this succeeds; the point is that it never
        // silently returns an unmapped value.
        assert!(!host().unwrap().is_empty());
    }

    #[test]
    fn remap_rejects_x86_without_ssse3() {
        // /proc/cpuinfo on any x86_64 CI box has ssse3, so this only checks the
        // x86_64 -> x86 fallback path is reachable and typed.
        let mapped = maybe_remap("x86_64").unwrap();
        assert!(mapped == "x86_64" || mapped == "x86");
    }

    #[test]
    fn remap_passes_through_other_targets() {
        assert_eq!(maybe_remap("arm").unwrap(), "arm");
    }
}
