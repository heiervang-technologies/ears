//! Unix Domain Socket IPC server for streaming events
//!
//! Broadcasts `StreamingEvent`s as newline-delimited JSON to connected clients
//! over a Unix socket at `$XDG_RUNTIME_DIR/ears.sock` (fallback `/tmp/ears.sock`).

use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;
use tokio::sync::broadcast;
use tracing::{debug, error, info};

use crate::streaming_engine::StreamingEvent;

/// Return the default IPC socket path.
pub fn socket_path() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(|d| PathBuf::from(d).join("ears.sock"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/ears.sock"))
}

/// Start the IPC server with a custom socket path in a background tokio task.
pub fn start_ipc_server_at(path: PathBuf, rx: broadcast::Receiver<StreamingEvent>) {
    tokio::spawn(async move {
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }

        let listener = match UnixListener::bind(&path) {
            Ok(l) => l,
            Err(e) => {
                error!("Failed to bind IPC socket at {}: {}", path.display(), e);
                return;
            }
        };

        info!("IPC server listening on {}", path.display());

        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, _addr)) => {
                            debug!("IPC client connected");
                            let client_rx = rx.resubscribe();
                            tokio::spawn(handle_client(stream, client_rx));
                        }
                        Err(e) => {
                            error!("IPC accept error: {}", e);
                        }
                    }
                }
            }
        }
    });
}

/// Remove a specific socket file (best-effort cleanup).
pub fn cleanup_socket_at(path: &PathBuf) {
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
}

/// Start the IPC server in a background tokio task.
///
/// Accepts concurrent client connections and broadcasts every event received
/// on `rx` as a newline-delimited JSON line to each connected client.
pub fn start_ipc_server(rx: broadcast::Receiver<StreamingEvent>) {
    tokio::spawn(async move {
        let sock_path = socket_path();

        // Remove stale socket from a previous run
        if sock_path.exists() {
            let _ = std::fs::remove_file(&sock_path);
        }

        let listener = match UnixListener::bind(&sock_path) {
            Ok(l) => l,
            Err(e) => {
                error!(
                    "Failed to bind IPC socket at {}: {}",
                    sock_path.display(),
                    e
                );
                return;
            }
        };

        info!("IPC server listening on {}", sock_path.display());

        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, _addr)) => {
                            debug!("IPC client connected");
                            let client_rx = rx.resubscribe();
                            tokio::spawn(handle_client(stream, client_rx));
                        }
                        Err(e) => {
                            error!("IPC accept error: {}", e);
                        }
                    }
                }
            }
        }
    });
}

/// Handle a single connected client, forwarding events until disconnect.
async fn handle_client(
    mut stream: tokio::net::UnixStream,
    mut rx: broadcast::Receiver<StreamingEvent>,
) {
    loop {
        match rx.recv().await {
            Ok(event) => {
                let json = match serde_json::to_string(&event) {
                    Ok(j) => j,
                    Err(e) => {
                        error!("Failed to serialize event: {}", e);
                        continue;
                    }
                };
                // Newline-delimited JSON
                if stream.write_all(json.as_bytes()).await.is_err()
                    || stream.write_all(b"\n").await.is_err()
                {
                    debug!("IPC client disconnected");
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                debug!("IPC client lagged, skipped {} events", n);
            }
            Err(broadcast::error::RecvError::Closed) => {
                debug!("IPC broadcast channel closed");
                break;
            }
        }
    }
}

/// Remove the socket file (best-effort cleanup).
pub fn cleanup_socket() {
    let path = socket_path();
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
}
