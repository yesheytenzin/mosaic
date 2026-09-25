// SPDX-License-Identifier: GPL-3.0-or-later

//! `SystemSuspend`, in userspace.
//!
//! On a device this is a native daemon that owns the kernel's suspend machinery:
//! it takes wake locks, decides when the machine may sleep, and wakes it. The
//! framework reaches it three ways, and all three are hard requirements rather
//! than conveniences:
//!
//! | Name | Interface | Who asks |
//! | --- | --- | --- |
//! | `suspend_control` | `android.system.suspend.ISuspendControlService` | `BatteryStatsService.nativeWaitWakeup`, which registers a wakeup callback |
//! | `suspend_control_internal` | `android.system.suspend.internal.ISuspendControlServiceInternal` | `PowerManagerService`'s JNI: autosuspend on and off, force suspend, the statistics |
//! | `android.system.suspend.ISystemSuspend/default` | `android.system.suspend.ISystemSuspend` | the same JNI, for the wake lock that keeps the machine up |
//!
//! The first is the one that stops the boot. `getSuspendControl()` in
//! `com_android_server_power_PowerManagerService.cpp` is
//!
//! ```text
//! gSuspendControl = waitForService<ISuspendControlService>(String16("suspend_control"));
//! LOG_ALWAYS_FATAL_IF(gSuspendControl == nullptr);
//! ```
//!
//! so a lookup that finds nothing is a SIGABRT in the system server, which is
//! exactly where `SystemServer` stopped before this existed.
//!
//! What these do on a host with no kernel suspend: nothing, on purpose. Nothing
//! here can put a desktop to sleep, and nothing should: the machine's own power
//! management is the user's business. A wake lock is therefore granted and
//! ignored, autosuspend is reported as enabled, and a forced suspend reports that
//! it did not happen -- the one answer that is not a lie.

use crate::binder::parcel::Parcel;
use crate::binder::{Answer, BinderObject, Handed};
use anyhow::Result;
use parking_lot::Mutex;
use std::sync::Arc;

/// `android.system.suspend.ISuspendControlService`, in declaration order.
mod control {
    pub const REGISTER_CALLBACK: u32 = 1;
    pub const REGISTER_WAKELOCK_CALLBACK: u32 = 2;
}

/// `android.system.suspend.internal.ISuspendControlServiceInternal`.
mod internal {
    pub const ENABLE_AUTOSUSPEND: u32 = 1;
    pub const FORCE_SUSPEND: u32 = 2;
    pub const GET_WAKELOCK_STATS: u32 = 3;
    pub const GET_WAKEUP_STATS: u32 = 4;
    pub const GET_SUSPEND_STATS: u32 = 5;
}

/// `android.system.suspend.ISystemSuspend`.
mod hal {
    pub const ACQUIRE_WAKE_LOCK: u32 = 1;
}

/// `android.system.suspend.IWakeLock`: one method, and it is oneway.
mod wakelock {
    pub const RELEASE: u32 = 1;
}

/// The interface names as they appear in a request's interface token.
pub const SUSPEND_CONTROL: &str = "android.system.suspend.ISuspendControlService";
pub const SUSPEND_CONTROL_INTERNAL: &str =
    "android.system.suspend.internal.ISuspendControlServiceInternal";
pub const SYSTEM_SUSPEND: &str = "android.system.suspend.ISystemSuspend";
pub const WAKE_LOCK: &str = "android.system.suspend.IWakeLock";

/// The names these are published under, which is how the framework asks for them.
pub const CONTROL_NAME: &str = "suspend_control";
pub const CONTROL_INTERNAL_NAME: &str = "suspend_control_internal";
pub const HAL_NAME: &str = "android.system.suspend.ISystemSuspend/default";

/// The fields of `internal.SuspendInfo`, all of them `long`.
const SUSPEND_INFO_FIELDS: usize = 10;

/// `Status::EX_UNSUPPORTED_OPERATION`, as an AIDL exception code.
const EX_UNSUPPORTED_OPERATION: i32 = -7;

/// `suspend_control`: the wakeup callback the battery statistics thread registers.
///
/// The callback is accepted and never called. That is the truth of it: nothing on
/// this machine can wake it from suspend, because nothing here suspends it. The
/// thread that registers waits on a semaphore afterwards, which is where it stays.
#[derive(Default)]
pub struct SuspendControl;

impl BinderObject for SuspendControl {
    fn descriptor(&self) -> &str {
        SUSPEND_CONTROL
    }

    fn transact(&mut self, code: u32, _data: &[u8]) -> Result<Answer> {
        let mut reply = Parcel::new();
        match code {
            control::REGISTER_CALLBACK | control::REGISTER_WAKELOCK_CALLBACK => {
                reply.boolean(true);
            }
            _ => reply.boolean(false),
        }
        Ok(reply.into_bytes().into())
    }
}

/// `suspend_control_internal`: what `PowerManagerService` drives the machine with.
#[derive(Default)]
pub struct SuspendControlInternal;

impl BinderObject for SuspendControlInternal {
    fn descriptor(&self) -> &str {
        SUSPEND_CONTROL_INTERNAL
    }

    fn transact(&mut self, code: u32, _data: &[u8]) -> Result<Answer> {
        let mut reply = Parcel::new();
        match code {
            // The token the caller passes is a binder it wants to hear from when
            // autosuspend changes state. Accepted, and nothing is ever sent: the
            // host's own power management is not ours to report on.
            internal::ENABLE_AUTOSUSPEND => reply.boolean(true),
            // The one honest no. A forced suspend is a debug command, and this
            // machine did not suspend.
            internal::FORCE_SUSPEND => reply.boolean(false),
            // No wake locks are held here, so there is nothing to report: an empty
            // array is the shape, and a count of zero says it.
            internal::GET_WAKELOCK_STATS | internal::GET_WAKEUP_STATS => reply.empty_array(),
            internal::GET_SUSPEND_STATS => {
                reply.parcelable_present();
                for _ in 0..SUSPEND_INFO_FIELDS {
                    reply.i64(0);
                }
            }
            _ => reply.boolean(false),
        }
        Ok(reply.into_bytes().into())
    }
}

/// `android.system.suspend.IWakeLock`: the object `acquireWakeLock` hands back.
///
/// On a device this is what keeps the machine from suspending while the framework
/// still has work to finish. On a host there is nothing here that suspends the
/// machine, so the lock holds nothing -- but it has to *exist*: the framework's
/// `disableAutoSuspend` asserts the lock it asked for is not null, and releases it
/// when the screen comes back on. A null answer is a crash waiting for the first
/// caller; this is an object with the one method the interface has, and it counts
/// what it was asked to do so a caller can tell the difference.
pub struct WakeLock {
    released: Arc<Mutex<u32>>,
}

impl BinderObject for WakeLock {
    fn descriptor(&self) -> &str {
        WAKE_LOCK
    }

    fn transact(&mut self, code: u32, _data: &[u8]) -> Result<Answer> {
        // `release` is oneway: the caller does not wait for, or read, an answer.
        if code == wakelock::RELEASE {
            *self.released.lock() += 1;
            return Ok(Answer::default());
        }
        let mut reply = Parcel::new();
        reply.i32(EX_UNSUPPORTED_OPERATION);
        reply.string16(&format!("IWakeLock method {} is not implemented", code));
        reply.i32(0); // the remote stack trace header the reader expects
        Ok(reply.into_bytes().into())
    }
}

/// `android.system.suspend.ISystemSuspend/default`: the wake lock HAL.
///
/// `acquireWakeLock` returns an `IWakeLock` that the caller holds until it
/// releases it. Nothing on the boot path takes one -- the lock is taken when the
/// screen turns off and released when it turns on -- which is why a null object
/// went unnoticed for as long as it did, and why a real one is worth having before
/// something asks.
#[derive(Default)]
pub struct SystemSuspend {
    released: Arc<Mutex<u32>>,
}

impl SystemSuspend {
    /// How many wake locks this HAL handed out have been released.
    pub fn released(&self) -> u32 {
        *self.released.lock()
    }
}

impl BinderObject for SystemSuspend {
    fn descriptor(&self) -> &str {
        SYSTEM_SUSPEND
    }

    fn transact(&mut self, code: u32, _data: &[u8]) -> Result<Answer> {
        let mut reply = Parcel::new();
        if code != hal::ACQUIRE_WAKE_LOCK {
            reply.boolean(false);
            return Ok(reply.into_bytes().into());
        }
        // The status is already written; then the word the object goes in, where
        // the broker writes the caller's handle and hosts the lock it will call.
        let offset = reply.handle_binder(0);
        Ok(Answer::from(reply.into_bytes()).handing(
            offset,
            Handed::fresh(Box::new(WakeLock {
                released: Arc::clone(&self.released),
            })),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binder::parcel::args_after;

    /// A request as the shim forwards it: the AIDL header, the interface token as
    /// a string16, then the arguments.
    fn request(descriptor: &str, args: &[u8]) -> Vec<u8> {
        let mut data = vec![0x00, 0x00, 0x00, 0x80, 0xff, 0xff, 0xff, 0xff];
        data.extend_from_slice(b"TSYS");
        let count = (descriptor.len() + 1) as i32; // the token includes its NUL
        data.extend_from_slice(&count.to_le_bytes());
        for unit in descriptor.encode_utf16() {
            data.extend_from_slice(&unit.to_le_bytes());
        }
        data.extend_from_slice(&[0, 0]); // the NUL
        while data.len() % 4 != 0 {
            data.push(0); // and the padding to four bytes
        }
        data.extend_from_slice(args);
        data
    }

    #[test]
    fn arguments_are_found_after_the_token() {
        let data = request(SUSPEND_CONTROL, &[7, 0, 0, 0]);
        assert_eq!(args_after(&data, SUSPEND_CONTROL), &[7, 0, 0, 0]);
        // An interface this request is not for yields no arguments rather than the
        // wrong ones.
        assert_eq!(args_after(&data, SYSTEM_SUSPEND), &[] as &[u8]);
    }

    #[test]
    fn register_callback_answers_that_it_is_registered() {
        let mut service = SuspendControl;
        let data = request(SUSPEND_CONTROL, &[0; 28]);
        let reply = service
            .transact(control::REGISTER_CALLBACK, &data)
            .unwrap()
            .data;
        // Status 0, then true. The framework's `isRegistered` is read from here,
        // and a false makes it log that the callback could not be registered.
        assert_eq!(reply, vec![1, 0, 0, 0], "the value alone");
    }

    #[test]
    fn autosuspend_is_reported_enabled_and_a_forced_suspend_is_not_claimed() {
        let mut service = SuspendControlInternal;
        let enabled = service
            .transact(internal::ENABLE_AUTOSUSPEND, &[])
            .unwrap()
            .data;
        assert_eq!(enabled, vec![1, 0, 0, 0], "the value alone");
        let forced = service.transact(internal::FORCE_SUSPEND, &[]).unwrap().data;
        assert_eq!(
            forced,
            vec![0, 0, 0, 0],
            "four bytes, and no status in front"
        );
    }

    #[test]
    fn statistics_are_empty_rather_than_absent() {
        let mut service = SuspendControlInternal;
        // An array is a count: zero of them, after the status.
        let locks = service
            .transact(internal::GET_WAKELOCK_STATS, &[])
            .unwrap()
            .data;
        assert_eq!(locks, vec![0, 0, 0, 0], "the value alone");
        // A parcelable is the AIDL present-flag and then its fields, so a reader
        // gets ten zeros rather than reading past the end of the parcel.
        let stats = service
            .transact(internal::GET_SUSPEND_STATS, &[])
            .unwrap()
            .data;
        assert_eq!(stats.len(), 4 + SUSPEND_INFO_FIELDS * 8);
        assert_eq!(&stats[..4], &[1, 0, 0, 0], "the value alone");
        assert!(stats[8..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn a_wake_lock_is_handed_back_as_an_object() {
        let mut service = SystemSuspend::default();
        let answer = service.transact(hal::ACQUIRE_WAKE_LOCK, &[]).unwrap();
        // A transaction-sized object word: a handle, which the side holding the
        // caller's table fills in, then the stability word -- and no status in front,
        // because the shim writes that.
        assert_eq!(answer.data.len(), 28, "24 bytes and the stability word");
        assert_eq!(answer.objects.len(), 1);
        assert_eq!(answer.objects[0].offset, 0);
        assert_eq!(
            &answer.data[..4],
            &crate::binder::parcel::BINDER_TYPE_HANDLE.to_le_bytes()
        );
        // And the object is the lock itself, which the framework calls `release`
        // on when the screen comes back on.
        let Handed::New { object: lock, .. } = &answer.objects[0].object else {
            panic!("the wake lock has to be an object the broker has not seen yet")
        };
        assert_eq!(lock.descriptor(), WAKE_LOCK);
    }

    #[test]
    fn releasing_a_wake_lock_is_counted_and_answered_with_nothing() {
        let mut service = SystemSuspend::default();
        let answer = service.transact(hal::ACQUIRE_WAKE_LOCK, &[]).unwrap();
        let Handed::New {
            object: mut lock, ..
        } = answer.objects.into_iter().next().unwrap().object
        else {
            panic!("expected a lock")
        };
        // `release` is oneway: an empty answer is the whole of it.
        let reply = lock.transact(wakelock::RELEASE, &[]).unwrap();
        assert!(reply.data.is_empty());
        assert_eq!(service.released(), 1);
        // A method this interface does not have is refused, by name.
        let reply = lock.transact(9, &[]).unwrap().data;
        assert_eq!(&reply[..4], &EX_UNSUPPORTED_OPERATION.to_le_bytes());
    }
}
