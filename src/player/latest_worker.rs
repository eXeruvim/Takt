//! Bounded single-thread worker where the newest request replaces older work.

use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use anyhow::Context;

pub(super) type CancelFlag = Arc<AtomicBool>;
pub(super) type WakeCallback = Arc<dyn Fn() + Send + Sync + 'static>;
pub(super) type LatestLoader<Request, Output> =
    Arc<dyn Fn(Request, CancelFlag) -> Option<Output> + Send + Sync + 'static>;

struct WorkerState<Request, Output> {
    pending: Option<Request>,
    completed: Option<Output>,
    active_cancel: Option<CancelFlag>,
    shutdown: bool,
}

impl<Request, Output> Default for WorkerState<Request, Output> {
    fn default() -> Self {
        Self {
            pending: None,
            completed: None,
            active_cancel: None,
            shutdown: false,
        }
    }
}

struct Shared<Request, Output> {
    state: Mutex<WorkerState<Request, Output>>,
    ready: Condvar,
}

pub(super) struct LatestWorker<Request: Send + 'static, Output: Send + 'static> {
    shared: Arc<Shared<Request, Output>>,
    thread: Option<JoinHandle<()>>,
    panic_message: &'static str,
}

impl<Request: Send + 'static, Output: Send + 'static> LatestWorker<Request, Output> {
    pub(super) fn new(
        thread_name: &str,
        spawn_error: &'static str,
        panic_message: &'static str,
        wake: WakeCallback,
        loader: LatestLoader<Request, Output>,
    ) -> anyhow::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(WorkerState::default()),
            ready: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name(thread_name.to_owned())
            .spawn(move || run_worker(worker_shared, wake, loader))
            .context(spawn_error)?;
        Ok(Self {
            shared,
            thread: Some(thread),
            panic_message,
        })
    }

    pub(super) fn submit(&self, request: Request) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.shutdown {
            return;
        }
        cancel_active(&mut state);
        state.pending = Some(request);
        state.completed = None;
        self.shared.ready.notify_one();
    }

    pub(super) fn cancel(&self) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cancel_active(&mut state);
        state.pending = None;
        state.completed = None;
    }

    pub(super) fn take_result(&self) -> Option<Output> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .completed
            .take()
    }
}

impl<Request: Send + 'static, Output: Send + 'static> Drop for LatestWorker<Request, Output> {
    fn drop(&mut self) {
        {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.shutdown = true;
            cancel_active(&mut state);
            state.pending = None;
            state.completed = None;
            self.shared.ready.notify_one();
        }
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            eprintln!("{}", self.panic_message);
        }
    }
}

fn cancel_active<Request, Output>(state: &mut WorkerState<Request, Output>) {
    if let Some(cancelled) = state.active_cancel.as_ref() {
        cancelled.store(true, Ordering::Release);
    }
}

fn run_worker<Request: Send + 'static, Output: Send + 'static>(
    shared: Arc<Shared<Request, Output>>,
    wake: WakeCallback,
    loader: LatestLoader<Request, Output>,
) {
    loop {
        let (request, cancelled) = {
            let mut state = shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            while state.pending.is_none() && !state.shutdown {
                state = shared
                    .ready
                    .wait(state)
                    .unwrap_or_else(|error| error.into_inner());
            }
            if state.shutdown {
                return;
            }
            let request = state.pending.take().expect("pending request checked");
            let cancelled = Arc::new(AtomicBool::new(false));
            state.active_cancel = Some(Arc::clone(&cancelled));
            (request, cancelled)
        };

        let output = loader(request, Arc::clone(&cancelled));
        let should_wake = {
            let mut state = shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state
                .active_cancel
                .as_ref()
                .is_some_and(|active| Arc::ptr_eq(active, &cancelled))
            {
                state.active_cancel = None;
            }
            if state.shutdown {
                return;
            }
            if cancelled.load(Ordering::Acquire) || state.pending.is_some() {
                false
            } else if let Some(output) = output {
                state.completed = Some(output);
                true
            } else {
                false
            }
        };
        if should_wake {
            wake();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        time::{Duration, Instant},
    };

    use super::LatestWorker;

    #[test]
    fn newer_request_cancels_and_replaces_active_work() {
        let (started_tx, started_rx) = mpsc::channel();
        let worker = LatestWorker::new(
            "oxide-latest-test",
            "latest worker test did not start",
            "latest worker test panicked",
            Arc::new(|| {}),
            Arc::new(move |request: u32, cancelled: Arc<AtomicBool>| {
                if request == 1 {
                    let _ = started_tx.send(());
                    while !cancelled.load(Ordering::Acquire) {
                        std::thread::yield_now();
                    }
                    return None;
                }
                Some(request)
            }),
        )
        .unwrap();

        worker.submit(1);
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.submit(2);

        let deadline = Instant::now() + Duration::from_secs(1);
        let result = loop {
            if let Some(result) = worker.take_result() {
                break result;
            }
            assert!(Instant::now() < deadline, "latest work did not finish");
            std::thread::yield_now();
        };
        assert_eq!(result, 2);
    }

    #[test]
    fn dropping_worker_cancels_and_joins_active_work() {
        let (started_tx, started_rx) = mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let finished_in_loader = Arc::clone(&finished);
        let worker = LatestWorker::new(
            "oxide-latest-drop-test",
            "latest worker drop test did not start",
            "latest worker drop test panicked",
            Arc::new(|| {}),
            Arc::new(move |_: (), cancelled: Arc<AtomicBool>| {
                let _ = started_tx.send(());
                while !cancelled.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
                finished_in_loader.store(true, Ordering::Release);
                None::<()>
            }),
        )
        .unwrap();

        worker.submit(());
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        drop(worker);
        assert!(finished.load(Ordering::Acquire));
    }
}
