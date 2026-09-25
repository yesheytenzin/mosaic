// SPDX-License-Identifier: GPL-3.0-or-later

//! The broker: one lazily started, idle-exiting unprivileged process that owns
//! the package registry, launches apps on demand, and routes userspace Binder
//! traffic (ADR-0005, ADR-0011). It is the only writer of the registry.

pub mod protocol;
pub mod registry;

pub use protocol::{read_frame, write_frame, Request, Response};
pub use registry::{Package, Registry};

use crate::args::MosaicArgs;
use std::os::unix::io::FromRawFd;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::net::{UnixListener, UnixStream};

/// How long the broker stays alive with nothing to do. It exits and systemd
/// starts it again on the next connection (ADR-0005).
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Where the broker binds when systemd is not handing it a socket. The user
/// runtime directory is preferred because the packaged broker is a systemd user
/// service and `$XDG_RUNTIME_DIR` is the one place both sides can reach.
pub fn bind_path(work: &str) -> String {
    if let Ok(path) = std::env::var("MOSAIC_SOCKET") {
        return path;
    }
    bind_path_in(work, user_runtime_dir().as_deref())
}

fn bind_path_in(work: &str, runtime: Option<&str>) -> String {
    match runtime {
        Some(runtime) => format!("{}/mosaic/broker.sock", runtime),
        None => format!("{}/broker.sock", work),
    }
}

fn user_runtime_dir() -> Option<String> {
    match std::env::var("XDG_RUNTIME_DIR") {
        Ok(dir) if !dir.is_empty() => Some(dir),
        _ => None,
    }
}

/// Every path a client should try, most likely first.
pub fn socket_candidates(work: &str) -> Vec<String> {
    let mut paths = vec![bind_path(work)];
    let fallback = format!("{}/broker.sock", work);
    if !paths.contains(&fallback) {
        paths.push(fallback);
    }
    paths
}

/// Run the broker until idle. systemd passes a listening socket on fd 3 for
/// socket activation; otherwise bind the path ourselves, which is what
/// `mosaic daemon` does when run by hand.
pub async fn serve(args: &MosaicArgs) -> anyhow::Result<()> {
    std::fs::create_dir_all(&args.work)?;
    let listener = match adopted_listener()? {
        Some(listener) => listener,
        None => {
            let socket = bind_path(&args.work);
            if let Some(parent) = Path::new(&socket).parent() {
                std::fs::create_dir_all(parent)?;
            }
            if Path::new(&socket).exists() {
                let _ = std::fs::remove_file(&socket);
            }
            UnixListener::bind(&socket)?
        }
    };
    log::info!("Broker listening on {}", bind_path(&args.work));

    let open = Arc::new(AtomicUsize::new(0));
    let binder = Arc::new(crate::binder::Transport::new());
    // The device services the framework asks for by name have to be here before
    // it asks: those lookups wait rather than fail, so a name nobody hosts is a
    // boot that waits.
    // What the device services need of this process's configuration: the runtime
    // bundle is where the framework's `/data` and `/system` live.
    //
    // `MOSAIC_ANDROID_ROOT` wins when it is set, because that is the tree the
    // framework is *actually* running from: a harness that launches the framework
    // against a bundle the broker does not know about would otherwise have services
    // answering about a different one -- the apex service listing the apexes of a
    // directory that is not the one the package manager scans, and so on.
    let root = match std::env::var("MOSAIC_ANDROID_ROOT") {
        Ok(dir) if !dir.is_empty() => dir,
        _ => crate::runtime::resolve(&args.work)
            .map(|(dir, _)| dir)
            .unwrap_or_default(),
    };
    crate::device::host_all(&binder, &root);
    monitor_idle(open.clone());
    loop {
        let (stream, _) = listener.accept().await?;
        open.fetch_add(1, Ordering::SeqCst);
        let args = args.clone();
        let open = open.clone();
        let binder = binder.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(&args, binder, stream).await {
                log::debug!("Broker request failed: {}", e);
            }
            open.fetch_sub(1, Ordering::SeqCst);
        });
    }
}

/// Exit once nothing has talked to the broker for `IDLE_TIMEOUT`. A launched
/// app (Phase 2) will hold it open the same way an in-flight request does.
fn monitor_idle(open: Arc<AtomicUsize>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(IDLE_TIMEOUT);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if open.load(Ordering::SeqCst) == 0 {
                log::info!("Broker has been idle, exiting");
                std::process::exit(0);
            }
        }
    });
}

fn adopted_listener() -> anyhow::Result<Option<UnixListener>> {
    let fds: i32 = std::env::var("LISTEN_FDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if fds < 1 {
        return Ok(None);
    }
    // SAFETY: systemd passes the first socket as fd 3 when LISTEN_FDS is set,
    // and we take ownership of it exactly once here.
    let listener = unsafe { std::os::unix::net::UnixListener::from_raw_fd(3) };
    listener.set_nonblocking(true)?;
    Ok(Some(UnixListener::from_std(listener)?))
}

async fn handle(
    args: &MosaicArgs,
    binder: Arc<crate::binder::Transport>,
    mut stream: UnixStream,
) -> anyhow::Result<()> {
    // Four bytes say which plane this connection is. The Binder magic read as a
    // length prefix would have been refused long before this, so the two cannot
    // be confused.
    let mut prefix = [0u8; 4];
    stream.read_exact(&mut prefix).await?;
    if crate::binder::is_binder_prefix(&prefix) {
        let stream = stream.into_std()?;
        stream.set_nonblocking(false)?;
        // The Binder plane is a conversation that blocks: a transaction waits
        // for its answer. It gets a thread of its own rather than the reactor.
        return tokio::task::spawn_blocking(move || binder.serve(&stream))
            .await
            .map_err(|e| anyhow::anyhow!("binder session panicked: {}", e))?;
    }

    let request: Request = crate::broker::protocol::read_frame_after(prefix, &mut stream).await?;
    let response = dispatch(args, request).await;
    write_frame(&mut stream, &response).await?;
    Ok(())
}

async fn dispatch(args: &MosaicArgs, request: Request) -> Response {
    match request {
        Request::Ping => Response::Pong,

        Request::Query { package } => {
            let registry = Registry::load(&args.work);
            match package {
                Some(name) => match registry.resolve(&name) {
                    Ok(p) => Response::Apps(vec![p.clone()]),
                    // A query that matches nothing is an answer, not a failure: it
                    // prints as an empty list rather than an error message.
                    Err(_) => Response::Apps(Vec::new()),
                },
                None => Response::Apps(registry.list()),
            }
        }

        Request::Install { apk } => match plan(args, &apk) {
            // Already installed: nothing to do, and no privileged step.
            Ok((package, true)) => Response::Installed(package),
            Ok((package, false)) => Response::Planned(package),
            Err(e) => Response::Error {
                message: e.to_string(),
            },
        },

        Request::Commit { package } => match commit(args, package) {
            Ok(package) => Response::Installed(package),
            Err(e) => Response::Error {
                message: e.to_string(),
            },
        },

        Request::Uninstall { package } => match uninstall(args, &package) {
            Ok(package) => Response::Uninstalled(package),
            Err(e) => Response::Error {
                message: e.to_string(),
            },
        },

        Request::Launch {
            package,
            args: extra,
        } => match launch(args, &package, extra).await {
            Ok(()) => Response::Launched { package },
            Err(e) => Response::Error {
                message: e.to_string(),
            },
        },
    }
}

/// Reserve the next UID and a data directory for an APK. The broker is the
/// allocation authority because it owns the registry, but it is unprivileged:
/// the caller creates the user and drives the polkit prompt (ADR-0008).
///
/// The flag is true when the package was already installed, in which case the
/// caller has nothing left to do.
fn plan(args: &MosaicArgs, apk: &str) -> anyhow::Result<(Package, bool)> {
    let apk_path =
        std::fs::canonicalize(apk).map_err(|e| anyhow::anyhow!("cannot read {}: {}", apk, e))?;
    let name = apk_package_name(&apk_path)?;

    let registry = Registry::load(&args.work);
    if let Some(existing) = registry.get(&name) {
        log::info!("{} is already installed at {}", name, existing.data_dir);
        return Ok((existing.clone(), true));
    }

    let (start, end) = crate::config::load(&args.config).uid_range();
    let uid = registry
        .next_uid(start, end)
        .ok_or_else(|| anyhow::anyhow!("no free UID in the reserved range {}-{}", start, end))?;

    let package = Package {
        name: name.clone(),
        uid,
        data_dir: format!("{}/apps/{}", args.work, name),
        apk: apk_path.to_string_lossy().to_string(),
        installed_at: now(),
    };
    Ok((package, false))
}

/// Make a reserved package permanent. Idempotent, so a retry after a lost
/// reply lands in the same state.
fn commit(args: &MosaicArgs, package: Package) -> anyhow::Result<Package> {
    let mut registry = Registry::load(&args.work);
    registry.add(package.clone());
    registry.save(&args.work)?;
    log::info!("Installed {} as uid {}", package.name, package.uid);
    Ok(package)
}

/// Remove a package from the registry and return it. Deleting its data
/// directory needs root, which the broker does not have; the caller does that.
fn uninstall(args: &MosaicArgs, name: &str) -> anyhow::Result<Package> {
    let mut registry = Registry::load(&args.work);
    let resolved = registry.resolve(name)?.name.clone();
    let package = registry
        .remove(&resolved)
        .ok_or_else(|| anyhow::anyhow!("{} is not installed", resolved))?;
    registry.save(&args.work)?;
    log::info!("Uninstalled {}", name);
    Ok(package)
}

async fn launch(args: &MosaicArgs, name: &str, extra: Vec<String>) -> anyhow::Result<()> {
    let registry = Registry::load(&args.work);
    // A selector, not necessarily the exact name: the name is derived from the file
    // and nobody wants to type it.
    let package = registry.resolve(name)?;
    let _runtime = crate::runtime::require(args)?;

    crate::binder::launch_app(args, package, &extra)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Send one request to the broker, starting it through systemd socket
/// activation if it is not already listening.
pub async fn request(args: &MosaicArgs, request: &Request) -> anyhow::Result<Response> {
    if let Some(mut stream) = connect_existing(&args.work).await {
        write_frame(&mut stream, request).await?;
        return read_frame(&mut stream).await;
    }

    // Nothing is listening. Ask systemd to activate the socket, then retry.
    let activation = activate();
    if let Ok(mut stream) = connect_retrying(&args.work).await {
        write_frame(&mut stream, request).await?;
        return read_frame(&mut stream).await;
    }

    let candidates = socket_candidates(&args.work).join(", ");
    match activation {
        Ok(()) => anyhow::bail!(
            "the broker did not start. Tried {}. Check 'systemctl --user status mosaic-broker.socket'",
            candidates
        ),
        Err(e) => anyhow::bail!(
            "the broker is not running and could not be started ({}). \
             Run 'mosaic daemon' in another terminal, or enable the socket with \
             'systemctl --user enable --now mosaic-broker.socket'",
            e
        ),
    }
}

/// Connect to the first candidate path that exists.
async fn connect_existing(work: &str) -> Option<UnixStream> {
    for path in socket_candidates(work) {
        if Path::new(&path).exists() {
            if let Ok(stream) = UnixStream::connect(&path).await {
                return Some(stream);
            }
        }
    }
    None
}

/// Give an activating socket unit a moment to accept.
async fn connect_retrying(work: &str) -> anyhow::Result<UnixStream> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if let Some(stream) = connect_existing(work).await {
            return Ok(stream);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("the broker socket did not become available")
}

fn activate() -> anyhow::Result<()> {
    let status = std::process::Command::new("systemctl")
        .args(["--user", "start", "mosaic-broker.socket"])
        .status();
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => anyhow::bail!("systemctl --user start mosaic-broker.socket failed: {}", s),
        Err(e) => anyhow::bail!("could not run systemctl: {}", e),
    }
}

/// Read a package name out of an APK's manifest.
///
/// Phase 2 owns real manifest parsing. Until then, derive a stable name from
/// the file so install and query are exercisable end to end.
pub fn apk_package_name(apk: &Path) -> anyhow::Result<String> {
    let stem = apk
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .ok_or_else(|| anyhow::anyhow!("cannot derive a package name from {}", apk.display()))?;
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' {
                c
            } else {
                '.'
            }
        })
        .collect();
    if cleaned.is_empty() {
        anyhow::bail!("cannot derive a package name from {}", apk.display());
    }
    Ok(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_candidates_always_include_the_work_dir() {
        let candidates = socket_candidates("/var/lib/mosaic");
        assert!(
            candidates.contains(&"/var/lib/mosaic/broker.sock".to_string()),
            "got {:?}",
            candidates
        );
    }

    /// Every user-facing command that names a package takes a selector, not only
    /// the exact derived name: launch, query and uninstall go through the same
    /// resolution, and `install` does not, because it is given a file.
    #[test]
    fn query_and_uninstall_take_a_selector_too() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().to_str().unwrap();
        let args = test_args(work);
        let apk = dir.path().join("org.example.hello.apk");
        std::fs::write(&apk, b"not a real apk").unwrap();
        let (planned, _) = plan(&args, apk.to_str().unwrap()).unwrap();
        commit(&args, planned).unwrap();

        let registry = Registry::load(work);
        assert!(registry.resolve("org.example").is_ok(), "a prefix");
        assert!(registry.resolve("hello").is_ok(), "a substring");
        assert!(
            registry.resolve("ORG.EXAMPLE").is_ok(),
            "case does not matter"
        );

        // Uninstalling by prefix removes the package it resolved to.
        let removed = uninstall(&args, "org.ex").unwrap();
        assert_eq!(removed.name, "org.example.hello");
        assert!(Registry::load(work).list().is_empty());
    }

    /// "not installed" on its own leaves nothing to try, because the name of an
    /// installed package is derived from its file and is not something a person
    /// would type. The message has to name what is there.
    #[tokio::test]
    async fn launching_something_absent_names_what_is_installed() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().to_str().unwrap();
        let args = test_args(work);

        let err = launch(&args, "nope", Vec::new()).await.unwrap_err();
        assert!(
            err.to_string().contains("nothing is"),
            "an empty registry should say so: {}",
            err
        );

        let apk = dir.path().join("org.example.hello.apk");
        std::fs::write(&apk, b"not a real apk").unwrap();
        let (planned, _) = plan(&args, apk.to_str().unwrap()).unwrap();
        commit(&args, planned.clone()).unwrap();

        let err = launch(&args, "nope", Vec::new()).await.unwrap_err();
        assert!(
            err.to_string().contains("org.example.hello"),
            "the message should name the installed package: {}",
            err
        );
    }

    #[test]
    fn apk_package_name_sanitizes_separators() {
        let name = apk_package_name(Path::new("/tmp/My App-1.2!.apk")).unwrap();
        assert!(!name.contains('/') && !name.contains(' '));
        assert!(name.starts_with("My"));
    }

    /// The socket unit and the code must agree on the path, or the CLI connects
    /// to nothing while the daemon listens somewhere else.
    #[test]
    fn systemd_socket_matches_the_bound_path() {
        let unit = std::fs::read_to_string("systemd/mosaic-broker.socket").unwrap();
        let listen = unit
            .lines()
            .find_map(|line| line.trim().strip_prefix("ListenStream="))
            .expect("the socket unit must declare a ListenStream");
        let runtime = "/run/user/1000";
        assert_eq!(
            listen.trim().replace("%t", runtime),
            bind_path_in("/var/lib/mosaic", Some(runtime))
        );
    }

    /// A6: the framework's priority calls land at `setpriority`, which
    /// `RLIMIT_NICE` caps, and a user service can only use a limit its manager
    /// already has. The grant therefore lives system side and the broker repeats
    /// it; if the two disagree the service silently keeps the default and every
    /// audio, display and binder priority call keeps failing.
    #[test]
    fn the_priority_limit_is_granted_system_side_and_used_by_the_broker() {
        fn limit(text: &str) -> u32 {
            text.lines()
                .find_map(|line| line.trim().strip_prefix("LimitNICE="))
                .expect("a priority limit")
                .trim()
                .parse()
                .expect("a number")
        }

        let unit = std::fs::read_to_string("systemd/mosaic-broker.service").unwrap();
        let drop_in =
            std::fs::read_to_string("packaging/arch/user@.service.d/mosaic.conf").unwrap();
        assert_eq!(
            limit(&unit),
            limit(&drop_in),
            "the grant and the service that uses it must agree"
        );
        // The framework asks for niceness as low as -20, which needs 40.
        assert!(
            limit(&unit) >= 40,
            "a lower limit leaves the framework's priority calls failing"
        );
    }

    /// The one path root has to create is the one Bionic compiles in, so it
    /// cannot be redirected away.
    #[test]
    fn the_provisioned_paths_are_the_ones_that_cannot_be_redirected() {
        let tmpfiles = std::fs::read_to_string("packaging/arch/mosaic.tmpfiles").unwrap();
        assert!(
            tmpfiles.contains("/dev/__properties__"),
            "libc's property area path is hardcoded and has to exist"
        );
    }

    fn test_args(work: &str) -> MosaicArgs {
        use clap::Parser;
        let cli = crate::args::Cli::try_parse_from(["mosaic", "-w", work, "query"]).unwrap();
        crate::args::MosaicArgs::from_cli(cli)
    }

    /// Install is a two step transaction so the broker never needs root: the
    /// broker reserves, the caller does the privileged work, the broker
    /// commits. The reservation must not be visible as an install.
    #[test]
    fn plan_commit_uninstall_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().to_str().unwrap();
        let apk = dir.path().join("org.example.hello.apk");
        std::fs::write(&apk, b"not a real apk").unwrap();
        let args = test_args(work);

        let (planned, already) = plan(&args, apk.to_str().unwrap()).unwrap();
        assert!(!already);
        assert_eq!(planned.name, "org.example.hello");
        assert_eq!(planned.uid, 5000);
        assert_eq!(planned.data_dir, format!("{}/apps/org.example.hello", work));
        assert!(
            Registry::load(work).list().is_empty(),
            "a reservation must not appear as an installed package"
        );

        commit(&args, planned.clone()).unwrap();
        assert_eq!(
            Registry::load(work).get("org.example.hello").unwrap().uid,
            5000
        );

        // Committing twice is the same as committing once.
        commit(&args, planned.clone()).unwrap();
        assert_eq!(Registry::load(work).list().len(), 1);

        // A second package gets the next UID.
        let second = dir.path().join("org.example.other.apk");
        std::fs::write(&second, b"not a real apk either").unwrap();
        let (planned, already) = plan(&args, second.to_str().unwrap()).unwrap();
        assert!(!already);
        assert_eq!(planned.uid, 5001);

        // Planning an installed package reports it as already installed.
        let (again, already) = plan(&args, apk.to_str().unwrap()).unwrap();
        assert!(already);
        assert_eq!(again.uid, 5000);

        let removed = uninstall(&args, "org.example.hello").unwrap();
        assert_eq!(removed.uid, 5000);
        assert_eq!(Registry::load(work).list().len(), 0);
    }
}
