// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use crate::interfaces::gbinder::{serve, Reader, Writer};
use std::sync::atomic::AtomicBool;

const INTERFACE: &str = crate::guest::IFACE_USER_MONITOR;
const SERVICE_NAME: &str = crate::guest::SVC_USER_MONITOR;

const TRANSACTION_USER_UNLOCKED: u32 = 1;
const TRANSACTION_PACKAGE_STATE_CHANGED: u32 = 2;

pub const PACKAGE_ADDED: i32 = 0;
pub const PACKAGE_REMOVED: i32 = 1;
pub const PACKAGE_UPDATED: i32 = 2;

pub fn add_service<F1, F2>(
    args: &MosaicArgs,
    user_unlocked: F1,
    package_state_changed: F2,
    stop: &AtomicBool,
) where
    F1: Fn(i32) + Send + 'static,
    F2: Fn(i32, String, i32) + Send + 'static,
{
    let handler = move |mut reader: Reader, code: u32, _flags: u32, reply: &mut Writer| -> i32 {
        log::debug!("{}: Received transaction: {}", SERVICE_NAME, code);
        match code {
            TRANSACTION_USER_UNLOCKED => {
                if let Ok((_, arg1)) = reader.read_int32() {
                    user_unlocked(arg1);
                }
                reply.append_int32(0);
                0
            }
            TRANSACTION_PACKAGE_STATE_CHANGED => {
                let (_, mode) = reader.read_int32().unwrap_or((0, 0));
                let package = reader.read_string16().unwrap_or_default();
                let (_, uid) = reader.read_int32().unwrap_or((0, 0));
                package_state_changed(mode, package, uid);
                reply.append_int32(0);
                0
            }
            _ => -99999,
        }
    };
    serve(args, INTERFACE, SERVICE_NAME, handler, stop);
}
