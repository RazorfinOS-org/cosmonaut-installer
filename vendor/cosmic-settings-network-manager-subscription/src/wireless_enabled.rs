// Copyright 2024 System76 <info@system76.com>
// SPDX-License-Identifier: MPL-2.0

use super::Event;
use cosmic_dbus_networkmanager::nm::NetworkManager;
use futures::{SinkExt, StreamExt};
use zbus::Connection;

// Upstream `wireless_enabled_subscription` wrapper removed; see active_conns.rs
// for the rationale. `watch` below is the actual entry point cosmonaut uses.

#[derive(Debug, Clone)]
pub enum State {
    Continue(Connection),
    Error,
}

pub async fn watch(conn: zbus::Connection, mut output: futures::channel::mpsc::Sender<Event>) {
    let mut state = State::Continue(conn);

    loop {
        state = start_listening(state, &mut output).await;
    }
}

async fn start_listening(
    state: State,
    output: &mut futures::channel::mpsc::Sender<Event>,
) -> State {
    let conn = match state {
        State::Continue(conn) => conn,
        State::Error => futures::future::pending().await,
    };

    let network_manager = match NetworkManager::new(&conn).await {
        Ok(n) => n,
        Err(why) => {
            tracing::error!(why = why.to_string(), "Failed to connect to NetworkManager");
            return State::Error;
        }
    };

    let mut wireless_enabled_changed = network_manager.receive_wireless_enabled_changed().await;

    while let Some(change) = wireless_enabled_changed.next().await {
        if let Ok(enable) = change.get().await {
            _ = output.send(Event::WiFiEnabled(enable)).await;
        }
    }
    State::Continue(conn)
}
