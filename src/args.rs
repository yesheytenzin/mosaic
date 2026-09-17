// SPDX-License-Identifier: GPL-3.0-or-later

//! Command line surface for the native-execution model.
//!
//! There is no container, no session and no image. The verbs are the broker's:
//! install an app (allocate its user), launch it, query what is installed. Two
//! more run a process in a special mode: the daemon itself, and the privileged
//! UID helper the broker calls.

use clap::{Args as ClapArgs, Parser, Subcommand};

#[derive(Parser, Debug, Clone)]
#[command(
    name = "mosaic",
    version = crate::config::VERSION,
    about = "Run Android apps as native Linux processes",
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

    /// Seconds a command may go without output before it is killed
    #[arg(short = 't', long = "timeout", default_value = "60", hide = true)]
    pub timeout: u64,

    #[command(subcommand)]
    pub action: Option<Action>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Action {
    /// Install an APK and allocate the app its own system user
    Install(InstallArgs),
    /// Remove an installed app and its user data
    Uninstall(UninstallArgs),
    /// Launch an installed app
    Launch(LaunchArgs),
    /// List installed apps, or describe one
    Query(QueryArgs),
    /// Manage the host-native ART and Bionic runtime bundle
    Runtime(RuntimeArgs),
    /// Run the broker in the foreground (started by systemd)
    Daemon,
    /// Allocate a UID for an app. Privileged, invoked by the broker
    #[command(name = "uid-helper")]
    UidHelper(UidHelperArgs),
}

#[derive(ClapArgs, Debug, Clone)]
pub struct InstallArgs {
    /// Path to the APK
    pub apk: String,
    /// Skip the polkit prompt and pretend the user answered yes. For tests
    #[arg(long, hide = true)]
    pub yes: bool,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct UninstallArgs {
    pub package: String,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct LaunchArgs {
    pub package: String,
    /// Extra arguments passed to the app process
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct QueryArgs {
    /// Package name. Omit to list everything
    pub package: Option<String>,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct RuntimeArgs {
    #[command(subcommand)]
    pub subaction: RuntimeSubaction,
}

#[derive(Subcommand, Debug, Clone)]
pub enum RuntimeSubaction {
    /// Download and verify the runtime bundle
    Fetch,
    /// Show whether a bundle is installed and which version
    Status,
    /// Check that the installed bundle runs: start ART from it and report back
    Verify,
    /// Use a bundle built locally instead of downloading one
    Use {
        /// Directory holding a bundle, as tools/bundle/bundle.sh build produces
        directory: String,
        /// Version to record it under
        #[arg(long, default_value = "local")]
        version: String,
    },
}

#[derive(ClapArgs, Debug, Clone)]
pub struct UidHelperArgs {
    /// Package name to allocate a user for
    pub package: String,
    /// UID chosen by the broker, in the reserved range
    #[arg(long)]
    pub uid: Option<u32>,
    /// APK to place in the app's data directory
    #[arg(long)]
    pub apk: Option<String>,
    /// Remove the app's data directory and user instead of allocating
    #[arg(long)]
    pub remove: bool,
    /// Data directory (with --remove)
    #[arg(long)]
    pub data_dir: Option<String>,
}

/// The work directory and derived paths, resolved once at startup.
#[derive(Debug, Clone)]
pub struct MosaicArgs {
    pub cli: Cli,
    pub work: String,
    pub config: String,
    pub log: String,
    pub details_to_stdout: bool,
    pub verbose: bool,
    pub quiet: bool,
    pub timeout: u64,
}

impl MosaicArgs {
    pub fn from_cli(cli: Cli) -> Self {
        let work = cli
            .work
            .clone()
            .unwrap_or_else(|| crate::config::Defaults::new().work);
        let config = format!("{}/mosaic.cfg", work);
        let log_path = cli
            .log
            .clone()
            .unwrap_or_else(|| format!("{}/mosaic.log", work));
        let details_to_stdout = cli.details_to_stdout;
        let verbose = cli.verbose;
        let quiet = cli.quiet;
        let timeout = cli.timeout;
        Self {
            work,
            config,
            log: log_path,
            details_to_stdout,
            verbose,
            quiet,
            timeout,
            cli,
        }
    }

    pub fn action_name(&self) -> Option<String> {
        self.cli
            .action
            .as_ref()
            .map(|a| match a {
                Action::Install(_) => "install",
                Action::Uninstall(_) => "uninstall",
                Action::Launch(_) => "launch",
                Action::Query(_) => "query",
                Action::Runtime(_) => "runtime",
                Action::Daemon => "daemon",
                Action::UidHelper(_) => "uid-helper",
            })
            .map(|s| s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parse_install() {
        let cli = Cli::try_parse_from(["mosaic", "install", "/tmp/app.apk"]).unwrap();
        matches!(cli.action, Some(Action::Install(_)));
    }

    #[test]
    fn parse_launch_with_args() {
        let cli = Cli::try_parse_from(["mosaic", "launch", "com.termux", "--", "-e", "x"]).unwrap();
        if let Some(Action::Launch(a)) = cli.action {
            assert_eq!(a.package, "com.termux");
            assert!(a.args.contains(&"-e".to_string()));
        } else {
            panic!("expected launch");
        }
    }

    #[test]
    fn parse_daemon_and_helper() {
        assert!(matches!(
            Cli::try_parse_from(["mosaic", "daemon"]).unwrap().action,
            Some(Action::Daemon)
        ));
        assert!(matches!(
            Cli::try_parse_from(["mosaic", "uid-helper", "com.termux"])
                .unwrap()
                .action,
            Some(Action::UidHelper(_))
        ));
    }

    #[test]
    fn parse_runtime_status() {
        let cli = Cli::try_parse_from(["mosaic", "runtime", "status"]).unwrap();
        assert!(matches!(
            cli.action,
            Some(Action::Runtime(RuntimeArgs {
                subaction: RuntimeSubaction::Status
            }))
        ));
    }
}
