use std::io;
use std::thread::sleep;
use std::time::Duration;

use super::win32::{
    self, INPUT_KEYBOARD, Input, KEYEVENTF_KEYUP, KeyBdInput, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, Point, VK_A, VK_D, VK_SPACE, Word,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeldKey {
    None,
    A,
    D,
}

fn cursor_pos() -> io::Result<Point> {
    let mut p = Point { x: 0, y: 0 };
    unsafe {
        if win32::GetCursorPos(&mut p) == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(p)
        }
    }
}

pub fn key_event(vk: Word, up: bool) {
    let input = Input {
        r#type: INPUT_KEYBOARD,
        _pad: 0,
        ki: KeyBdInput {
            w_vk: vk,
            w_scan: 0,
            dw_flags: if up { KEYEVENTF_KEYUP } else { 0 },
            time: 0,
            dw_extra_info: 0,
        },
        _union_pad: 0,
    };
    unsafe {
        let sent = win32::SendInput(1, &input, std::mem::size_of::<Input>() as i32);
        if sent == 0 {}
    }
}

pub fn click_screen(x: i32, y: i32) -> io::Result<Point> {
    let mut actual = cursor_pos().unwrap_or(Point { x: 0, y: 0 });

    for _ in 0..3 {
        unsafe {
            if win32::SetCursorPos(x, y) == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        sleep(Duration::from_millis(40));

        actual = cursor_pos()?;
        if (actual.x - x).abs() <= 2 && (actual.y - y).abs() <= 2 {
            unsafe {
                win32::mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
            }
            sleep(Duration::from_millis(30));
            unsafe {
                win32::mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
            }
            sleep(Duration::from_millis(120));
            return Ok(actual);
        }
    }

    Err(io::Error::other(format!(
        "cursor did not move to click target: target={x},{y} actual={},{}",
        actual.x, actual.y
    )))
}

pub fn release_keys(held: &mut HeldKey, force_all: bool) {
    match *held {
        HeldKey::A => key_event(VK_A, true),
        HeldKey::D => key_event(VK_D, true),
        HeldKey::None if force_all => {
            key_event(VK_A, true);
            key_event(VK_D, true);
        }
        HeldKey::None => {}
    }
    *held = HeldKey::None;
}

pub fn hold_key(held: &mut HeldKey, key: HeldKey) {
    if *held == key {
        return;
    }
    release_keys(held, false);
    match key {
        HeldKey::A => key_event(VK_A, false),
        HeldKey::D => key_event(VK_D, false),
        HeldKey::None => {}
    }
    *held = key;
}

pub fn tap_space() {
    key_event(VK_SPACE, false);
    sleep(Duration::from_millis(70));
    key_event(VK_SPACE, true);
}
