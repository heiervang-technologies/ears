//! WebSocket audio input server
//!
//! Accepts WebSocket connections and streams PCM audio into the VAD pipeline.
//! Echoes transcription events back to the client as JSON text frames.
//!
//! Protocol:
//! - Client sends JSON text frame: `{"type": "start", "sample_rate": 16000, "channels": 1}`
//! - Client sends binary frames: raw PCM s16le audio chunks, at most 64 KiB
//! - Uploads wait for bounded queue space; split larger audio into messages
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
    audio_tx: mpsc::Sender<Vec<f32>>,
    event_tx: broadcast::Sender<StreamingEvent>,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
    let listener = TcpListener::bind(&addr).await?;
    info!("WebSocket server listening on ws://{}", addr);

    Ok(spawn_server(listener, audio_tx, event_tx))
}

fn spawn_server(
    listener: TcpListener,
    audio_tx: mpsc::Sender<Vec<f32>>,
    event_tx: broadcast::Sender<StreamingEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Aborting the listener also cancels clients blocked on queue space.
        let mut clients = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => match accepted {
                    Ok((stream, peer)) => {
                        info!("WebSocket connection from {}", peer);
                        let tx = audio_tx.clone();
                        let event_rx = event_tx.subscribe();
                        clients.spawn(async move {
                            if let Err(e) = handle_connection(stream, tx, event_rx).await {
                                warn!("WebSocket client {} error: {}", peer, e);
                            }
                            info!("WebSocket client {} disconnected", peer);
                        });
                    }
                    Err(e) => error!("WebSocket accept error: {}", e),
                },
                Some(result) = clients.join_next(), if !clients.is_empty() => {
                    if let Err(error) = result {
                        warn!(%error, "WebSocket client task failed");
                    }
                }
            }
        }
    })
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    audio_tx: mpsc::Sender<Vec<f32>>,
    mut event_rx: broadcast::Receiver<StreamingEvent>,
) -> anyhow::Result<()> {
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
        max_message_size: Some(64 * 1024),
        max_frame_size: Some(64 * 1024),
        ..Default::default()
    };
    let ws_stream = tokio_tungstenite::accept_async_with_config(stream, Some(config)).await?;
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
                                    for _ in 0..10 {
                                        audio_tx.send(vec![0.0; crate::pipeline::AUDIO_CHUNK_SAMPLES]).await?;
                                    }
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
                        // Bound each queued allocation; await capacity so TCP
                        // backpressure reaches uploaders instead of losing PCM.
                        for bytes in data.chunks(crate::pipeline::AUDIO_CHUNK_SAMPLES * 2) {
                            let samples: Vec<f32> = bytes.as_chunks::<2>().0.iter()
                                .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
                                .collect();
                            if !samples.is_empty() {
                                audio_tx.send(samples).await?;
                            }
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
        mpsc::Receiver<Vec<f32>>,
        broadcast::Sender<StreamingEvent>,
        tokio::task::JoinHandle<()>,
    ) {
        let (audio_tx, audio_rx) = crate::pipeline::audio_channel();
        let (event_tx, _) = broadcast::channel(64);
        // Bind to port 0 to get an ephemeral port — but start_ws_server takes host/port,
        // so we bind manually and extract the port.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let handle = spawn_server(listener, audio_tx, event_tx.clone());

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
        for _ in 0..10 {
            let silence = audio_rx.recv().await.expect("Should receive silence flush");
            assert_eq!(silence.len(), crate::pipeline::AUDIO_CHUNK_SAMPLES);
            assert!(silence.iter().all(|&s| s == 0.0));
        }

        handle.abort();
    }

    #[tokio::test]
    async fn slow_consumer_bounds_queue_and_preserves_pcm_order() {
        let (port, mut audio_rx, _events, handle) = setup_server().await;
        let (mut write, _read) = connect_client(port).await;
        write
            .send(Message::Text(r#"{"type":"start"}"#.into()))
            .await
            .unwrap();
        // 104 chunks exceed the 100-chunk queue; use multi-chunk messages.
        for group in 0..26 {
            let mut bytes = Vec::new();
            for chunk in group * 4..group * 4 + 4 {
                for _ in 0..crate::pipeline::AUDIO_CHUNK_SAMPLES {
                    bytes.extend_from_slice(&(chunk as i16 - 52).to_le_bytes());
                }
            }
            write.send(Message::Binary(bytes)).await.unwrap();
        }
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while audio_rx.len() < crate::pipeline::AUDIO_QUEUE_CHUNKS {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(audio_rx.len(), crate::pipeline::AUDIO_QUEUE_CHUNKS);
        for chunk in 0..104 {
            let samples = tokio::time::timeout(std::time::Duration::from_secs(2), audio_rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(samples.len(), crate::pipeline::AUDIO_CHUNK_SAMPLES);
            assert!(samples
                .iter()
                .all(|&s| s == (chunk as f32 - 52.0) / 32768.0));
        }
        write.close().await.unwrap();
        handle.abort();
    }

    #[tokio::test]
    async fn aborting_server_closes_backpressured_clients() {
        let (port, audio_rx, _events, handle) = setup_server().await;
        let (mut write, mut read) = connect_client(port).await;
        write
            .send(Message::Text(r#"{"type":"start"}"#.into()))
            .await
            .unwrap();
        for _ in 0..110 {
            write
                .send(Message::Binary(vec![
                    0;
                    crate::pipeline::AUDIO_CHUNK_SAMPLES
                        * 2
                ]))
                .await
                .unwrap();
        }
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while audio_rx.len() < crate::pipeline::AUDIO_QUEUE_CHUNKS {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        handle.abort();
        let _ = handle.await;
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), read.next())
            .await
            .unwrap();
        assert!(matches!(
            result,
            None | Some(Err(_)) | Some(Ok(Message::Close(_)))
        ));
    }

    #[tokio::test]
    async fn oversized_message_is_rejected_before_audio_queue() {
        let (port, mut audio_rx, _events, handle) = setup_server().await;
        let (mut write, mut read) = connect_client(port).await;
        write
            .send(Message::Text(r#"{"type":"start"}"#.into()))
            .await
            .unwrap();
        write
            .send(Message::Binary(vec![0; 64 * 1024 + 2]))
            .await
            .unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), read.next())
            .await
            .unwrap();
        assert!(matches!(
            result,
            None | Some(Err(_)) | Some(Ok(Message::Close(_)))
        ));
        assert!(audio_rx.try_recv().is_err());
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
