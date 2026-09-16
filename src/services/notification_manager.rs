// SPDX-License-Identifier: GPL-3.0-or-later
// The generated Notify proxy method mirrors the D-Bus signature, which has
// eight parameters plus the connection handle.
#![allow(clippy::too_many_arguments)]

//! Serves `INotifications` to the guest and forwards to the host notification
//! daemon on `org.freedesktop.Notifications`.

use crate::args::MosaicArgs;
use crate::config::SessionDefaults;
use crate::interfaces::i_notification_callback::INotificationCallback;
use crate::interfaces::i_notifications::{self, Action, ImageData, ID_NONE};
use crate::services::user_manager::DESKTOP_PREFIX;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use zbus::zvariant::OwnedValue;

static STOPPING: AtomicBool = AtomicBool::new(true);

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;

    fn close_notification(&self, id: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_id: String) -> zbus::Result<()>;

    #[zbus(signal)]
    fn activation_token(&self, id: u32, token: String) -> zbus::Result<()>;
}

fn bool_hint(value: bool) -> OwnedValue {
    OwnedValue::from(value)
}

fn byte_hint(value: u8) -> OwnedValue {
    OwnedValue::from(value)
}

fn string_hint(value: &str) -> OwnedValue {
    let s: zbus::zvariant::Str = value.into();
    OwnedValue::try_from(zbus::zvariant::Value::Str(s)).unwrap_or_else(|_| OwnedValue::from(0u8))
}

#[allow(clippy::too_many_arguments)]
fn build_hints(
    package_name: &str,
    category: &str,
    image_data: &Option<ImageData>,
    suppress_sound: bool,
    resident: bool,
    transient: bool,
    urgency: u8,
) -> HashMap<String, OwnedValue> {
    let mut hints: HashMap<String, OwnedValue> = HashMap::new();
    hints.insert(
        "desktop-entry".to_string(),
        string_hint(&format!("{}{}", DESKTOP_PREFIX, package_name)),
    );
    hints.insert("resident".to_string(), bool_hint(resident));
    hints.insert("transient".to_string(), bool_hint(transient));
    hints.insert("urgency".to_string(), byte_hint(urgency));
    hints.insert("suppress-sound".to_string(), bool_hint(suppress_sound));
    if !category.is_empty() {
        hints.insert("category".to_string(), string_hint(category));
    }
    if let Some(image) = image_data {
        // (width, height, rowstride, has_alpha, bits_per_sample, channels, data)
        let channels: i32 = if image.has_alpha { 4 } else { 3 };
        let data = zbus::zvariant::Array::from(
            image
                .data
                .iter()
                .map(|b| zbus::zvariant::Value::U8(*b))
                .collect::<Vec<_>>(),
        );
        let structure = zbus::zvariant::Structure::from((
            image.width,
            image.height,
            image.rowstride,
            image.has_alpha,
            8i32,
            channels,
            data,
        ));
        if let Ok(owned) = zbus::zvariant::Value::Structure(structure).try_to_owned() {
            hints.insert("image-data".to_string(), owned);
        }
    }
    hints
}

pub fn start(args: &MosaicArgs, _session: &SessionDefaults) -> anyhow::Result<()> {
    // Everything runs on the service thread. Building the runtime and calling
    // block_on in the caller would run inside the async entry point's runtime
    // and panic with "Cannot start a runtime from within a runtime".
    STOPPING.store(false, Ordering::SeqCst);
    let args = args.clone();
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => Arc::new(rt),
            Err(e) => {
                log::debug!("No notification runtime: {}", e);
                return;
            }
        };

        // Verify the host notification daemon exists before starting the service.
        let connected = runtime.block_on(async {
            let connection = zbus::Connection::session().await.ok()?;
            NotificationsProxy::new(&connection).await.ok()
        });
        let Some(proxy) = connected else {
            log::info!(
                "Skipping notification manager service because we could not connect to the notifications server"
            );
            return;
        };

        let listeners: Arc<Mutex<Vec<INotificationCallback>>> = Arc::new(Mutex::new(Vec::new()));
        let pending_tokens: Arc<Mutex<HashMap<u32, String>>> = Arc::new(Mutex::new(HashMap::new()));

        // Dispatch host signals to the registered guest listeners.
        {
            let runtime = runtime.clone();
            let listeners = listeners.clone();
            let pending_tokens = pending_tokens.clone();
            let proxy = proxy.clone();
            runtime.spawn(async move {
                let mut actions = match NotificationsProxy::receive_action_invoked(&proxy).await {
                    Ok(stream) => stream,
                    Err(e) => {
                        log::debug!("Failed to subscribe to ActionInvoked: {}", e);
                        return;
                    }
                };
                while let Some(signal) = futures_util::StreamExt::next(&mut actions).await {
                    if let Ok(args) = signal.args() {
                        let token = pending_tokens
                            .lock()
                            .unwrap()
                            .remove(&args.id)
                            .unwrap_or_default();
                        for listener in listeners.lock().unwrap().iter() {
                            listener.on_action_invoked(args.id as i32, &args.action_id, &token);
                        }
                    }
                }
            });
        }
        {
            let runtime = runtime.clone();
            let pending_tokens = pending_tokens.clone();
            let proxy = proxy.clone();
            runtime.spawn(async move {
                let mut tokens = match NotificationsProxy::receive_activation_token(&proxy).await {
                    Ok(stream) => stream,
                    Err(e) => {
                        log::debug!("Failed to subscribe to ActivationToken: {}", e);
                        return;
                    }
                };
                while let Some(signal) = futures_util::StreamExt::next(&mut tokens).await {
                    if let Ok(args) = signal.args() {
                        pending_tokens
                            .lock()
                            .unwrap()
                            .insert(args.id, args.token.clone());
                    }
                }
            });
        }

        while !STOPPING.load(Ordering::SeqCst) {
            let notify_runtime = runtime.clone();
            let notify_proxy = proxy.clone();
            let listeners_for_register = listeners.clone();
            let notify_args = args.clone();
            let close_runtime = runtime.clone();
            let close_proxy = proxy.clone();
            i_notifications::add_service(
                &args,
                move |mut listener: INotificationCallback| {
                    listener.add_death_handler({
                        let listeners = listeners_for_register.clone();
                        move || {
                            // The guest side died; drop every listener. The
                            // list is short and re-registration follows.
                            listeners.lock().unwrap().clear();
                        }
                    });
                    listeners_for_register.lock().unwrap().push(listener);
                },
                move |replaces_id,
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
                      urgency| {
                    let _ = &notify_args;
                    let action_strings: Vec<String> = actions
                        .iter()
                        .flat_map(|a: &Action| [a.id.clone(), a.label.clone()])
                        .collect();
                    let hints = build_hints(
                        &package_name,
                        &category,
                        &image_data,
                        suppress_sound,
                        resident,
                        transient,
                        urgency,
                    );
                    let app_icon = String::new();
                    let result = notify_runtime.block_on(async {
                        notify_proxy
                            .notify(
                                &app_name,
                                replaces_id as u32,
                                &app_icon,
                                &summary,
                                &body,
                                action_strings,
                                hints,
                                expire_timeout,
                            )
                            .await
                    });
                    match result {
                        Ok(id) => id as i32,
                        Err(e) => {
                            log::warn!("Failed to post notification: {}", e);
                            ID_NONE
                        }
                    }
                },
                move |notification_id| {
                    let _ = close_runtime.block_on(async {
                        close_proxy.close_notification(notification_id as u32).await
                    });
                },
                &STOPPING,
            );
        }
    });

    Ok(())
}

pub fn stop(_args: &MosaicArgs) -> anyhow::Result<()> {
    STOPPING.store(true, Ordering::SeqCst);
    Ok(())
}
