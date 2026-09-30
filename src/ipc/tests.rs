use super::*;
use std::os::unix::fs::MetadataExt;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::net::UnixStream;

async fn connect(path: &std::path::Path) -> UnixStream {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(stream) = UnixStream::connect(path).await {
                break stream;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("server did not bind")
}

#[tokio::test]
async fn duplicate_owner_and_legacy_listener_are_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.sock");
    let first = OwnedSocket::bind(path.clone()).await.unwrap();
    let inode = std::fs::metadata(&path).unwrap().ino();
    assert!(
        matches!(OwnedSocket::bind(path.clone()).await, Err(e) if e.kind() == std::io::ErrorKind::AddrInUse)
    );
    assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
    UnixStream::connect(&path).await.unwrap();
    drop(first);
    assert!(!path.exists());

    let legacy = tokio::net::UnixListener::bind(&path).unwrap();
    let inode = std::fs::metadata(&path).unwrap().ino();
    assert!(
        matches!(OwnedSocket::bind(path.clone()).await, Err(e) if e.kind() == std::io::ErrorKind::AddrInUse)
    );
    assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
    drop(legacy);
    // A dead listener's pathname is reclaimed under the lock.
    let replacement = OwnedSocket::bind(path.clone()).await.unwrap();
    drop(replacement);
    assert!(!path.exists());
}

#[tokio::test]
async fn shutdown_does_not_unlink_a_replacement_or_regular_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.sock");
    std::fs::write(&path, "keep me").unwrap();
    assert!(OwnedSocket::bind(path.clone()).await.is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep me");
    std::fs::remove_file(&path).unwrap();
    let first = OwnedSocket::bind(path.clone()).await.unwrap();
    std::fs::remove_file(&path).unwrap();
    let replacement = tokio::net::UnixListener::bind(&path).unwrap();
    drop(first);
    assert!(path.exists());
    UnixStream::connect(&path).await.unwrap();
    drop(replacement);
}

#[tokio::test]
async fn event_delivery_shutdown_and_immediate_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.sock");
    let (tx, rx) = broadcast::channel(8);
    let server = start_ipc_server_at(path.clone(), rx);
    let mut client = BufReader::new(connect(&path).await);
    tokio::time::timeout(Duration::from_secs(2), async {
        while tx.receiver_count() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tx.send(StreamingEvent::SpeechStarted).unwrap();
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(line, "\"SpeechStarted\"\n");
    server.shutdown().await;
    assert!(!path.exists());
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), client.read_u8())
            .await
            .unwrap()
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::UnexpectedEof
    );
    let replacement = start_ipc_server_at(path.clone(), tx.subscribe());
    let _client = connect(&path).await;
    replacement.shutdown().await;
    assert!(!path.exists());
}

#[tokio::test]
async fn command_routing_and_duplicate_start_do_not_replace_owner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("commands.sock");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let server = start_cmd_server_at(path.clone(), tx);
    drop(connect(&path).await);
    let (other_tx, _other_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut duplicate = start_cmd_server_at(path.clone(), other_tx);
    tokio::time::timeout(Duration::from_secs(2), &mut duplicate.task)
        .await
        .unwrap()
        .unwrap();
    let sender_path = path.clone();
    let request =
        tokio::spawn(async move { send_command_at(&sender_path, "toggle-auto-enter").await });
    let command = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let EarsCommand::ToggleAutoEnter { respond } = command else {
        panic!("unexpected command")
    };
    respond.send("on".into()).unwrap();
    assert_eq!(request.await.unwrap().unwrap(), "on");
    assert_eq!(
        send_command_at(&path, "unknown").await.unwrap(),
        "error:unknown-command"
    );
    server.shutdown().await;
    assert!(!path.exists());
}
