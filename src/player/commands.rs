//! Bounded command mailbox. No native calls, UI callbacks or waits for capacity
//! are allowed while its short queue mutex is held.
use crate::{ipc::Command, player::limits::PATH_TEXT_BYTES};
use crossbeam_channel::{Receiver, Sender, bounded};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub const CAPACITY: usize = 64;
pub const DISPATCH_BUDGET: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendError {
    Full,
    Disconnected,
    PayloadTooLarge,
}

impl SendError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Full => {
                "Плеер занят: слишком много команд. Последнее действие не принято — повторите его чуть позже"
            }
            Self::Disconnected => "Плеер недоступен. Перезапустите приложение",
            Self::PayloadTooLarge => "Слишком длинный путь к файлу. Команда не добавлена в очередь",
        }
    }
}

#[derive(Default)]
struct Queue {
    commands: VecDeque<Command>,
    render_ready: bool,
    video_window: Option<usize>,
}

struct Shared {
    queue: Mutex<Queue>,
    quit: AtomicBool,
    alive: AtomicBool,
}

#[derive(Clone)]
pub struct CommandSender {
    shared: Arc<Shared>,
    errors: Sender<SendError>,
}

pub struct CommandReceiver(Arc<Shared>);

pub fn channel() -> (CommandSender, CommandReceiver, Receiver<SendError>) {
    let shared = Arc::new(Shared {
        queue: Mutex::new(Queue::default()),
        quit: AtomicBool::new(false),
        alive: AtomicBool::new(true),
    });
    // Error reporting must not create an unbounded second queue on overload.
    let (errors, notices) = bounded(1);
    (
        CommandSender {
            shared: shared.clone(),
            errors,
        },
        CommandReceiver(shared),
        notices,
    )
}

impl CommandSender {
    pub fn send(&self, command: Command) -> Result<(), SendError> {
        let result = self.submit(command);
        if let Err(error) = result {
            let _ = self.errors.try_send(error);
        }
        result
    }

    fn submit(&self, command: Command) -> Result<(), SendError> {
        if !self.shared.alive.load(Ordering::Acquire) {
            return Err(SendError::Disconnected);
        }
        if !payload_is_bounded(&command) {
            return Err(SendError::PayloadTooLarge);
        }
        if matches!(command, Command::Quit) {
            self.shared.quit.store(true, Ordering::Release);
            return Ok(());
        }
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !self.shared.alive.load(Ordering::Acquire) || self.shared.quit.load(Ordering::Acquire) {
            return Err(SendError::Disconnected);
        }
        match command {
            Command::RenderApiReady => queue.render_ready = true,
            Command::SetVideoWindow(handle) => queue.video_window = Some(handle),
            command => {
                if queue
                    .commands
                    .back()
                    .is_some_and(|previous| replaces(previous, &command))
                {
                    *queue.commands.back_mut().expect("nonempty queue") = command;
                } else if queue.commands.len() < CAPACITY {
                    queue.commands.push_back(command);
                } else {
                    return Err(SendError::Full);
                }
            }
        }
        Ok(())
    }
}

fn payload_is_bounded(command: &Command) -> bool {
    match command {
        Command::OpenFile(path) | Command::OpenFolder(path) | Command::LoadSubtitle(path) => {
            path.len() <= PATH_TEXT_BYTES
        }
        _ => true,
    }
}

impl CommandReceiver {
    pub fn quit_requested(&self) -> bool {
        self.0.quit.load(Ordering::Acquire)
    }

    pub fn try_recv(&self, output_ready: bool) -> Option<Command> {
        if self.quit_requested() {
            return Some(Command::Quit);
        }
        let mut queue = self
            .0
            .queue
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if queue.render_ready {
            queue.render_ready = false;
            return Some(Command::RenderApiReady);
        }
        if let Some(handle) = queue.video_window.take() {
            return Some(Command::SetVideoWindow(handle));
        }
        // Leave media commands in this same bounded queue until output is ready.
        // Commands behind them must not jump across a file boundary either.
        if !output_ready && queue.commands.front().is_some_and(requires_output) {
            return None;
        }
        queue.commands.pop_front()
    }
}

impl Drop for CommandReceiver {
    fn drop(&mut self) {
        self.0.alive.store(false, Ordering::Release);
        let mut queue = self
            .0
            .queue
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        queue.commands.clear();
        queue.video_window = None;
        queue.render_ready = false;
    }
}

fn requires_output(command: &Command) -> bool {
    matches!(
        command,
        Command::OpenFile(_)
            | Command::OpenFolder(_)
            | Command::NextVideo
            | Command::PreviousVideo
            | Command::PlayIndex(_)
            | Command::RemoveIndex(_)
    )
}

fn replaces(previous: &Command, next: &Command) -> bool {
    // Absolute values only, adjacent only. Relative deltas/toggles/steps have
    // side effects; summing them changes clamping and ordering semantics.
    matches!(
        (previous, next),
        (Command::Seek(_), Command::Seek(_))
            | (Command::SetVolume(_), Command::SetVolume(_))
            | (Command::SetSpeed(_), Command::SetSpeed(_))
            | (Command::SetDarkTheme(_), Command::SetDarkTheme(_))
            | (Command::RequestMediaInfo, Command::RequestMediaInfo)
    )
}

#[cfg(test)]
mod tests;
