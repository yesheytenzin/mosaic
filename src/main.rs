// SPDX-License-Identifier: GPL-3.0-or-later

use clap::Parser;
use std::path::Path;

use mosaic_lib::args::{
    Action, AdbSubaction, AppSubaction, Cli, ContainerSubaction, MosaicArgs, PropSubaction,
    SessionSubaction,
};

#[tokio::main]
async fn main() {
    let exit_code = run().await;
    std::process::exit(exit_code);
}

async fn run() -> i32 {
    let cli = Cli::parse();
    let mut args = MosaicArgs::from_cli(cli);

    if nix::unistd::getuid().as_raw() == 0 {
        if !Path::new(&args.work).exists() {
            let _ = std::fs::create_dir_all(&args.work);
        }
    } else if !Path::new(&args.log).exists() {
        args.log = "/tmp/mosaic.log".to_string();
    }

    let action_name = args.action_name();
    let action_for_log = action_name.clone();

    mosaic_lib::helpers::logging::init(
        args.verbose,
        args.quiet,
        &args.log,
        action_for_log.as_deref(),
        args.details_to_stdout,
    );

    let action = args.cli.action.clone().unwrap_or(Action::FirstLaunch);

    let is_initialized = mosaic_lib::actions::initializer::is_initialized(&args);
    let action_str = match &action {
        Action::Status => "status",
        Action::Log(_) => "log",
        Action::Init(_) => "init",
        Action::Upgrade(_) => "upgrade",
        Action::Session(_) => "session",
        Action::Container(_) => "container",
        Action::App(_) => "app",
        Action::Prop(_) => "prop",
        Action::ShowFullUi => "show-full-ui",
        Action::FirstLaunch => "first-launch",
        Action::Shell(_) => "shell",
        Action::Logcat(_) => "logcat",
        Action::Adb(_) => "adb",
        Action::Bugreport => "bugreport",
    };
    let allowed_uninitialized = ["init", "container", "first-launch", "log", "bugreport"];
    if !is_initialized && !allowed_uninitialized.contains(&action_str) {
        println!("Mosaic is not initialized, run \"mosaic init\"");
        return 0;
    }

    let result: anyhow::Result<()> = match action {
        Action::Status => mosaic_lib::actions::status::print_status(&args).await,
        Action::Log(log_args) => {
            if log_args.clear_log {
                let _ = std::process::Command::new("truncate")
                    .args(["-s", "0", &args.log])
                    .status();
                Ok(())
            } else {
                let mut cmd = std::process::Command::new("tail");
                cmd.args(["-n", &log_args.lines, "-F", &args.log]);
                match cmd.status() {
                    Ok(_) => Ok(()),
                    Err(e) => {
                        if e.kind() == std::io::ErrorKind::Interrupted {
                            Ok(())
                        } else {
                            Err(e.into())
                        }
                    }
                }
            }
        }
        Action::Init(init_args) => {
            if init_args.client {
                mosaic_lib::actions::initializer::remote_init_client(&args).await
            } else {
                if nix::unistd::getuid().as_raw() != 0 {
                    eprintln!("ERROR: Action \"init\" needs root access");
                    return 1;
                }
                mosaic_lib::actions::initializer::init(
                    &mut args,
                    init_args.force,
                    init_args.images_path,
                    init_args.system_channel,
                    init_args.vendor_channel,
                    init_args.rom_type,
                    init_args.system_type,
                    None,
                )
            }
        }
        Action::Upgrade(up_args) => {
            if nix::unistd::getuid().as_raw() != 0 {
                eprintln!("ERROR: Action \"upgrade\" needs root access");
                return 1;
            }
            mosaic_lib::actions::upgrader::upgrade(&args, up_args.offline)
        }
        Action::Session(sess_args) => match sess_args.subaction {
            Some(SessionSubaction::Start) => {
                mosaic_lib::actions::session_manager::start(args.clone()).await
            }
            Some(SessionSubaction::Stop) => mosaic_lib::actions::session_manager::stop(&args),
            None => {
                log::info!("Run mosaic session -h for usage information.");
                Ok(())
            }
        },
        Action::Container(cont_args) => {
            if nix::unistd::getuid().as_raw() != 0 {
                eprintln!("ERROR: Action \"container\" needs root access");
                return 1;
            }
            match cont_args.subaction {
                Some(ContainerSubaction::Start) => {
                    mosaic_lib::actions::container_manager::run_container_service(args.clone())
                        .await
                }
                Some(ContainerSubaction::Stop) => {
                    mosaic_lib::actions::container_manager::stop(&args, true, None)
                }
                Some(ContainerSubaction::Restart) => {
                    let _ = mosaic_lib::actions::container_manager::stop(&args, false, None);
                    mosaic_lib::actions::container_manager::run_container_service(args.clone())
                        .await
                }
                Some(ContainerSubaction::Freeze) => {
                    mosaic_lib::actions::container_manager::freeze(&args)
                }
                Some(ContainerSubaction::Unfreeze) => {
                    mosaic_lib::actions::container_manager::unfreeze(&args)
                }
                None => {
                    log::info!("Run mosaic container -h for usage information.");
                    Ok(())
                }
            }
        }
        Action::App(app_args) => match app_args.subaction {
            Some(AppSubaction::Install { package }) => {
                mosaic_lib::actions::app_manager::install(&args, &package).await
            }
            Some(AppSubaction::Remove { package }) => {
                mosaic_lib::actions::app_manager::remove(&args, &package).await
            }
            Some(AppSubaction::Launch { package }) => {
                mosaic_lib::actions::app_manager::launch(&args, &package).await
            }
            Some(AppSubaction::Intent { action, uri }) => {
                mosaic_lib::actions::app_manager::intent(&args, &action, &uri).await
            }
            Some(AppSubaction::List) => mosaic_lib::actions::app_manager::list(&args).await,
            None => {
                log::info!("Run mosaic app -h for usage information.");
                Ok(())
            }
        },
        Action::Prop(prop_args) => match prop_args.subaction {
            Some(PropSubaction::Get { key }) => mosaic_lib::actions::prop::get(&args, &key).await,
            Some(PropSubaction::Set { key, value }) => {
                mosaic_lib::actions::prop::set(&args, &key, &value).await
            }
            None => {
                log::info!("Run mosaic prop -h for usage information.");
                Ok(())
            }
        },
        Action::Shell(shell_args) => {
            if nix::unistd::getuid().as_raw() != 0 {
                eprintln!("ERROR: Action \"shell\" needs root access");
                return 1;
            }
            mosaic_lib::helpers::lxc::shell(
                &args,
                shell_args.uid.as_deref(),
                shell_args.gid.as_deref(),
                shell_args.context.as_deref(),
                shell_args.nolsm,
                shell_args.allcaps,
                shell_args.nocgroup,
                &shell_args.command,
            )
        }
        Action::Logcat(logcat_args) => {
            if nix::unistd::getuid().as_raw() != 0 {
                eprintln!("ERROR: Action \"logcat\" needs root access");
                return 1;
            }
            mosaic_lib::helpers::lxc::logcat(&args, &logcat_args.args)
        }
        Action::ShowFullUi => mosaic_lib::actions::app_manager::show_full_ui(&args).await,
        Action::FirstLaunch => {
            if !mosaic_lib::actions::initializer::is_initialized(&args) {
                let _ = mosaic_lib::actions::initializer::remote_init_client(&args).await;
            }
            if mosaic_lib::actions::initializer::is_initialized(&args) {
                mosaic_lib::actions::app_manager::show_full_ui(&args).await
            } else {
                Ok(())
            }
        }
        Action::Adb(adb_args) => match adb_args.subaction {
            Some(AdbSubaction::Connect) => mosaic_lib::helpers::net::adb_connect(&args),
            Some(AdbSubaction::Disconnect) => mosaic_lib::helpers::net::adb_disconnect(&args),
            None => {
                log::info!("Run mosaic adb -h for usage information.");
                Ok(())
            }
        },
        Action::Bugreport => mosaic_lib::actions::bugreport::bugreport(&args),
    };

    match result {
        Ok(_) => 0,
        Err(e) => {
            log::info!("ERROR: {}", e);
            log::info!("See also: <https://github.com/yesheytenzin/mosaic>");
            log::debug!("{:?}", e);
            if args.details_to_stdout {
                return 1;
            }
            let log_hint =
                if !Path::new(&args.log).exists() || action_str == "container" {
                    format!(
                    "Use '--details-to-stdout' to get more details:\n  {} --details-to-stdout {}",
                    std::env::args().next().unwrap_or_else(|| "mosaic".to_string()),
                    std::env::args().skip(1).collect::<Vec<_>>().join(" ")
                )
                } else {
                    "Run 'mosaic log' for details.".to_string()
                };
            println!("{}", log_hint);
            1
        }
    }
}
