use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY,
};

use super::{INJECTION_MARKER, Keyboard, Vk};

pub struct Win32Keyboard;

impl Keyboard for Win32Keyboard {
    fn is_down(&self, key: Vk) -> bool {
        (unsafe { GetAsyncKeyState(i32::from(key.0)) } as u16 & 0x8000) != 0
    }

    fn tap(&self, key: Vk) {
        let inputs = [keyboard_input(key, false), keyboard_input(key, true)];
        unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    }
}

fn keyboard_input(key: Vk, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(key.0),
                wScan: 0,
                dwFlags: if key_up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: INJECTION_MARKER,
            },
        },
    }
}
