use std::future::IntoFuture;

use futures::Future;
use tokio::sync::oneshot::{self, Receiver};

/// Represents a value that may not yet be available.
///
/// It provides (spawn)[Promise::spawn] and (spawn_blocking)[Promise::spawn_blocking]
/// which allows to spawn a new task or blocking thread that will resolve into the value.
#[derive(Debug)]
pub(crate) enum Promise<T> {
    Pending(Receiver<T>),
    Resolved(T),
}

#[allow(unused)]
impl<T> Promise<T> {
    /// Spawns a the future in a new task and returns the promise.
    ///
    /// When the task completes, the promise will be resolved.
    pub(crate) fn spawn<F>(future: F) -> Self
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();

        tokio::spawn(async move {
            tx.send(future.await);
        });

        Self::Pending(rx)
    }

    /// Spawns the blocking future and returns the promise.
    ///
    /// When the task completes, the promise will be resolved.
    pub(crate) fn spawn_blocking<F>(f: F) -> Self
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();

        tokio::task::spawn_blocking(|| {
            tx.send(f());
        });

        Self::Pending(rx)
    }

    /// Tell wether the promise resolved.
    #[inline]
    pub(crate) fn is_resolved(&self) -> bool {
        matches!(self, Self::Resolved(_))
    }

    /// Get the resolved value, panics if not resolved.
    #[inline]
    pub(crate) fn unwrap(self) -> T {
        match self {
            Self::Resolved(v) => v,
            Self::Pending(_) => panic!("unwrapped an unresoleved promise"),
        }
    }

    /// Get the resolved value, panics if not resolved.
    #[inline]
    pub(crate) fn unwrap_ref(&self) -> &T {
        match self {
            Self::Resolved(v) => v,
            Self::Pending(_) => panic!("unwrapped an unresoleved promise"),
        }
    }

    /// Get the resolved value, panics if the promise not resolved.
    #[inline]
    pub(crate) fn unwrap_mut(&mut self) -> &mut T {
        match self {
            Self::Resolved(v) => v,
            Self::Pending(_) => panic!("unwrapped an unresoleved promise"),
        }
    }

    /// Attempts to get the value. If the promise is not resolved, tries to resolve it.
    pub(crate) fn value(&mut self) -> Option<&T> {
        let rx = match self {
            Self::Resolved(v) => return (&*v).into(),
            Self::Pending(tx) => tx,
        };

        if let Ok(v) = rx.try_recv() {
            *self = Self::Resolved(v);
            Some(self.unwrap_ref())
        } else {
            None
        }
    }
}

impl<T, F> From<F> for Promise<T>
where
    F: IntoFuture<Output = T>,
    F::IntoFuture: Send + 'static,
    T: Send + 'static,
{
    fn from(f: F) -> Self {
        Self::spawn(f.into_future())
    }
}
