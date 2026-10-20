//! Event-driven delivery with exactly one outstanding UI callback per bridge.
//! Bounded source channels alone do not bound Slint's pending callback queue.
use crossbeam_channel::{Receiver, Sender, bounded, select_biased};
use std::{
    io,
    thread::{self, JoinHandle},
};

pub(crate) const PLAYER_EVENT_CAPACITY: usize = 32;
pub(crate) const CONVERTER_EVENT_CAPACITY: usize = 32;
pub(crate) const BATCH_LIMIT: usize = 8;

/// The UI callback owns this until it has applied the entire batch. Dropping it
/// also acknowledges early returns, destroyed callbacks, and panic unwinding.
pub(super) struct Completion(Sender<()>);

impl Drop for Completion {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}

pub(super) struct Bridge {
    stop: Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.stop.try_send(());
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            eprintln!("UI event bridge panicked during shutdown");
        }
    }
}

/// `dispatch` must schedule work nonblockingly and transfer Completion into that
/// callback. Returning false means the event loop is gone. Only adjacent state
/// samples may replace each other; never coalesce across file/lifecycle events.
pub(super) fn spawn<T: Send + 'static>(
    rx: Receiver<T>,
    replaces: impl Fn(&T, &T) -> bool + Send + 'static,
    mut dispatch: impl FnMut(Vec<T>, Completion) -> bool + Send + 'static,
) -> io::Result<Bridge> {
    let (stop, worker_stop) = bounded(1);
    let worker = thread::Builder::new()
        .name("takt-ui-events".into())
        .spawn(move || {
            loop {
                // No timer/polling while idle. Shutdown wins over a saturated
                // event stream, both here and while awaiting the UI callback.
                let first = select_biased! {
                    recv(worker_stop) -> _ => break,
                    recv(rx) -> event => match event {
                        Ok(event) => event,
                        Err(_) => break,
                    },
                };
                let mut batch = vec![first];
                // Bound inputs consumed, not just coalesced batch length: a producer
                // flooding samples must not keep the bridge in an endless drain.
                for event in rx.try_iter().take(BATCH_LIMIT - 1) {
                    if batch
                        .last()
                        .is_some_and(|previous| replaces(previous, &event))
                    {
                        *batch.last_mut().expect("nonempty batch") = event;
                    } else {
                        batch.push(event);
                    }
                }
                let (done, completed) = bounded(1);
                if !dispatch(batch, Completion(done)) {
                    break;
                }
                select_biased! {
                    recv(worker_stop) -> _ => break,
                    recv(completed) -> _ => {},
                }
            }
            // Dropping rx wakes producers blocked on a full source channel.
        })?;
    Ok(Bridge {
        stop,
        worker: Some(worker),
    })
}

#[cfg(test)]
mod tests;
