use std::collections::HashSet;
use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use super::cycler::next_layout;
use super::{CommandSink, Direction, SwitchMode};
use crate::layouts::LayoutSystem;

/// Notified on the worker thread; implementations must not block for long.
pub trait SwitchObserver: Send {
    fn layout_selected(&self, layout: isize, mode: SwitchMode);
    fn session_ended(&self);
}

enum Command {
    Cycle(Direction, SwitchMode),
    EndSession,
}

type OptionalLayouts = Arc<Mutex<HashSet<isize>>>;

/// Executes layout commands on a dedicated worker thread, keeping all slow work off the hook thread.
pub struct SwitchService {
    sender: Option<Sender<Command>>,
    worker: Option<JoinHandle<()>>,
    optional: OptionalLayouts,
}

impl SwitchService {
    pub fn start(layouts: impl LayoutSystem + 'static, observer: impl SwitchObserver + 'static) -> Self {
        let (sender, receiver) = mpsc::channel();
        let optional = OptionalLayouts::default();
        let worker_optional = Arc::clone(&optional);
        let worker = thread::Builder::new()
            .name("layout-switcher".into())
            .spawn(move || {
                let worker = Worker::new(layouts, observer, worker_optional);
                // A dead worker would silently stop all switching; crash instead so the watchdog restarts us.
                if panic::catch_unwind(AssertUnwindSafe(|| worker.run(receiver))).is_err() {
                    std::process::abort();
                }
            })
            .expect("failed to spawn the layout switcher thread");

        Self { sender: Some(sender), worker: Some(worker), optional }
    }

    pub fn commands(&self) -> ChannelCommandSink {
        ChannelCommandSink(self.sender.clone().expect("service is running"))
    }

    /// Ends the current switch session, as a Win release would.
    pub fn end_session(&self) {
        self.commands().end_session();
    }

    pub fn set_optional_layouts(&self, layouts: HashSet<isize>) {
        *self.optional.lock().unwrap_or_else(PoisonError::into_inner) = layouts;
    }
}

impl Drop for SwitchService {
    /// The worker exits once every sender is gone, so the keyboard hook (holding the other sender)
    /// must be stopped first.
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub struct ChannelCommandSink(Sender<Command>);

impl CommandSink for ChannelCommandSink {
    fn cycle(&self, direction: Direction, mode: SwitchMode) {
        let _ = self.0.send(Command::Cycle(direction, mode));
    }

    fn end_session(&self) {
        let _ = self.0.send(Command::EndSession);
    }
}

struct Worker<L, O> {
    layouts: L,
    observer: O,
    optional: OptionalLayouts,
    // Windows apply a layout request asynchronously, so within one Win hold we continue from our own last
    // choice instead of re-reading a layout the window may not have switched to yet.
    session_layout: Option<isize>,
}

impl<L: LayoutSystem, O: SwitchObserver> Worker<L, O> {
    fn new(layouts: L, observer: O, optional: OptionalLayouts) -> Self {
        Self { layouts, observer, optional, session_layout: None }
    }

    fn run(mut self, commands: Receiver<Command>) {
        for command in commands {
            match command {
                Command::Cycle(direction, mode) => self.cycle(direction, mode),
                Command::EndSession => {
                    self.session_layout = None;
                    self.observer.session_ended();
                }
            }
        }
    }

    fn cycle(&mut self, direction: Direction, mode: SwitchMode) {
        let installed = self.layouts.installed();
        if installed.is_empty() {
            return;
        }

        let current = self.session_layout.unwrap_or_else(|| self.layouts.active());
        let next = {
            let optional = self.optional.lock().unwrap_or_else(PoisonError::into_inner);
            next_layout(&installed, current, direction, |layout| {
                mode == SwitchMode::All || !optional.contains(&layout)
            })
        };

        self.session_layout = Some(next);
        if next != current {
            self.layouts.activate(next);
        }
        self.observer.layout_selected(next, mode);
    }
}
