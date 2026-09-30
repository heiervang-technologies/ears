//! Unix Domain Socket IPC server for streaming events
//!
//! Broadcasts `StreamingEvent`s as newline-delimited JSON to connected clients
//! over a Unix socket at `$XDG_RUNTIME_DIR/ears.sock` (fallback `/tmp/ears.sock`).

use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
mod socket;
use socket::OwnedSocket;
use tokio::sync::broadcast;
use tokio::task::{JoinHandle, JoinSet};
use tracing::{debug, error, info};

use crate::streaming_engine::StreamingEvent;

/// Return the default IPC socket path.
pub fn socket_path() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(|d| PathBuf::from(d).join("ears.sock"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/ears.sock"))
}

/// Owns the server task and all its accepted connections. Dropping it cancels
/// the server; `shutdown` additionally waits for socket/lock cleanup.
#[must_use = "keep the server alive for the lifetime of the IPC owner"]
pub struct IpcServer {
    task: JoinHandle<()>,
}

impl IpcServer {
    pub async fn shutdown(mut self) {
        self.task.abort();
        let _ = (&mut self.task).await;
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Start an event server. Duplicate starts log an error and leave the owner intact.
pub fn start_ipc_server_at(
    path: PathBuf,
    mut rx: broadcast::Receiver<StreamingEvent>,
) -> IpcServer {
    IpcServer {
        task: tokio::spawn(async move {
            let socket = match OwnedSocket::bind(path.clone()).await {
                Ok(socket) => socket,
                Err(e) => {
                    error!("Failed to bind IPC socket at {}: {}", path.display(), e);
                    return;
                }
            };
            info!("IPC server listening on {}", path.display());
            let mut clients = JoinSet::new();
            loop {
                tokio::select! {
                    result = socket.listener.accept() => match result {
                        Ok((stream, _)) => { clients.spawn(handle_client(stream, rx.resubscribe())); }
                        Err(e) => { error!("IPC accept error: {}", e); break; }
                    },
                    result = rx.recv() => {
                        if matches!(result, Err(broadcast::error::RecvError::Closed)) { break; }
                    },
                    Some(_) = clients.join_next(), if !clients.is_empty() => {}
                }
            }
        }),
    }
}

/// Start the event server at the default socket path.
pub fn start_ipc_server(rx: broadcast::Receiver<StreamingEvent>) -> IpcServer {
    start_ipc_server_at(socket_path(), rx)
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
    /// Switch typing into the focused window on/off (`typing-on|off|toggle|status`).
    Typing {
        request: crate::typing_switch::TypingRequest,
        respond: tokio::sync::oneshot::Sender<String>,
    },
}

/// Start the command server at the default socket path.
pub fn start_cmd_server(cmd_tx: tokio::sync::mpsc::UnboundedSender<EarsCommand>) -> IpcServer {
    start_cmd_server_at(cmd_socket_path(), cmd_tx)
}

/// Start a command server at an explicit path, with the same ownership rules as events.
pub fn start_cmd_server_at(
    sock_path: PathBuf,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<EarsCommand>,
) -> IpcServer {
    IpcServer {
        task: tokio::spawn(async move {
            let socket = match OwnedSocket::bind(sock_path.clone()).await {
                Ok(socket) => socket,
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
            let mut clients = JoinSet::new();
            loop {
                tokio::select! {
                    _ = cmd_tx.closed() => break,
                    Some(_) = clients.join_next(), if !clients.is_empty() => {},
                    result = socket.listener.accept() => match result {
                    Ok((mut stream, _)) => {
                        let tx = cmd_tx.clone();
                        clients.spawn(async move {
                            use tokio::io::{AsyncBufReadExt, BufReader};
                            let (reader, mut writer) = stream.split();
                            let mut lines = BufReader::new(reader).lines();
                            while let Ok(Some(line)) = lines.next_line().await {
                                let verb = line.trim();
                                let response = match verb {
                                    v if crate::typing_switch::TypingRequest::from_command(v)
                                        .is_some() =>
                                    {
                                        let request =
                                            crate::typing_switch::TypingRequest::from_command(v)
                                                .expect("checked above");
                                        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
                                        let _ = tx.send(EarsCommand::Typing {
                                            request,
                                            respond: resp_tx,
                                        });
                                        resp_rx
                                            .await
                                            .unwrap_or_else(|_| "error:internal".to_string())
                                    }
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
                    Err(e) => { error!("Command accept error: {}", e); break; }
                    }
                }
            }
        }),
    }
}

/// Send a command to a running ears instance. Returns the response.
pub async fn send_command(cmd: &str) -> anyhow::Result<String> {
    send_command_at(&cmd_socket_path(), cmd).await
}

/// Send a command to an explicitly selected instance.
pub async fn send_command_at(sock_path: &std::path::Path, cmd: &str) -> anyhow::Result<String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
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

#[cfg(test)]
mod tests;
