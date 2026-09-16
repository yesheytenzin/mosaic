// SPDX-License-Identifier: GPL-3.0-or-later

//! Client side of the notification callback the guest registers with us.

use crate::interfaces::gbinder::{Client, RemoteObject};

const INTERFACE: &str = crate::guest::IFACE_NOTIFICATION_CALLBACK;

const TRANSACTION_ON_ACTION_INVOKED: u32 = 1;

pub struct INotificationCallback {
    client: Client,
}

impl INotificationCallback {
    pub fn new(remote: RemoteObject) -> Self {
        Self {
            client: Client::new(remote, INTERFACE),
        }
    }

    /// Notify when the guest side goes away, so listeners can be dropped.
    pub fn add_death_handler(&mut self, handler: impl Fn() + Send + 'static) {
        self.client.remote.add_death_handler(handler);
    }

    pub fn on_action_invoked(
        &self,
        notification_id: i32,
        action_id: &str,
        xdg_activation_token: &str,
    ) {
        let mut request = self.client.new_request();
        request.append_int32(notification_id);
        request.append_string16(action_id);
        request.append_string16(xdg_activation_token);
        self.client
            .transact_sync_oneway(TRANSACTION_ON_ACTION_INVOKED, request);
    }
}
