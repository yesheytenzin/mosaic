// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use crate::interfaces::gbinder::{serve, Reader, Writer};
use std::sync::atomic::AtomicBool;

const INTERFACE: &str = crate::guest::IFACE_CLIPBOARD;
const SERVICE_NAME: &str = crate::guest::SVC_CLIPBOARD;

const TRANSACTION_SEND_CLIPBOARD_DATA: u32 = 1;
const TRANSACTION_GET_CLIPBOARD_DATA: u32 = 2;

pub fn add_service<F1, F2>(
    args: &MosaicArgs,
    send_clipboard: F1,
    get_clipboard: F2,
    stop: &AtomicBool,
) where
    F1: Fn(String) + Send + 'static,
    F2: Fn() -> String + Send + 'static,
{
    let handler = move |mut reader: Reader, code: u32, _flags: u32, reply: &mut Writer| -> i32 {
        log::debug!("{}: Received transaction: {}", SERVICE_NAME, code);
        match code {
            TRANSACTION_SEND_CLIPBOARD_DATA => {
                match reader.read_string16() {
                    Ok(arg1) => send_clipboard(arg1),
                    Err(e) => log::debug!("Failed to read clipboard data: {}", e),
                }
                reply.append_int32(0);
                0
            }
            TRANSACTION_GET_CLIPBOARD_DATA => {
                let data = get_clipboard();
                reply.append_int32(0);
                reply.append_string16(&data);
                0
            }
            _ => -99999,
        }
    };
    serve(args, INTERFACE, SERVICE_NAME, handler, stop);
}
