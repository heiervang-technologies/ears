//! WebSocket audio input server
//!
//! Accepts WebSocket connections and streams PCM audio into the VAD pipeline.
//!
//! Protocol:
//! - Client sends JSON text frame: `{"type": "start", "sample_rate": 16000, "channels": 1}`
//! - Client sends binary frames: raw PCM s16le audio chunks
//! - Client sends JSON text frame: `{"type": "end"}`

use futures_util::StreamExt;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};

/// Start the WebSocket server. Sends received PCM audio as `Vec<f32>` through `audio_tx`.
///
/// Returns a `JoinHandle` for the server task. The server runs until the handle is aborted
/// or the process exits.
pub async fn start_ws_server(
    host: &str,
    port: u16,
    audio_tx: mpsc::UnboundedSender<Vec<f32>>,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
    let listener = TcpListener::bind(&addr).await?;
    info!("WebSocket server listening on ws://{}", addr);

    let handle = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    info!("WebSocket connection from {}", peer);
                    let tx = audio_tx.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, tx).await {
                            warn!("WebSocket client {} error: {}", peer, e);
                        }
                        info!("WebSocket client {} disconnected", peer);
                    });
                }
                Err(e) => {
                    error!("WebSocket accept error: {}", e);
                }
            }
        }
    });

    Ok(handle)
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    audio_tx: mpsc::UnboundedSender<Vec<f32>>,
) -> anyhow::Result<()> {
    let ws_stream = tokio_tungstenite::accept_async(stream).await?;
    let (_, mut read) = ws_stream.split();

    let mut session_active = false;

    while let Some(msg) = read.next().await {
        let msg = msg?;
        match msg {
            Message::Text(text) => {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                    match json.get("type").and_then(|v| v.as_str()) {
                        Some("start") => {
                            info!("WebSocket audio session started");
                            session_active = true;
                        }
                        Some("end") => {
                            info!("WebSocket audio session ended");
                            session_active = false;
                        }
                        other => {
                            debug!("Unknown WS message type: {:?}", other);
                        }
                    }
                }
            }
            Message::Binary(data) => {
                if !session_active {
                    debug!("Ignoring binary frame outside session");
                    continue;
                }
                if data.len() % 2 != 0 {
                    warn!("Odd byte count in PCM frame ({}), trimming", data.len());
                }
                let samples: Vec<f32> = data
                    .chunks_exact(2)
                    .map(|b| {
                        let s = i16::from_le_bytes([b[0], b[1]]);
                        s as f32 / 32768.0
                    })
                    .collect();
                if audio_tx.send(samples).is_err() {
                    debug!("Audio receiver dropped, closing WS connection");
                    break;
                }
            }
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }

    Ok(())
}
