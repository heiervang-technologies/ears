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
                            if write.send(Message::Text(json)).await.is_err() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::connect_async;

    /// Helper: start the WS server on an ephemeral port and return (port, audio_rx, event_tx, handle)
    async fn setup_server() -> (
        u16,
        mpsc::UnboundedReceiver<Vec<f32>>,
        broadcast::Sender<StreamingEvent>,
        tokio::task::JoinHandle<()>,
    ) {
        let (audio_tx, audio_rx) = mpsc::unbounded_channel();
        let (event_tx, _) = broadcast::channel(64);
        // Bind to port 0 to get an ephemeral port — but start_ws_server takes host/port,
        // so we bind manually and extract the port.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let event_tx_clone = event_tx.clone();
        let handle = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _peer)) => {
                        let tx = audio_tx.clone();
                        let event_rx = event_tx_clone.subscribe();
                        tokio::spawn(async move {
                            let _ = handle_connection(stream, tx, event_rx).await;
                        });
                    }
                    Err(_) => break,
                }
            }
        });

        (port, audio_rx, event_tx, handle)
    }

    /// Connect a WS client to the test server
    async fn connect_client(
        port: u16,
    ) -> (
        futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            Message,
        >,
        futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
    ) {
        let url = format!("ws://127.0.0.1:{}", port);
        let (ws, _) = connect_async(&url).await.expect("Failed to connect");
        ws.split()
    }

    /// Core protocol: start → binary PCM → end. Validates that audio arrives as f32 samples
    /// and that the "end" message flushes silence into the pipeline.
    #[tokio::test]
    async fn test_ws_start_send_pcm_end() {
        let (port, mut audio_rx, _event_tx, handle) = setup_server().await;
        let (mut write, _read) = connect_client(port).await;

        // Send start
        write
            .send(Message::Text(
                r#"{"type": "start", "sample_rate": 16000, "channels": 1}"#.into(),
            ))
            .await
            .unwrap();

        // Send a binary frame with 4 s16le samples: [100, -100, 0, 32767]
        let samples_i16: Vec<i16> = vec![100, -100, 0, 32767];
        let pcm: Vec<u8> = samples_i16.iter().flat_map(|s| s.to_le_bytes()).collect();
        write.send(Message::Binary(pcm)).await.unwrap();

        // Receive the audio
        let received = audio_rx.recv().await.expect("Should receive audio");
        assert_eq!(received.len(), 4);
        // Verify conversion: i16 / 32768.0
        assert!((received[0] - 100.0 / 32768.0).abs() < 1e-6);
        assert!((received[1] - (-100.0 / 32768.0)).abs() < 1e-6);
        assert!((received[2] - 0.0).abs() < 1e-6);
        assert!((received[3] - 32767.0 / 32768.0).abs() < 1e-6);

        // Send end
        write
            .send(Message::Text(r#"{"type": "end"}"#.into()))
            .await
            .unwrap();

        // Should receive a silence flush (16000 samples of zeros)
        let silence = audio_rx.recv().await.expect("Should receive silence flush");
        assert_eq!(silence.len(), 16000);
        assert!(silence.iter().all(|&s| s == 0.0));

        handle.abort();
    }

    /// Binary frames sent before "start" must be ignored (not forwarded to audio channel).
    #[tokio::test]
    async fn test_ws_binary_before_start_ignored() {
        let (port, mut audio_rx, _event_tx, handle) = setup_server().await;
        let (mut write, _read) = connect_client(port).await;

        // Send binary without start — should be ignored
        let pcm: Vec<u8> = vec![0u8; 64];
        write.send(Message::Binary(pcm)).await.unwrap();

        // Now start and send real audio
        write
            .send(Message::Text(r#"{"type": "start"}"#.into()))
            .await
            .unwrap();
        let real_pcm: Vec<u8> = 100i16.to_le_bytes().to_vec();
        write.send(Message::Binary(real_pcm)).await.unwrap();

        // Only the post-start audio should arrive
        let received = audio_rx.recv().await.expect("Should receive audio");
        assert_eq!(received.len(), 1);

        handle.abort();
    }

    /// Server echoes StreamingEvents back to the client as JSON text frames.
    #[tokio::test]
    async fn test_ws_event_echo() {
        let (port, _audio_rx, event_tx, handle) = setup_server().await;
        let (mut write, mut read) = connect_client(port).await;

        // Start session so the connection is active
        write
            .send(Message::Text(r#"{"type": "start"}"#.into()))
            .await
            .unwrap();

        // Give the server a moment to set up the event subscriber
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Broadcast an event
        let _ = event_tx.send(StreamingEvent::SpeechStarted);

        // Client should receive it as JSON
        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), read.next())
            .await
            .expect("Timed out waiting for event")
            .expect("Stream ended")
            .expect("WS error");

        if let Message::Text(json) = msg {
            assert!(json.contains("SpeechStarted"), "Got: {}", json);
        } else {
            panic!("Expected text frame, got: {:?}", msg);
        }

        handle.abort();
    }

    /// Odd-length binary frames are handled gracefully (trimmed via chunks_exact).
    #[tokio::test]
    async fn test_ws_odd_byte_pcm_frame() {
        let (port, mut audio_rx, _event_tx, handle) = setup_server().await;
        let (mut write, _read) = connect_client(port).await;

        write
            .send(Message::Text(r#"{"type": "start"}"#.into()))
            .await
            .unwrap();

        // 5 bytes = 2 full samples + 1 trailing byte (dropped by chunks_exact)
        let pcm: Vec<u8> = vec![0, 0, 1, 0, 0xFF];
        write.send(Message::Binary(pcm)).await.unwrap();

        let received = audio_rx.recv().await.expect("Should receive audio");
        assert_eq!(
            received.len(),
            2,
            "Should have 2 samples, trailing byte dropped"
        );

        handle.abort();
    }

    /// Client close is handled gracefully without panics.
    #[tokio::test]
    async fn test_ws_client_disconnect() {
        let (port, _audio_rx, _event_tx, handle) = setup_server().await;
        let (mut write, _read) = connect_client(port).await;

        write
            .send(Message::Text(r#"{"type": "start"}"#.into()))
            .await
            .unwrap();

        // Close the connection
        write.send(Message::Close(None)).await.unwrap();

        // Server should handle this without panicking — give it a moment
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        handle.abort();
    }
}
