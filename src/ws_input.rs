//! WebSocket audio input server
//!
//! Accepts WebSocket connections and streams PCM audio into the VAD pipeline.
//! Echoes transcription events back to the client as JSON text frames.
//!
//! Protocol:
//! - Client sends JSON text frame: `{"type": "start", "sample_rate": 16000, "channels": 1}`
//! - Client sends binary frames: raw PCM s16le audio chunks
//! - Client sends JSON text frame: `{"type": "end"}`
//! - Server sends JSON text frames: streaming engine events (transcription updates, etc.)

use crate::streaming_engine::StreamingEvent;
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};

/// Start the WebSocket server. Sends received PCM audio as `Vec<f32>` through `audio_tx`.
/// Echoes streaming events from `event_tx` back to connected clients.
///
/// Returns a `JoinHandle` for the server task. The server runs until the handle is aborted
/// or the process exits.
pub async fn start_ws_server(
    host: &str,
    port: u16,
    audio_tx: mpsc::UnboundedSender<Vec<f32>>,
    event_tx: broadcast::Sender<StreamingEvent>,
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
                    let event_rx = event_tx.subscribe();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, tx, event_rx).await {
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
    mut event_rx: broadcast::Receiver<StreamingEvent>,
) -> anyhow::Result<()> {
    let ws_stream = tokio_tungstenite::accept_async(stream).await?;
    let (mut write, mut read) = ws_stream.split();

    let mut session_active = false;

    loop {
        tokio::select! {
            msg = read.next() => {
                let Some(msg) = msg else { break };
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
                                    // Send ~1s of silence to flush the VAD pipeline
                                    // (forces speech segment to end via silence timeout)
                                    let silence = vec![0.0f32; 16000];
                                    let _ = audio_tx.send(silence);
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
            event = event_rx.recv() => {
                match event {
                    Ok(ev) => {
                        if let Ok(json) = serde_json::to_string(&ev) {
                            if write.send(Message::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        debug!("WS client lagged, skipped {} events", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    Ok(())
}
