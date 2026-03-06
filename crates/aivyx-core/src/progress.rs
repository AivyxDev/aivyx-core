//! Generic progress reporting trait.
//!
//! [`ProgressSink`] provides a type-parameterized interface for emitting
//! progress events. Domain-specific crates define their own event types
//! (e.g., `ProgressEvent` in `aivyx-task`) while sharing the same sink
//! abstraction.

use async_trait::async_trait;

use crate::error::Result;

/// Trait for consuming typed progress events.
///
/// Parameterized over the event type `E` so that each domain (tasks,
/// teams, etc.) can define its own event enum while reusing the same
/// sink infrastructure.
#[async_trait]
pub trait ProgressSink<E: Send + Sync + 'static>: Send + Sync {
    /// Emit a progress event.
    async fn emit(&self, event: E) -> Result<()>;
}

/// A progress sink that silently discards all events.
///
/// Useful as a default when no progress reporting is needed.
pub struct NoopProgressSink<E> {
    _marker: std::marker::PhantomData<E>,
}

impl<E> NoopProgressSink<E> {
    /// Create a new no-op progress sink.
    pub fn new() -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }
}

impl<E> Default for NoopProgressSink<E> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<E: Send + Sync + 'static> ProgressSink<E> for NoopProgressSink<E> {
    async fn emit(&self, _event: E) -> Result<()> {
        Ok(())
    }
}

/// A progress sink backed by a `tokio::sync::mpsc` channel.
pub struct ChannelProgressSink<E> {
    tx: tokio::sync::mpsc::Sender<E>,
}

impl<E> ChannelProgressSink<E> {
    /// Create a new channel-backed progress sink.
    pub fn new(tx: tokio::sync::mpsc::Sender<E>) -> Self {
        Self { tx }
    }
}

#[async_trait]
impl<E: Send + Sync + 'static> ProgressSink<E> for ChannelProgressSink<E> {
    async fn emit(&self, event: E) -> Result<()> {
        self.tx
            .send(event)
            .await
            .map_err(|e| crate::AivyxError::Other(format!("progress channel closed: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_sink_succeeds() {
        let sink = NoopProgressSink::<String>::new();
        sink.emit("hello".to_string()).await.unwrap();
    }

    #[tokio::test]
    async fn channel_sink_delivers() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let sink = ChannelProgressSink::new(tx);

        sink.emit(42u32).await.unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received, 42);
    }
}
