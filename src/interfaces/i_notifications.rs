// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use crate::interfaces::gbinder::{serve, Reader, Writer};
use crate::interfaces::i_notification_callback::INotificationCallback;
use std::sync::atomic::AtomicBool;

const INTERFACE: &str = crate::guest::IFACE_NOTIFICATIONS;
const SERVICE_NAME: &str = crate::guest::SVC_NOTIFICATIONS;

const TRANSACTION_REGISTER_LISTENER: u32 = 1;
const TRANSACTION_NOTIFY: u32 = 2;
const TRANSACTION_CLOSE_NOTIFICATION: u32 = 3;

/// Parcelable flag value meaning "present".
const K_NULL_PARCELABLE_FLAG: i32 = 0;

/// Returned when no notification id was assigned.
pub const ID_NONE: i32 = 0;

pub mod urgency {
    pub const LOW: u8 = 0;
    pub const NORMAL: u8 = 1;
    pub const CRITICAL: u8 = 2;
}

pub struct Action {
    pub id: String,
    pub label: String,
}

pub struct ImageData {
    pub width: i32,
    pub height: i32,
    pub rowstride: i32,
    pub has_alpha: bool,
    pub data: Vec<u8>,
}

#[allow(clippy::too_many_arguments)]
pub fn add_service<F1, F2, F3>(
    args: &MosaicArgs,
    register_listener: F1,
    notify: F2,
    close_notification: F3,
    stop: &AtomicBool,
) where
    F1: Fn(INotificationCallback) + Send + 'static,
    F2: Fn(
            i32,
            String,
            String,
            String,
            String,
            Vec<Action>,
            Option<ImageData>,
            String,
            bool,
            i32,
            bool,
            bool,
            u8,
        ) -> i32
        + Send
        + 'static,
    F3: Fn(i32) + Send + 'static,
{
    let handler = move |mut reader: Reader, code: u32, _flags: u32, reply: &mut Writer| -> i32 {
        log::debug!("{}: Received transaction: {}", SERVICE_NAME, code);
        match code {
            TRANSACTION_REGISTER_LISTENER => {
                match reader.read_object() {
                    Ok(Some(remote)) => register_listener(INotificationCallback::new(remote)),
                    Ok(None) => log::debug!("registerListener without an object"),
                    Err(e) => log::debug!("Failed to read listener: {}", e),
                }
                reply.append_int32(0);
                0
            }
            TRANSACTION_NOTIFY => {
                let (_, replaces_id) = reader.read_int32().unwrap_or((0, 0));
                let app_name = reader.read_string16().unwrap_or_default();
                let package_name = reader.read_string16().unwrap_or_default();
                let summary = reader.read_string16().unwrap_or_default();
                let body = reader.read_string16().unwrap_or_default();

                let mut actions = Vec::new();
                let (_, actions_length) = reader.read_int32().unwrap_or((0, 0));
                for _ in 0..actions_length {
                    let (_, null_flag) = reader.read_int32().unwrap_or((0, 0));
                    if null_flag != K_NULL_PARCELABLE_FLAG {
                        let _ = reader.read_int32(); // parcel size
                        let id = reader.read_string16().unwrap_or_default();
                        let label = reader.read_string16().unwrap_or_default();
                        actions.push(Action { id, label });
                    }
                }

                let (_, null_flag) = reader.read_int32().unwrap_or((0, 0));
                let image_data = if null_flag != K_NULL_PARCELABLE_FLAG {
                    let _ = reader.read_int32(); // parcel size
                    let (_, width) = reader.read_int32().unwrap_or((0, 0));
                    let (_, height) = reader.read_int32().unwrap_or((0, 0));
                    let (_, rowstride) = reader.read_int32().unwrap_or((0, 0));
                    let (_, has_alpha) = reader.read_bool().unwrap_or((0, false));
                    let data = reader.read_byte_array().unwrap_or_default();
                    Some(ImageData {
                        width,
                        height,
                        rowstride,
                        has_alpha,
                        data,
                    })
                } else {
                    None
                };

                let category = reader.read_string16().unwrap_or_default();
                let (_, suppress_sound) = reader.read_bool().unwrap_or((0, false));
                let (_, expire_timeout) = reader.read_int32().unwrap_or((0, 0));
                let (_, resident) = reader.read_bool().unwrap_or((0, false));
                let (_, transient) = reader.read_bool().unwrap_or((0, false));
                let (_, urgency) = reader.read_byte().unwrap_or((0, 0));

                let notification_id = notify(
                    replaces_id,
                    app_name,
                    package_name,
                    summary,
                    body,
                    actions,
                    image_data,
                    category,
                    suppress_sound,
                    expire_timeout,
                    resident,
                    transient,
                    urgency,
                );
                reply.append_int32(0);
                reply.append_int32(notification_id);
                0
            }
            TRANSACTION_CLOSE_NOTIFICATION => {
                let (_, notification_id) = reader.read_int32().unwrap_or((0, 0));
                close_notification(notification_id);
                reply.append_int32(0);
                0
            }
            _ => -99999,
        }
    };
    serve(args, INTERFACE, SERVICE_NAME, handler, stop);
}
