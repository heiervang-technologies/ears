//! Cancellation and shutdown shared by microphone and WebSocket pipelines.
use std::{future::Future, time::Duration};
use tokio::{sync::watch, task::JoinHandle};

/// Drop in-flight processing when shutdown is requested or its owner disappears.
/// The processing future owns the engine and capture, so cancellation also drops
/// ghost connections and capture guards rather than draining queued audio.
pub async fn until_shutdown(mut shutdown: watch::Receiver<bool>, work: impl Future<Output = ()>) {
    let stopped = async {
        loop {
            if *shutdown.borrow_and_update() || shutdown.changed().await.is_err() {
                break;
            }
        }
    };
    tokio::select! {
        biased;
        _ = stopped => {},
        _ = work => {},
    }
}

/// Wait for cooperative shutdown, then abort a stuck asynchronous task.
/// Synchronous blocking code cannot be forcibly cancelled by Tokio; even that
/// case must not leave the interface awaiting the task without a deadline.
pub async fn join_stopped(handle: JoinHandle<()>) {
    join_with_deadlines(handle, Duration::from_secs(2), Duration::from_secs(1)).await;
}

async fn join_with_deadlines(mut handle: JoinHandle<()>, grace: Duration, abort_grace: Duration) {
    match tokio::time::timeout(grace, &mut handle).await {
        Ok(Ok(())) => return,
        Ok(Err(error)) => {
            tracing::warn!(%error, "Pipeline task failed during shutdown");
            return;
        }
        Err(_) => tracing::warn!("Pipeline shutdown deadline exceeded; aborting task"),
    }
    handle.abort();
    match tokio::time::timeout(abort_grace, &mut handle).await {
        Ok(Err(error)) if error.is_cancelled() => {}
        Ok(Err(error)) => tracing::warn!(%error, "Pipeline task failed during abort"),
        Ok(Ok(())) => {}
        Err(_) => tracing::error!("Pipeline is blocked in synchronous code after abort"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    struct OnDrop(Option<oneshot::Sender<()>>);
    impl Drop for OnDrop {
        fn drop(&mut self) {
            let _ = self.0.take().unwrap().send(());
        }
    }

    #[tokio::test]
    async fn shutdown_cancels_inflight_work_and_drops_resources() {
        let (tx, rx) = watch::channel(false);
        let (started_tx, started_rx) = oneshot::channel();
        let (dropped_tx, dropped_rx) = oneshot::channel();
        let task = tokio::spawn(until_shutdown(rx, async move {
            let _resource = OnDrop(Some(dropped_tx));
            started_tx.send(()).unwrap();
            std::future::pending::<()>().await;
            panic!("cancelled transcription must not commit");
        }));
        started_rx.await.unwrap();
        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        dropped_rx.await.unwrap();
    }

    #[tokio::test]
    async fn stopped_owner_does_not_start_queued_audio() {
        for closed in [false, true] {
            let (tx, rx) = watch::channel(false);
            if closed {
                drop(tx);
            } else {
                tx.send(true).unwrap();
            }
            until_shutdown(rx, async {
                panic!("queued work started after shutdown");
            })
            .await;
        }
    }

    #[tokio::test]
    async fn join_deadline_aborts_unresponsive_async_task() {
        let (started_tx, started_rx) = oneshot::channel();
        let (dropped_tx, dropped_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _resource = OnDrop(Some(dropped_tx));
            started_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        started_rx.await.unwrap();
        tokio::time::timeout(
            Duration::from_secs(1),
            join_with_deadlines(task, Duration::from_millis(10), Duration::from_millis(100)),
        )
        .await
        .unwrap();
        dropped_rx.await.unwrap();
    }
}
