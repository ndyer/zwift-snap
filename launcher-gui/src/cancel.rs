//! Cooperative cancellation shared by the UI and launcher worker.

use std::io;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Clone, Default)]
pub struct Cancellation {
    state: Arc<(Mutex<bool>, Condvar)>,
}

impl Cancellation {
    pub fn cancel(&self) {
        let (state, changed) = &*self.state;
        *state.lock().expect("cancellation mutex poisoned") = true;
        changed.notify_all();
    }

    pub fn is_cancelled(&self) -> bool {
        *self.state.0.lock().expect("cancellation mutex poisoned")
    }

    pub fn check(&self) -> io::Result<()> {
        if self.is_cancelled() {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Launcher cancelled",
            ))
        } else {
            Ok(())
        }
    }

    /// Wait for a duration, returning early if cancellation is requested.
    pub fn wait(&self, duration: Duration) -> io::Result<()> {
        let (state, changed) = &*self.state;
        let guard = state.lock().expect("cancellation mutex poisoned");
        let (guard, _) = changed
            .wait_timeout_while(guard, duration, |cancelled| !*cancelled)
            .expect("cancellation mutex poisoned");
        if *guard {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Launcher cancelled",
            ))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_wakes_waiters_and_is_sticky() {
        let cancellation = Cancellation::default();
        let worker_cancel = cancellation.clone();
        let (ready, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            ready.send(()).unwrap();
            worker_cancel.wait(Duration::from_secs(30))
        });
        receiver.recv().unwrap();
        cancellation.cancel();
        assert_eq!(
            worker.join().unwrap().unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        assert_eq!(
            cancellation.check().unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        cancellation.cancel();
    }
}
