mod cycler;
mod interceptor;
mod service;

pub use interceptor::LayoutHotkeyInterceptor;
pub use service::{SwitchObserver, SwitchService};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchMode {
    /// Win+Space: only required layouts.
    Required,
    /// Win+Alt+Space: every installed layout.
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
}

impl Direction {
    fn step(self) -> isize {
        match self {
            Self::Forward => 1,
            Self::Backward => -1,
        }
    }
}

/// Receives switch requests from the hook thread; implementations must not block.
pub trait CommandSink: Send {
    fn cycle(&self, direction: Direction, mode: SwitchMode);

    /// Win was released after one or more switches.
    fn end_session(&self);
}
