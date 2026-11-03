//! Implementazione del protocollo Discord.

pub mod gateway;
pub mod models;
pub mod remote_auth;
pub mod rest;
pub mod voice;
mod zlib;

use anyhow::Result;
use futures_util::{stream::SplitSink, SinkExt};
use tokio::{net::TcpStream, time::Interval};
use tokio_tungstenite::{tungstenite::Message as WsMessage, MaybeTlsStream, WebSocketStream};

pub(crate) const USER_AGENT: &str = concat!("ferrocord/", env!("CARGO_PKG_VERSION"));

pub(crate) type WsSink = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, WsMessage>;

pub(crate) async fn send_json(sink: &mut WsSink, value: &serde_json::Value) -> Result<()> {
    sink.send(WsMessage::Text(value.to_string())).await?;
    Ok(())
}

/// Attende il prossimo tick di un intervallo opzionale (mai, se è `None`).
/// Serve per gli heartbeat, che partono solo dopo l'`hello` del server.
pub(crate) async fn tick_opt(interval: &mut Option<Interval>) {
    match interval {
        Some(i) => {
            i.tick().await;
        }
        None => std::future::pending::<()>().await,
    }
}
