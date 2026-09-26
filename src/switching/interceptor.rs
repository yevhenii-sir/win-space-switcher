use super::{CommandSink, Direction, SwitchMode};
use crate::input::{KeyEvent, Keyboard, KeyboardInterceptor, Vk};

/// Turns Win+Space / Win+Alt+Space (+Shift for backwards) into layout commands.
///
/// Win itself is never swallowed, only Space. Because the shell then sees Win pressed "alone", it would open
/// the Start menu on release; so, like AutoHotkey (hook.cpp, sDisguiseNextMenu / KeyEventMenuMask), an
/// unassigned key is tapped right before the release reaches the system, making Win look used in a combination.
pub struct LayoutHotkeyInterceptor<K: Keyboard, S: CommandSink> {
    keyboard: K,
    commands: S,
    left_win_down: bool,
    right_win_down: bool,
    win_needs_mask: bool,
    space_swallowed: bool,
}

impl<K: Keyboard, S: CommandSink> LayoutHotkeyInterceptor<K, S> {
    pub fn new(keyboard: K, commands: S) -> Self {
        Self {
            keyboard,
            commands,
            left_win_down: false,
            right_win_down: false,
            win_needs_mask: false,
            space_swallowed: false,
        }
    }

    fn is_win_held(&self) -> bool {
        self.left_win_down || self.right_win_down
    }

    fn on_win(&mut self, event: KeyEvent) -> bool {
        let was_held = self.is_win_held();
        if event.key == Vk::LEFT_WIN {
            self.left_win_down = !event.is_up;
        } else {
            self.right_win_down = !event.is_up;
        }

        if !event.is_up {
            if !was_held {
                self.win_needs_mask = false;
            }
        } else if self.win_needs_mask && !self.is_win_held() {
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
        if self.space_swallowed {
            return true;
        }

        // Physical state instead of is_win_held(): a Win release can be missed (e.g. while the lock screen is up).
        // Ctrl is excluded because Win+Ctrl+Space is a system shortcut and AltGr arrives as LeftCtrl+RightAlt.
        let win_down = self.keyboard.is_down(Vk::LEFT_WIN) || self.keyboard.is_down(Vk::RIGHT_WIN);
        if !win_down || self.keyboard.is_down(Vk::CONTROL) {
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

        fn press(&mut self, key: Vk) -> bool {
            self.fake.0.borrow_mut().down.insert(key);
            self.interceptor.intercept(KeyEvent { key, is_up: false })
        }

        fn release(&mut self, key: Vk) -> bool {
            let swallowed = self.interceptor.intercept(KeyEvent { key, is_up: true });
            self.fake.0.borrow_mut().down.remove(&key);
            swallowed
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
