// SPDX-License-Identifier: GPL-3.0-or-later

use crate::helpers::process;

pub fn user(
    args: &crate::args::MosaicArgs,
    cmd: &[String],
    output: &str,
    output_return: bool,
    check: Option<bool>,
) -> anyhow::Result<String> {
    let msg = format!("% {}", cmd.join(" "));
    let check = check.unwrap_or(true);
    let result = process::core(
        args,
        &msg,
        cmd,
        None,
        output,
        output_return,
        check,
        false,
        false,
    )?;
    Ok(result.unwrap_or_default())
}

pub fn root(
    args: &crate::args::MosaicArgs,
    cmd: &[String],
    output: &str,
    output_return: bool,
    check: Option<bool>,
) -> anyhow::Result<String> {
    let mut full = vec!["sudo".to_string()];
    full.extend_from_slice(cmd);
    user(args, &full, output, output_return, check)
}

pub fn flat_cmd(
    cmd: &[String],
    working_dir: Option<&str>,
    env: &std::collections::HashMap<String, String>,
) -> String {
    let mut escaped = Vec::new();
    for (k, v) in env {
        escaped.push(format!("{}={}", k, shell_escape(v)));
    }
    for c in cmd {
        escaped.push(shell_escape(c));
    }
    let mut ret = escaped.join(" ");
    if let Some(wd) = working_dir {
        ret = format!("cd {};{}", shell_escape(wd), ret);
    }
    ret
}

fn shell_escape(s: &str) -> String {
    if s.contains(' ') || s.contains('"') || s.contains('\'') || s.contains('$') {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        s.to_string()
    }
}
