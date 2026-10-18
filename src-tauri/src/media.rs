//! Connects the pairing server to the picture: a WebRTC connection (peer.rs)
//! for every receiver that is let in, offered again if it drops while the
//! receiver is still there.

use crate::peer::{self, Ended};
use crate::server::{Hub, Inbound, MediaEvent, Outbound};
use crate::video::Video;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Start listening for receivers; runs for the life of the app.
pub fn start(hub: Arc<Hub>, video: Video) {
    let mut events = hub.take_signals();
    tauri::async_runtime::spawn(async move {
        let mut peers: HashMap<u64, mpsc::UnboundedSender<Inbound>> = HashMap::new();
        while let Some(ev) = events.recv().await {
            match ev {
                MediaEvent::Joined { session } => {
                    let (tx, rx) = mpsc::unbounded_channel();
                    peers.insert(session, tx);
                    tauri::async_runtime::spawn(serve(hub.clone(), session, rx, video.clone()));
                }
                // Dropping the sender ends that receiver's connection.
                MediaEvent::Left { session } => {
                    peers.remove(&session);
                }
                MediaEvent::Signal { session, msg } => {
                    if let Some(tx) = peers.get(&session) {
                        let _ = tx.send(msg);
                    }
                }
            }
        }
    });
}

/// One receiver, for as long as it stays.
async fn serve(
    hub: Arc<Hub>,
    session: u64,
    mut signals: mpsc::UnboundedReceiver<Inbound>,
    video: Video,
) {
    let mut tries = 0u32;
    loop {
        match peer::run(&hub, session, &mut signals, &video).await {
            Ended::Left => return,
            Ended::Fatal(why) => {
                tracing::warn!("session {session}: {why}");
                hub.send(session, &Outbound::Notice { text: &why });
                // Nothing more to do until the receiver leaves.
                while signals.recv().await.is_some() {}
                return;
            }
            Ended::Dropped(why) => {
                tracing::info!("session {session}: {why}; offering again");
                tries += 1;
                // Back off a little, but keep trying while the receiver is there.
                let wait = Duration::from_millis(500 * u64::from(tries.min(10)));
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    s = signals.recv() => if s.is_none() { return },
                }
            }
        }
    }
}
