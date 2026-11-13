//! Text clipboard (copy, cut, paste in the path box, the filter, …) with
//! Windows' own functions. egui's clipboard support is left off on purpose:
//! it would also decode images from the clipboard, which a disk analyzer
//! doesn't need. Text only, nothing else is read.

use std::ffi::c_void;

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn OpenClipboard(owner: *mut c_void) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn GetClipboardData(format: u32) -> *mut c_void;
    fn SetClipboardData(format: u32, mem: *mut c_void) -> *mut c_void;
    fn IsClipboardFormatAvailable(format: u32) -> i32;
    fn GetKeyState(key: i32) -> i16;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
    fn GlobalLock(mem: *mut c_void) -> *mut c_void;
    fn GlobalUnlock(mem: *mut c_void) -> i32;
    fn GlobalFree(mem: *mut c_void) -> *mut c_void;
    fn GlobalSize(mem: *mut c_void) -> usize;
}

const CF_UNICODETEXT: u32 = 13;
const GMEM_MOVEABLE: u32 = 0x0002;

/// Opens the clipboard, retrying briefly: another program may hold it.
fn open() -> bool {
    for _ in 0..10 {
        // SAFETY: plain Win32 call; null owner = the current task.
        if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    false
}

/// The clipboard's text, if it holds any (at most 1 MB, line breaks as \n).
pub fn get() -> Option<String> {
    // SAFETY: the clipboard is opened and closed around the read; the data
    // handle is only used while locked, within its reported size.
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 || !open() {
            return None;
        }
        let mut out = None;
        let h = GetClipboardData(CF_UNICODETEXT);
        if !h.is_null() {
            let p = GlobalLock(h) as *const u16;
            if !p.is_null() {
                let max = (GlobalSize(h) / 2).min(512 * 1024);
                let len = (0..max).position(|i| *p.add(i) == 0).unwrap_or(max);
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)).replace("\r\n", "\n"));
                GlobalUnlock(h);
            }
        }
        CloseClipboard();
        out.filter(|s| !s.is_empty())
    }
}

/// Puts `text` on the clipboard.
pub fn set(text: &str) {
    let wide: Vec<u16> = text.replace("\r\n", "\n").replace('\n', "\r\n").encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: the buffer is allocated, filled within its size and handed to
    // the clipboard (which then owns it); freed only if the hand-over fails.
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
        if mem.is_null() {
            return;
        }
        let p = GlobalLock(mem) as *mut u16;
        if p.is_null() {
            GlobalFree(mem);
            return;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
        GlobalUnlock(mem);
        if !open() {
            GlobalFree(mem);
            return;
        }
        EmptyClipboard();
        if SetClipboardData(CF_UNICODETEXT, mem).is_null() {
            GlobalFree(mem);
        }
        CloseClipboard();
    }
}

/// Is the key down right now (as far as this window's messages go)?
fn down(vk: i32) -> bool {
    // SAFETY: plain Win32 call.
    unsafe { GetKeyState(vk) < 0 }
}

/// Paste shortcut state, to catch the moment it is pressed (Ctrl+V or
/// Shift+Insert). egui-winit swallows these keys when its own clipboard is
/// off, so they are read here.
#[derive(Default)]
pub struct PasteKey {
    was_down: bool,
}

impl PasteKey {
    /// True once per press.
    pub fn pressed(&mut self) -> bool {
        const VK_SHIFT: i32 = 0x10;
        const VK_CONTROL: i32 = 0x11;
        const VK_MENU: i32 = 0x12;
        const VK_INSERT: i32 = 0x2D;
        let now = (down(VK_CONTROL) && !down(VK_MENU) && down(b'V' as i32)) || (down(VK_SHIFT) && down(VK_INSERT));
        let edge = now && !self.was_down;
        self.was_down = now;
        edge
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "uses the real clipboard (puts back the text that was there)"]
    fn round_trip() {
        let before = get();
        set("DiskRoot clip test\nline 2 · ü");
        let got = get();
        if let Some(b) = &before {
            set(b);
        }
        assert_eq!(got.as_deref(), Some("DiskRoot clip test\nline 2 · ü"));
    }
}
