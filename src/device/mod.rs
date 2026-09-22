// SPDX-License-Identifier: GPL-3.0-or-later

//! The device layer: the services an Android device runs *outside* the framework,
//! which the framework nonetheless expects to find by name.
//!
//! These are the ones whose absence is fatal rather than degrading. The framework
//! looks them up through the service manager, so hosting them in the broker is
//! what makes them reachable from every process (ADR-0005): a name resolves to a
//! handle in the asking process's table, and a transaction on it is forwarded here.
//!
//! A desktop is not a device, so each of these answers rather than does. What
//! matters is that the name resolves and the call comes back, because the JNI that
//! asks is written to abort when it does not -- `getSuspendControl()` in
//! `libandroid_servers` is `LOG_ALWAYS_FATAL_IF(gSuspendControl == nullptr)`.
//! What each one actually does is stated where it is implemented.

pub mod installd;
pub mod surfaceflinger;
pub mod suspend;

use crate::binder::Transport;

/// Publish every device service the framework asks for by name.
///
/// Called once, when the broker starts: a name that resolves has to be there
/// before the framework looks it up, because the lookups block
/// (`ServiceManagerShim::waitForService` retries once a second) and a boot that
/// waits for a service nobody hosts simply waits.
pub fn host_all(binder: &Transport, root: &str) {
    binder.host(suspend::CONTROL_NAME, Box::new(suspend::SuspendControl));
    binder.host(
        suspend::CONTROL_INTERNAL_NAME,
        Box::new(suspend::SuspendControlInternal),
    );
    binder.host(
        suspend::HAL_NAME,
        Box::new(suspend::SystemSuspend::default()),
    );
    // The package manager waits for this one, and asks it about every package it
    // scans: `Installer: installd not found; trying again` repeats until it exists.
    binder.host(installd::NAME, Box::new(installd::Installd::new(root)));
    // The system server waits for this one before it starts anything else
    // (`DisplayManagerService`), the way it waits for `installd`.
    binder.host(
        surfaceflinger::NAME,
        Box::new(surfaceflinger::SurfaceFlinger::reporting(
            surfaceflinger::LEGACY,
        )),
    );
    binder.host(
        surfaceflinger::AIDL_NAME,
        Box::new(surfaceflinger::SurfaceFlinger::reporting(
            surfaceflinger::AIDL,
        )),
    );
}
