use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::oneshot;

pub struct NotifyOnDrop {
    pub notify: Option<oneshot::Sender<()>>,
}

impl Drop for NotifyOnDrop {
    fn drop(&mut self) {
        if let Some(tx) = self.notify.take() {
            let _ = tx.send(());
        }
    }
}

pub struct TrackedStream<S> {
    pub stream: S,
    pub _notify: NotifyOnDrop,
}

impl<S: Stream + Unpin> Stream for TrackedStream<S> {
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.stream).poll_next(cx)
    }
}
