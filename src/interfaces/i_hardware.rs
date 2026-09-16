// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use crate::interfaces::gbinder::{serve, Reader, Writer};
use std::sync::atomic::AtomicBool;

const INTERFACE: &str = crate::guest::IFACE_HARDWARE;
const SERVICE_NAME: &str = crate::guest::SVC_HARDWARE;

const TRANSACTION_ENABLE_NFC: u32 = 1;
const TRANSACTION_ENABLE_BLUETOOTH: u32 = 2;
const TRANSACTION_SUSPEND: u32 = 3;
const TRANSACTION_REBOOT: u32 = 4;
const TRANSACTION_UPGRADE: u32 = 5;
const TRANSACTION_UPGRADE2: u32 = 6;
const TRANSACTION_SHUTDOWN_REQUEST: u32 = 7;

#[allow(clippy::too_many_arguments)]
pub fn add_service<F1, F2, F3, F4, F5, F6>(
    args: &MosaicArgs,
    enable_nfc: F1,
    enable_bluetooth: F2,
    suspend: F3,
    reboot: F4,
    upgrade: F5,
    shutdown_request: F6,
    stop: &AtomicBool,
) where
    F1: Fn(bool) -> i32 + Send + 'static,
    F2: Fn(bool) -> i32 + Send + 'static,
    F3: Fn() + Send + 'static,
    F4: Fn() + Send + 'static,
    F5: Fn(String, i64, String, i64) + Send + 'static,
    F6: Fn(String) + Send + 'static,
{
    let handler = move |mut reader: Reader, code: u32, _flags: u32, reply: &mut Writer| -> i32 {
        log::debug!("{}: Received transaction: {}", SERVICE_NAME, code);
        match code {
            TRANSACTION_ENABLE_NFC => {
                let (_, arg1) = reader.read_int32().unwrap_or((0, 0));
                let ret = enable_nfc(arg1 != 0);
                reply.append_int32(0);
                reply.append_int32(ret);
                0
            }
            TRANSACTION_ENABLE_BLUETOOTH => {
                let (_, arg1) = reader.read_int32().unwrap_or((0, 0));
                let ret = enable_bluetooth(arg1 != 0);
                reply.append_int32(0);
                reply.append_int32(ret);
                0
            }
            TRANSACTION_SUSPEND => {
                suspend();
                reply.append_int32(0);
                0
            }
            TRANSACTION_REBOOT => {
                reboot();
                reply.append_int32(0);
                0
            }
            TRANSACTION_UPGRADE => {
                let archive = reader.read_string16().unwrap_or_default();
                let (_, time) = reader.read_int32().unwrap_or((0, 0));
                let vendor = reader.read_string16().unwrap_or_default();
                let (_, vendor_time) = reader.read_int32().unwrap_or((0, 0));
                upgrade(archive, time as i64, vendor, vendor_time as i64);
                reply.append_int32(0);
                0
            }
            TRANSACTION_UPGRADE2 => {
                let archive = reader.read_string16().unwrap_or_default();
                let (_, time) = reader.read_int64().unwrap_or((0, 0));
                let vendor = reader.read_string16().unwrap_or_default();
                let (_, vendor_time) = reader.read_int64().unwrap_or((0, 0));
                upgrade(archive, time, vendor, vendor_time);
                reply.append_int32(0);
                0
            }
            TRANSACTION_SHUTDOWN_REQUEST => {
                let reason = reader.read_string16().unwrap_or_default();
                shutdown_request(reason);
                reply.append_int32(0);
                0
            }
            _ => -99999,
        }
    };
    serve(args, INTERFACE, SERVICE_NAME, handler, stop);
}
