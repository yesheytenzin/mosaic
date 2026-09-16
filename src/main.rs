// SPDX-License-Identifier: GPL-3.0-or-later

use clap::Parser;
use std::path::Path;

use mosaic_lib::args::{Action, Cli, MosaicArgs};
use mosaic_lib::broker::{self, Request, Response};

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    std::process::exit(run().await);
}

async fn run() -> i32 {
    let cli = Cli::parse();
    let args = MosaicArgs::from_cli(cli);

    if nix::unistd::getuid().is_root() && !Path::new(&args.work).exists() {
        let _ = std::fs::create_dir_all(&args.work);
    }

    mosaic_lib::helpers::logging::init(
        args.verbose,
        args.quiet,
        &args.log,
        args.action_name().as_deref(),
        args.details_to_stdout,
    );

    let action = match args.cli.action.clone() {
        Some(action) => action,
        None => {
            let mut command = <Cli as clap::CommandFactory>::command();
            let _ = command.print_help();
            println!();
            return 0;
        }
    };

    let result = dispatch(&args, action).await;

    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("ERROR: {}", e);
            if args.details_to_stdout {
                eprintln!("{:#}", e);
            }
            1
        }
    }
}

async fn dispatch(args: &MosaicArgs, action: Action) -> anyhow::Result<()> {
    match action {
        Action::Install(install) => {
            let reserved = broker::request(args, &Request::Install { apk: install.apk }).await?;
            let package = match reserved {
                // Already installed: the broker did everything.
                Response::Installed(package) => return render(Response::Installed(package)),
                Response::Planned(package) => package,
                other => return render(other),
            };

            // The privileged step runs here, in the user's session, so polkit
            // has a session to prompt on. The broker is never root (ADR-0008).
            // `--yes` means "do not prompt": go through `sudo -n` instead.
            mosaic_lib::actions::uid_helper::invoke(
                args,
                &package.name,
                package.uid,
                Some(&package.apk),
                &package.data_dir,
                install.yes,
            )?;

            render(broker::request(args, &Request::Commit { package }).await?)
        }
        Action::Uninstall(uninstall) => {
            let response = broker::request(
                args,
                &Request::Uninstall {
                    package: uninstall.package,
                },
            )
            .await?;
            match response {
                Response::Uninstalled(package) => {
                    // Deleting the data directory needs root, which the broker
                    // does not have. A failure here leaves the package gone
                    // from the registry, so report it rather than hide it.
                    if let Err(e) = mosaic_lib::actions::uid_helper::invoke_remove(args, &package) {
                        eprintln!(
                            "WARNING: {} was uninstalled but {} was left behind: {}",
                            package.name, package.data_dir, e
                        );
                    }
                    render(Response::Uninstalled(package))
                }
                other => render(other),
            }
        }
        Action::Launch(launch) => render(
            broker::request(
                args,
                &Request::Launch {
                    package: launch.package,
                    args: launch.args,
                },
            )
            .await?,
        ),
        Action::Query(query) => {
            let package = query.package;
            let response = broker::request(
                args,
                &Request::Query {
                    package: package.clone(),
                },
            )
            .await?;
            render_query(package.as_deref(), response)
        }
        Action::Runtime(runtime) => {
            mosaic_lib::actions::runtime_cmd::dispatch(args, &runtime.subaction).await
        }
        Action::Daemon => broker::serve(args).await,
        Action::UidHelper(helper) => mosaic_lib::actions::uid_helper::run(args, &helper),
    }
}

fn render_query(package: Option<&str>, response: Response) -> anyhow::Result<()> {
    if let Response::Apps(apps) = &response {
        if apps.is_empty() {
            match package {
                Some(name) => println!("{} is not installed.", name),
                None => println!("No apps installed."),
            }
            return Ok(());
        }
    }
    render(response)
}

fn render(response: Response) -> anyhow::Result<()> {
    match response {
        Response::Pong => {}
        Response::Installed(package) => {
            println!(
                "Installed {} (uid {}, data {})",
                package.name, package.uid, package.data_dir
            )
        }
        Response::Planned(package) => {
            println!("Planned {} (uid {})", package.name, package.uid)
        }
        Response::Uninstalled(package) => {
            println!(
                "Removed {} (uid {}, data {})",
                package.name, package.uid, package.data_dir
            )
        }
        Response::Apps(apps) => {
            for package in apps {
                println!("{}\tuid {}\t{}", package.name, package.uid, package.apk);
            }
        }
        Response::Launched { package } => println!("Launched {}", package),
        Response::Error { message } => anyhow::bail!("{}", message),
    }
    Ok(())
}
