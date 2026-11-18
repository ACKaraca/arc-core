//! A bounded blocking executor for the futures `wgpu` returns.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use arc_base::{Error, Result};

use crate::error::codes;

/// Wakes the waiting thread.
struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `future` on the current thread until it completes or `timeout` elapses, which fails
/// with `gpu.timeout`. Native `wgpu` futures are ready when first polled; the bound protects
/// against a driver that never answers.
pub(crate) fn block_on<F: Future>(future: F, timeout: Duration) -> Result<F::Output> {
    let mut future = pin!(future);
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut context = Context::from_waker(&waker);
    let deadline = Instant::now() + timeout;
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return Ok(output);
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(Error::new(codes::TIMEOUT));
        }
        thread::park_timeout(deadline - now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_future_completes() {
        assert_eq!(block_on(async { 7 }, Duration::from_millis(10)).unwrap(), 7);
    }

    #[test]
    fn pending_future_times_out() {
        let error = block_on(std::future::pending::<()>(), Duration::from_millis(20)).unwrap_err();
        assert_eq!(error.code(), "gpu.timeout");
    }

    #[test]
    fn future_woken_from_another_thread_completes() {
        let (sender, receiver) = std::sync::mpsc::channel::<()>();
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let set = Arc::clone(&flag);
        let handle = thread::spawn(move || {
            receiver.recv().unwrap();
            set.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        sender.send(()).unwrap();
        handle.join().unwrap();
        let future = std::future::poll_fn(|_| {
            if flag.load(std::sync::atomic::Ordering::SeqCst) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        });
        block_on(future, Duration::from_secs(1)).unwrap();
    }
}
