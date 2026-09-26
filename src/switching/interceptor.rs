use super::{CommandSink, Direction, SwitchMode};
use crate::input::{KeyEvent, Keyboard, KeyboardInterceptor, Vk};

/// Turns Win+Space / Win+Alt+Space (+Shift for backwards) into layout commands.
///
/// Win itself is never swallowed, only Space. Because the shell then sees Win pressed "alone", it would open
/// the Start menu on release; so, like AutoHotkey (hook.cpp, sDisguiseNextMenu / KeyEventMenuMask), an
/// unassigned key is tapped right before the release reaches the system, making Win look used in a combination.
///
/// Key state is read from the keyboard rather than tracked from events: a release can be missed (lock screen,
/// secure desktop, a hook timeout), and remembered state would then stay wrong for good. Inside the hook the
/// keyboard still reports the state from before the event being processed.
pub struct LayoutHotkeyInterceptor<K: Keyboard, S: CommandSink> {
    keyboard: K,
    commands: S,
    win_needs_mask: bool,
    space_swallowed: bool,
}

impl<K: Keyboard, S: CommandSink> LayoutHotkeyInterceptor<K, S> {
    pub fn new(keyboard: K, commands: S) -> Self {
        Self {
            keyboard,
            commands,
            win_needs_mask: false,
            space_swallowed: false,
        }
    }

    fn is_win_held(&self) -> bool {
        self.keyboard.is_down(Vk::LEFT_WIN) || self.keyboard.is_down(Vk::RIGHT_WIN)
    }

    fn on_win(&mut self, event: KeyEvent) -> bool {
        let other = if event.key == Vk::LEFT_WIN { Vk::RIGHT_WIN } else { Vk::LEFT_WIN };
        if !event.is_up {
            // A fresh press, not autorepeat: forget a mask left over from a hold whose release was missed.
            if !self.is_win_held() {
                self.win_needs_mask = false;
            }
        } else if self.win_needs_mask && !self.keyboard.is_down(other) {
            self.win_needs_mask = false;
            self.keyboard.tap(Vk::UNASSIGNED);
            self.commands.end_session();
        }
        false
    }

    fn on_space(&mut self, event: KeyEvent) -> bool {
        if event.is_up {
            return std::mem::take(&mut self.space_swallowed);
        }
        // Autorepeat of a swallowed press; after a missed release Space is up and this is a fresh press.
        if self.space_swallowed && self.keyboard.is_down(Vk::SPACE) {
            return true;
        }
        self.space_swallowed = false;

        // Ctrl is excluded because Win+Ctrl+Space is a system shortcut and AltGr arrives as LeftCtrl+RightAlt.
        if !self.is_win_held() || self.keyboard.is_down(Vk::CONTROL) {
            return false;
        }

        self.space_swallowed = true;
        self.win_needs_mask = true;
        let direction = if self.keyboard.is_down(Vk::SHIFT) { Direction::Backward } else { Direction::Forward };
        let mode = if self.keyboard.is_down(Vk::ALT) { SwitchMode::All } else { SwitchMode::Required };
        self.commands.cycle(direction, mode);
        true
    }

    fn on_modifier(&mut self, event: KeyEvent) -> bool {
        // A lone modifier release while Win is held and undisguised opens Start immediately. Windows itself sends
        // such a Ctrl release when switching to or from an AltGr layout (see AutoHotkey 1.1.27.01), and the Alt of
        // Win+Alt+Space would otherwise also activate the focused window's menu bar.
        if event.is_up && self.win_needs_mask && self.is_win_held() {
            self.keyboard.tap(Vk::UNASSIGNED);
        }
        false
    }
}

impl<K: Keyboard, S: CommandSink> KeyboardInterceptor for LayoutHotkeyInterceptor<K, S> {
    fn intercept(&mut self, event: KeyEvent) -> bool {
        match event.key {
            key if key.is_win() => self.on_win(event),
            Vk::SPACE => self.on_space(event),
            key if key.is_modifier() => self.on_modifier(event),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::rc::Rc;

    use super::*;

    #[derive(Default)]
    struct State {
        down: HashSet<Vk>,
        taps: Vec<Vk>,
        commands: Vec<String>,
    }

    #[derive(Clone, Default)]
    struct Fake(Rc<RefCell<State>>);

    // Tests are single-threaded; Send is only required by the trait bounds.
    unsafe impl Send for Fake {}

    impl Keyboard for Fake {
        fn is_down(&self, key: Vk) -> bool {
            let down = &self.0.borrow().down;
            let any = |a: Vk, b: Vk| down.contains(&a) || down.contains(&b);
            match key {
                Vk::SHIFT => any(Vk::LEFT_SHIFT, Vk::RIGHT_SHIFT),
                Vk::CONTROL => any(Vk::LEFT_CONTROL, Vk::RIGHT_CONTROL),
                Vk::ALT => any(Vk::LEFT_ALT, Vk::RIGHT_ALT),
                _ => down.contains(&key),
            }
        }

        fn tap(&self, key: Vk) {
            self.0.borrow_mut().taps.push(key);
        }
    }

    impl CommandSink for Fake {
        fn cycle(&self, direction: Direction, mode: SwitchMode) {
            self.0.borrow_mut().commands.push(format!("cycle {direction:?} {mode:?}"));
        }

        fn end_session(&self) {
            self.0.borrow_mut().commands.push("end".into());
        }
    }

    struct Harness {
        fake: Fake,
        interceptor: LayoutHotkeyInterceptor<Fake, Fake>,
    }

    impl Harness {
        fn new() -> Self {
            let fake = Fake::default();
            Self { interceptor: LayoutHotkeyInterceptor::new(fake.clone(), fake.clone()), fake }
        }

        /// Like the real hook, the interceptor sees the keyboard state from before the event.
        fn press(&mut self, key: Vk) -> bool {
            let swallowed = self.interceptor.intercept(KeyEvent { key, is_up: false });
            self.fake.0.borrow_mut().down.insert(key);
            swallowed
        }

        fn release(&mut self, key: Vk) -> bool {
            let swallowed = self.interceptor.intercept(KeyEvent { key, is_up: true });
            self.fake.0.borrow_mut().down.remove(&key);
            swallowed
        }

        /// A release the hook never sees, e.g. while the lock screen is up.
        fn lose_release(&mut self, key: Vk) {
            self.fake.0.borrow_mut().down.remove(&key);
        }

        fn taps(&self) -> Vec<Vk> {
            self.fake.0.borrow().taps.clone()
        }

        fn commands(&self) -> Vec<String> {
            self.fake.0.borrow().commands.clone()
        }
    }

    #[test]
    fn win_space_swallows_space_and_cycles_required() {
        let mut h = Harness::new();
        h.press(Vk::LEFT_WIN);
        assert!(h.press(Vk::SPACE));
        assert!(h.release(Vk::SPACE));
        assert_eq!(h.commands(), ["cycle Forward Required"]);
    }

    #[test]
    fn win_is_never_swallowed_and_its_release_is_masked_once() {
        let mut h = Harness::new();
        assert!(!h.press(Vk::LEFT_WIN));
        h.press(Vk::SPACE);
        h.release(Vk::SPACE);
        assert!(!h.release(Vk::LEFT_WIN));
        assert_eq!(h.taps(), [Vk::UNASSIGNED]);
        assert_eq!(h.commands().last().map(String::as_str), Some("end"));
    }

    #[test]
    fn missed_win_release_does_not_mask_the_next_plain_win_tap() {
        let mut h = Harness::new();
        h.press(Vk::LEFT_WIN);
        h.press(Vk::SPACE);
        h.release(Vk::SPACE);
        h.lose_release(Vk::LEFT_WIN);

        h.press(Vk::LEFT_WIN);
        h.release(Vk::LEFT_WIN);
        assert!(h.taps().is_empty());
    }

    #[test]
    fn missed_win_release_on_one_side_does_not_block_the_other() {
        let mut h = Harness::new();
        h.press(Vk::RIGHT_WIN);
        h.lose_release(Vk::RIGHT_WIN);

        h.press(Vk::LEFT_WIN);
        h.press(Vk::SPACE);
        h.release(Vk::SPACE);
        h.release(Vk::LEFT_WIN);
        assert_eq!(h.taps(), [Vk::UNASSIGNED]);
        assert_eq!(h.commands().last().map(String::as_str), Some("end"));
    }

    #[test]
    fn missed_space_release_does_not_swallow_the_next_space() {
        let mut h = Harness::new();
        h.press(Vk::LEFT_WIN);
        h.press(Vk::SPACE);
        h.lose_release(Vk::SPACE);
        h.release(Vk::LEFT_WIN);

        assert!(!h.press(Vk::SPACE));
        h.release(Vk::SPACE);
        h.press(Vk::LEFT_WIN);
        assert!(h.press(Vk::SPACE));
        assert_eq!(h.commands().iter().filter(|c| c.starts_with("cycle")).count(), 2);
    }

    #[test]
    fn plain_win_tap_is_not_masked() {
        let mut h = Harness::new();
        h.press(Vk::LEFT_WIN);
        h.release(Vk::LEFT_WIN);
        assert!(h.taps().is_empty());
        assert!(h.commands().is_empty());
    }

    #[test]
    fn alt_selects_all_layouts_and_shift_reverses() {
        let mut h = Harness::new();
        h.press(Vk::LEFT_WIN);
        h.press(Vk::LEFT_ALT);
        h.press(Vk::LEFT_SHIFT);
        h.press(Vk::SPACE);
        assert_eq!(h.commands(), ["cycle Backward All"]);
    }

    #[test]
    fn win_ctrl_space_is_left_to_the_system() {
        let mut h = Harness::new();
        h.press(Vk::LEFT_WIN);
        h.press(Vk::LEFT_CONTROL);
        assert!(!h.press(Vk::SPACE));
        assert!(h.commands().is_empty());
    }

    #[test]
    fn space_without_win_passes_through() {
        assert!(!Harness::new().press(Vk::SPACE));
    }

    #[test]
    fn held_space_autorepeat_is_swallowed_without_extra_switches() {
        let mut h = Harness::new();
        h.press(Vk::LEFT_WIN);
        h.press(Vk::SPACE);
        assert!(h.press(Vk::SPACE));
        assert_eq!(h.commands().len(), 1);
    }

    #[test]
    fn lone_modifier_release_while_win_held_after_switch_is_masked() {
        let mut h = Harness::new();
        h.press(Vk::LEFT_WIN);
        h.press(Vk::SPACE);
        h.release(Vk::SPACE);
        assert!(!h.release(Vk::LEFT_CONTROL));
        assert_eq!(h.taps(), [Vk::UNASSIGNED]);
    }

    #[test]
    fn modifier_release_before_any_switch_is_not_masked() {
        let mut h = Harness::new();
        h.press(Vk::LEFT_WIN);
        h.release(Vk::LEFT_CONTROL);
        assert!(h.taps().is_empty());
    }
}
