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
pub fn cleanup_socket_at(path: &std::path::Path) {
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
}

/// Start the IPC server in a background tokio task at the default socket path.
///
/// Accepts concurrent client connections and broadcasts every event received
/// on `rx` as a newline-delimited JSON line to each connected client.
pub fn start_ipc_server(rx: broadcast::Receiver<StreamingEvent>) {
    start_ipc_server_at(socket_path(), rx);
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

/// Remove the default socket file (best-effort cleanup).
pub fn cleanup_socket() {
    cleanup_socket_at(&socket_path());
}

// --- Command IPC (bidirectional) ---

/// Return the command socket path.
pub fn cmd_socket_path() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(|d| PathBuf::from(d).join("ears-cmd.sock"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/ears-cmd.sock"))
}

/// Commands that can be sent to a running ears instance.
#[derive(Debug)]
pub enum EarsCommand {
    ToggleAutoEnter {
        respond: tokio::sync::oneshot::Sender<String>,
    },
}

/// Start the command server, returning received commands via the channel.
pub fn start_cmd_server(cmd_tx: tokio::sync::mpsc::UnboundedSender<EarsCommand>) {
    let sock_path = cmd_socket_path();
    tokio::spawn(async move {
        if sock_path.exists() {
            let _ = std::fs::remove_file(&sock_path);
        }

        let listener = match UnixListener::bind(&sock_path) {
            Ok(l) => l,
            Err(e) => {
                error!(
                    "Failed to bind command socket at {}: {}",
                    sock_path.display(),
                    e
                );
                return;
            }
        };

        info!("Command server listening on {}", sock_path.display());

        loop {
            match listener.accept().await {
                Ok((mut stream, _)) => {
                    let tx = cmd_tx.clone();
                    tokio::spawn(async move {
                        use tokio::io::{AsyncBufReadExt, BufReader};
                        let (reader, mut writer) = stream.split();
                        let mut lines = BufReader::new(reader).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let response = match line.trim() {
                                "toggle-auto-enter" => {
                                    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
                                    let _ =
                                        tx.send(EarsCommand::ToggleAutoEnter { respond: resp_tx });
                                    resp_rx
                                        .await
                                        .unwrap_or_else(|_| "error:internal".to_string())
                                }
                                _ => "error:unknown-command".to_string(),
                            };
                            if writer
                                .write_all(format!("{}\n", response).as_bytes())
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    });
                }
                Err(e) => {
                    error!("Command accept error: {}", e);
                }
            }
        }
    });
}

/// Remove the command socket file.
pub fn cleanup_cmd_socket() {
    cleanup_socket_at(&cmd_socket_path());
}

/// Send a command to a running ears instance. Returns the response.
pub async fn send_command(cmd: &str) -> anyhow::Result<String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let sock_path = cmd_socket_path();
    let mut stream = tokio::net::UnixStream::connect(&sock_path)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to ears: {}", e))?;
    stream.write_all(cmd.as_bytes()).await?;
    stream.write_all(b"\n").await?;
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response).await?;
    Ok(response.trim().to_string())
}
