use std::fs::File;
use std::io::{LineWriter, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Instant;

use crate::input::{KeyEvent, KeyboardInterceptor};
use crate::layouts::LayoutInfo;
use crate::switching::{SwitchMode, SwitchObserver};

/// Non-blocking file log: callers only enqueue, a background thread writes.
pub struct DiagnosticLog {
    sender: Sender<String>,
    started: Instant,
}

impl DiagnosticLog {
    pub fn create(path: &Path) -> std::io::Result<Arc<Self>> {
        let mut file = LineWriter::new(File::create(path)?);
        let (sender, receiver) = mpsc::channel::<String>();
        thread::Builder::new().name("diagnostic-log".into()).spawn(move || {
            for line in receiver {
                let _ = writeln!(file, "{line}");
            }
        })?;
        Ok(Arc::new(Self { sender, started: Instant::now() }))
    }

    pub fn write(&self, message: &str) {
        let elapsed = self.started.elapsed().as_secs_f64() * 1000.0;
        let _ = self.sender.send(format!("{elapsed:10.1} ms  {message}"));
    }
}

pub struct LoggingInterceptor<I> {
    inner: I,
    log: Arc<DiagnosticLog>,
}

impl<I: KeyboardInterceptor> LoggingInterceptor<I> {
    pub fn new(inner: I, log: Arc<DiagnosticLog>) -> Self {
        Self { inner, log }
    }
}

impl<I: KeyboardInterceptor> KeyboardInterceptor for LoggingInterceptor<I> {
    fn intercept(&mut self, event: KeyEvent) -> bool {
        let swallowed = self.inner.intercept(event);
        let action = if event.is_up { "up" } else { "down" };
        let result = if swallowed { "swallowed" } else { "passed" };
        self.log.write(&format!("{:<12} {action:<4} {result}", format!("{:?}", event.key)));
        swallowed
    }
}

pub struct LoggingObserver<O> {
    inner: O,
    log: Arc<DiagnosticLog>,
}

impl<O: SwitchObserver> LoggingObserver<O> {
    pub fn new(inner: O, log: Arc<DiagnosticLog>) -> Self {
        Self { inner, log }
    }
}

impl<O: SwitchObserver> SwitchObserver for LoggingObserver<O> {
    fn layout_selected(&self, layout: isize, mode: SwitchMode) {
        self.log.write(&format!("switch {mode:?} -> {}", LayoutInfo::describe(layout).short_name));
        self.inner.layout_selected(layout, mode);
    }

    fn session_ended(&self) {
        self.log.write("session ended");
        self.inner.session_ended();
    }
}
