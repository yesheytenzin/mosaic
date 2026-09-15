// SPDX-License-Identifier: GPL-3.0-or-later

use clap::{Args as ClapArgs, Parser, Subcommand};

#[derive(Parser, Debug, Clone)]
#[command(
    name = "mosaic",
    version = crate::config::VERSION,
    about = "Mosaic container manager",
    long_about = None,
    disable_version_flag = true
)]
pub struct Cli {
    #[arg(short = 'V', long = "version", action = clap::ArgAction::Version)]
    pub version: Option<bool>,

    #[arg(short = 'l', long = "log", value_name = "PATH")]
    pub log: Option<String>,

    #[arg(long = "details-to-stdout", action = clap::ArgAction::SetTrue)]
    pub details_to_stdout: bool,

    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::SetTrue)]
    pub verbose: bool,

    #[arg(short = 'q', long = "quiet", action = clap::ArgAction::SetTrue)]
    pub quiet: bool,

    #[arg(short = 'w', long = "work", hide = true)]
    pub work: Option<String>,

    #[arg(short = 't', long = "timeout", hide = true, default_value = "1800")]
    pub timeout: u64,

    #[command(subcommand)]
    pub action: Option<Action>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Action {
    Status,
    Log(LogArgs),
    Init(InitArgs),
    Upgrade(UpgradeArgs),
    Session(SessionArgs),
    Container(ContainerArgs),
    App(AppArgs),
    Prop(PropArgs),
    #[command(name = "show-full-ui")]
    ShowFullUi,
    #[command(name = "first-launch")]
    FirstLaunch,
    Shell(ShellArgs),
    Logcat(LogcatArgs),
    Adb(AdbArgs),
    Bugreport,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct LogArgs {
    #[arg(short = 'n', long = "lines", default_value = "60")]
    pub lines: String,

    #[arg(short = 'c', long = "clear", action = clap::ArgAction::SetTrue)]
    pub clear_log: bool,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct InitArgs {
    #[arg(short = 'i', long = "images_path")]
    pub images_path: Option<String>,

    #[arg(short = 'f', long = "force", action = clap::ArgAction::SetTrue)]
    pub force: bool,

    #[arg(short = 'c', long = "system_channel")]
    pub system_channel: Option<String>,

    #[arg(short = 'v', long = "vendor_channel")]
    pub vendor_channel: Option<String>,

    #[arg(short = 'r', long = "rom_type")]
    pub rom_type: Option<String>,

    #[arg(short = 's', long = "system_type")]
    pub system_type: Option<String>,

    #[arg(long = "client", action = clap::ArgAction::SetTrue)]
    pub client: bool,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct UpgradeArgs {
    #[arg(short = 'o', long = "offline", action = clap::ArgAction::SetTrue)]
    pub offline: bool,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct SessionArgs {
    #[command(subcommand)]
    pub subaction: Option<SessionSubaction>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum SessionSubaction {
    Start,
    Stop,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct ContainerArgs {
    #[command(subcommand)]
    pub subaction: Option<ContainerSubaction>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum ContainerSubaction {
    Start,
    Stop,
    Restart,
    Freeze,
    Unfreeze,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct AppArgs {
    #[command(subcommand)]
    pub subaction: Option<AppSubaction>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum AppSubaction {
    Install { package: String },
    Remove { package: String },
    Launch { package: String },
    Intent { action: String, uri: String },
    List,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct PropArgs {
    #[command(subcommand)]
    pub subaction: Option<PropSubaction>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum PropSubaction {
    Get { key: String },
    Set { key: String, value: String },
}

#[derive(ClapArgs, Debug, Clone)]
pub struct ShellArgs {
    #[arg(short = 'u', long = "uid")]
    pub uid: Option<String>,

    #[arg(short = 'g', long = "gid")]
    pub gid: Option<String>,

    #[arg(short = 's', long = "context")]
    pub context: Option<String>,

    #[arg(short = 'L', long = "nolsm", action = clap::ArgAction::SetTrue)]
    pub nolsm: bool,

    #[arg(short = 'C', long = "allcaps", action = clap::ArgAction::SetTrue)]
    pub allcaps: bool,

    #[arg(short = 'G', long = "nocgroup", action = clap::ArgAction::SetTrue)]
    pub nocgroup: bool,

    #[arg(value_name = "COMMAND", num_args = 0.., allow_hyphen_values = true)]
    pub command: Vec<String>,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct LogcatArgs {
    #[arg(value_name = "ARGS", num_args = 0.., allow_hyphen_values = true)]
    pub args: Vec<String>,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct AdbArgs {
    #[command(subcommand)]
    pub subaction: Option<AdbSubaction>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum AdbSubaction {
    Connect,
    Disconnect,
}

#[derive(Debug, Clone)]
pub struct MosaicArgs {
    pub cli: Cli,
    pub work: String,
    pub config: String,
    pub log: String,
    pub sudo_timer: bool,
    pub timeout: u64,
    pub cache: std::collections::HashMap<String, String>,
    pub details_to_stdout: bool,
    pub verbose: bool,
    pub quiet: bool,
}

impl MosaicArgs {
    pub fn from_cli(cli: Cli) -> Self {
        let work = cli
            .work
            .clone()
            .unwrap_or_else(|| "/var/lib/mosaic".to_string());
        let config = format!("{}/mosaic.cfg", work);
        let log_path = cli
            .log
            .clone()
            .unwrap_or_else(|| format!("{}/mosaic.log", work));

        let timeout = cli.timeout;
        let details_to_stdout = cli.details_to_stdout;
        let verbose = cli.verbose;
        let quiet = cli.quiet;

        Self {
            work,
            config,
            log: log_path,
            sudo_timer: true,
            timeout,
            cache: std::collections::HashMap::new(),
            cli,
            details_to_stdout,
            verbose,
            quiet,
        }
    }

    pub fn action_name(&self) -> Option<String> {
        match &self.cli.action {
            Some(Action::Status) => Some("status".to_string()),
            Some(Action::Log(_)) => Some("log".to_string()),
            Some(Action::Init(_)) => Some("init".to_string()),
            Some(Action::Upgrade(_)) => Some("upgrade".to_string()),
            Some(Action::Session(_)) => Some("session".to_string()),
            Some(Action::Container(_)) => Some("container".to_string()),
            Some(Action::App(_)) => Some("app".to_string()),
            Some(Action::Prop(_)) => Some("prop".to_string()),
            Some(Action::ShowFullUi) => Some("show-full-ui".to_string()),
            Some(Action::FirstLaunch) => Some("first-launch".to_string()),
            Some(Action::Shell(_)) => Some("shell".to_string()),
            Some(Action::Logcat(_)) => Some("logcat".to_string()),
            Some(Action::Adb(_)) => Some("adb".to_string()),
            Some(Action::Bugreport) => Some("bugreport".to_string()),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parse_status() {
        let cli = Cli::try_parse_from(["mosaic", "status"]).unwrap();
        assert!(matches!(cli.action, Some(Action::Status)));
    }

    #[test]
    fn parse_init_with_options() {
        let cli = Cli::try_parse_from(["mosaic", "init", "-f", "-c", "https://example.com/system"])
            .unwrap();
        if let Some(Action::Init(args)) = cli.action {
            assert!(args.force);
            assert_eq!(args.system_channel.unwrap(), "https://example.com/system");
        } else {
            panic!("expected init");
        }
    }

    #[test]
    fn parse_app_install() {
        let cli = Cli::try_parse_from(["mosaic", "app", "install", "/tmp/foo.apk"]).unwrap();
        if let Some(Action::App(a)) = cli.action {
            if let Some(AppSubaction::Install { package }) = a.subaction {
                assert_eq!(package, "/tmp/foo.apk");
            } else {
                panic!("expected install");
            }
        } else {
            panic!("expected app");
        }
    }

    #[test]
    fn parse_shell_with_flags() {
        let cli = Cli::try_parse_from(["mosaic", "shell", "-u", "1000", "ls", "-l"]).unwrap();
        if let Some(Action::Shell(s)) = cli.action {
            assert_eq!(s.uid.unwrap(), "1000");
            assert_eq!(s.command, vec!["ls", "-l"]);
        } else {
            panic!("expected shell");
        }
    }
}
