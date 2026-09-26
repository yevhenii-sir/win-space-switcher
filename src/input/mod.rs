mod hook;
mod keyboard;

pub use hook::{HookHandle, KeyboardHook};
pub use keyboard::Win32Keyboard;

use std::fmt;

/// `dwExtraInfo` stamped on our own injected input so the hook can ignore it.
pub const INJECTION_MARKER: usize = 0x5753_5357;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Vk(pub u16);

impl Vk {
    pub const SHIFT: Vk = Vk(0x10);
    pub const CONTROL: Vk = Vk(0x11);
    pub const ALT: Vk = Vk(0x12);
    pub const SPACE: Vk = Vk(0x20);
    pub const LEFT_WIN: Vk = Vk(0x5B);
    pub const RIGHT_WIN: Vk = Vk(0x5C);
    pub const LEFT_SHIFT: Vk = Vk(0xA0);
    pub const RIGHT_SHIFT: Vk = Vk(0xA1);
    pub const LEFT_CONTROL: Vk = Vk(0xA2);
    pub const RIGHT_CONTROL: Vk = Vk(0xA3);
    pub const LEFT_ALT: Vk = Vk(0xA4);
    pub const RIGHT_ALT: Vk = Vk(0xA5);
    /// Not assigned to any key; safe to inject as a "menu mask" (AutoHotkey's #MenuMaskKey vkE8).
    pub const UNASSIGNED: Vk = Vk(0xE8);

    pub fn is_win(self) -> bool {
        self == Self::LEFT_WIN || self == Self::RIGHT_WIN
    }

    pub fn is_modifier(self) -> bool {
        (Self::LEFT_SHIFT.0..=Self::RIGHT_ALT.0).contains(&self.0)
    }
}

impl fmt::Debug for Vk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match *self {
            Self::SPACE => "Space",
            Self::LEFT_WIN => "LeftWin",
            Self::RIGHT_WIN => "RightWin",
            Self::LEFT_SHIFT => "LeftShift",
            Self::RIGHT_SHIFT => "RightShift",
            Self::LEFT_CONTROL => "LeftControl",
            Self::RIGHT_CONTROL => "RightControl",
            Self::LEFT_ALT => "LeftAlt",
            Self::RIGHT_ALT => "RightAlt",
            _ => return write!(f, "vk{:02X}", self.0),
        };
        f.write_str(name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Vk,
    pub is_up: bool,
}

pub trait Keyboard: Send {
    fn is_down(&self, key: Vk) -> bool;

    /// Injects a press and release of `key`, invisible to our own hook.
    fn tap(&self, key: Vk);
}

pub trait KeyboardInterceptor: Send {
    /// Called on the hook thread for every physical key event; must return quickly.
    /// Returns `true` to swallow the event so no application sees it.
    fn intercept(&mut self, event: KeyEvent) -> bool;
}
