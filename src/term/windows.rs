//! Windows console: VT output, key-record input, ctrl handler wakeups
//! (UI-020, UI-021, UI-025, UI-029, UI-032, PERF-028).

use std::ffi::c_void;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, BOOL, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::WriteFile;
use windows_sys::Win32::System::Console::{
    GetConsoleMode, GetConsoleOutputCP, GetConsoleScreenBufferInfo, GetNumberOfConsoleInputEvents,
    GetStdHandle, ReadConsoleInputW, SetConsoleCtrlHandler, SetConsoleMode, SetConsoleOutputCP,
    CONSOLE_SCREEN_BUFFER_INFO, DISABLE_NEWLINE_AUTO_RETURN, ENABLE_ECHO_INPUT, ENABLE_EXTENDED_FLAGS,
    ENABLE_LINE_INPUT, ENABLE_MOUSE_INPUT, ENABLE_PROCESSED_INPUT, ENABLE_PROCESSED_OUTPUT,
    ENABLE_QUICK_EDIT_MODE, ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
    ENABLE_WINDOW_INPUT, INPUT_RECORD, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent, Sleep, WaitForMultipleObjects};

use super::{Event, Key};

const ENTER: &[u8] = b"\x1b[?1049h\x1b[?25l\x1b[?7l";
const LEAVE: &[u8] = b"\x1b[0m\x1b[?7h\x1b[?25h\x1b[?1049l";

const CP_UTF8: u32 = 65001;
const INFINITE: u32 = 0xFFFF_FFFF;
const WAIT_TIMEOUT: u32 = 258;
const KEY_EVENT: u16 = 1;
const WINDOW_BUFFER_SIZE_EVENT: u16 = 4;
const NREC: usize = 32;

// Original state in statics so the panic hook can restore it (UI-021).
static HIN: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static HOUT: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static ORIG_IN: AtomicU32 = AtomicU32::new(0);
static ORIG_OUT: AtomicU32 = AtomicU32::new(0);
static ORIG_CP: AtomicU32 = AtomicU32::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static QUIT: AtomicBool = AtomicBool::new(false);
static WAKE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

pub struct Console {
    hin: HANDLE,
    hout: HANDLE,
    wake: HANDLE,
    recs: [INPUT_RECORD; NREC],
    /// Decoded keys not yet returned (reused, never shrinks).
    keys: Vec<Key>,
    pos: usize,
    resized: bool,
    hi_surrogate: u16,
}

unsafe extern "system" fn ctrl_handler(kind: u32) -> BOOL {
    // Ctrl-C/Break (only when processed input is on), close, logoff, shutdown -> Quit.
    QUIT.store(true, Ordering::SeqCst);
    let w = WAKE.load(Ordering::SeqCst);
    if !w.is_null() {
        SetEvent(w);
    }
    if kind >= 2 {
        // Close/logoff/shutdown: the process is killed when this returns, so give the
        // main thread time to restore and exit.
        Sleep(4000);
    }
    1
}

fn bad(h: HANDLE) -> bool {
    h.is_null() || h == INVALID_HANDLE_VALUE
}

fn write_handle(h: HANDLE, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        let mut n = 0u32;
        let len = bytes.len().min(u32::MAX as usize) as u32;
        // SAFETY: valid handle, buffer pointer and length; synchronous write.
        let ok = unsafe { WriteFile(h, bytes.as_ptr(), len, &mut n, std::ptr::null_mut()) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        bytes = &bytes[n as usize..];
    }
    Ok(())
}

/// Restore console modes and code page, leave the alternate screen. Idempotent.
pub fn restore_global() {
    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    let hin = HIN.load(Ordering::SeqCst);
    let hout = HOUT.load(Ordering::SeqCst);
    let _ = write_handle(hout, LEAVE);
    // SAFETY: restoring values read from these handles at open.
    unsafe {
        SetConsoleMode(hin, ORIG_IN.load(Ordering::SeqCst));
        SetConsoleMode(hout, ORIG_OUT.load(Ordering::SeqCst));
        SetConsoleOutputCP(ORIG_CP.load(Ordering::SeqCst));
    }
}

impl Console {
    pub fn open() -> io::Result<Console> {
        // SAFETY: plain Win32 calls; out-pointers are valid locals.
        unsafe {
            let hin = GetStdHandle(STD_INPUT_HANDLE);
            let hout = GetStdHandle(STD_OUTPUT_HANDLE);
            if bad(hin) || bad(hout) {
                return Err(io::Error::new(io::ErrorKind::Other, "no console"));
            }
            let (mut mi, mut mo) = (0u32, 0u32);
            if GetConsoleMode(hin, &mut mi) == 0 || GetConsoleMode(hout, &mut mo) == 0 {
                return Err(io::Error::last_os_error());
            }
            let wake = CreateEventW(std::ptr::null(), 0, 0, std::ptr::null());
            if wake.is_null() {
                return Err(io::Error::last_os_error());
            }
            HIN.store(hin, Ordering::SeqCst);
            HOUT.store(hout, Ordering::SeqCst);
            ORIG_IN.store(mi, Ordering::SeqCst);
            ORIG_OUT.store(mo, Ordering::SeqCst);
            ORIG_CP.store(GetConsoleOutputCP(), Ordering::SeqCst);
            WAKE.store(wake, Ordering::SeqCst);
            SetConsoleCtrlHandler(Some(ctrl_handler), 1);
            let c = Console {
                hin,
                hout,
                wake,
                recs: std::mem::zeroed(),
                keys: Vec::with_capacity(NREC * 2),
                pos: 0,
                resized: false,
                hi_surrogate: 0,
            };
            c.enter()?;
            Ok(c)
        }
    }

    /// UI-020 step 3: raw-ish input, VT output, UTF-8 code page, alternate screen.
    fn enter(&self) -> io::Result<()> {
        let mi = ORIG_IN.load(Ordering::SeqCst);
        let mo = ORIG_OUT.load(Ordering::SeqCst);
        let inm = (mi
            & !(ENABLE_LINE_INPUT
                | ENABLE_ECHO_INPUT
                | ENABLE_PROCESSED_INPUT
                | ENABLE_QUICK_EDIT_MODE
                | ENABLE_MOUSE_INPUT
                | ENABLE_VIRTUAL_TERMINAL_INPUT))
            | ENABLE_WINDOW_INPUT
            | ENABLE_EXTENDED_FLAGS;
        let outm = mo | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING | DISABLE_NEWLINE_AUTO_RETURN;
        // SAFETY: valid console handles.
        unsafe {
            if SetConsoleMode(self.hout, outm) == 0 {
                return Err(io::Error::last_os_error());
            }
            SetConsoleMode(self.hin, inm);
            SetConsoleOutputCP(CP_UTF8);
        }
        ACTIVE.store(true, Ordering::SeqCst);
        write_handle(self.hout, ENTER)
    }

    /// (cols, rows) of the visible window; 80x24 if unknown.
    pub fn size(&self) -> (u16, u16) {
        // SAFETY: info is plain data filled by the call.
        let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
        if unsafe { GetConsoleScreenBufferInfo(self.hout, &mut info) } == 0 {
            return (80, 24);
        }
        let r = info.srWindow;
        let w = (r.Right as i32 - r.Left as i32 + 1).clamp(1, u16::MAX as i32) as u16;
        let h = (r.Bottom as i32 - r.Top as i32 + 1).clamp(1, u16::MAX as i32) as u16;
        (w, h)
    }

    pub fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
        write_handle(self.hout, bytes)
    }

    /// Next already-decoded event, if any. Quit first, then keys, then one collapsed
    /// resize (UI-031).
    pub fn next_event(&mut self) -> Option<Event> {
        if QUIT.swap(false, Ordering::SeqCst) {
            return Some(Event::Quit);
        }
        if self.pos < self.keys.len() {
            let k = self.keys[self.pos];
            self.pos += 1;
            if self.pos == self.keys.len() {
                self.keys.clear();
                self.pos = 0;
            }
            return Some(Event::Key(k));
        }
        if self.resized {
            self.resized = false;
            let (w, h) = self.size();
            return Some(Event::Resize(w, h));
        }
        None
    }

    /// The single blocking wait (UI-025, PERF-028). Returns false on timeout.
    pub fn wait(&mut self, timeout: Option<Duration>) -> io::Result<bool> {
        let ms = match timeout {
            None => INFINITE,
            Some(d) => d.as_nanos().div_ceil(1_000_000).min((INFINITE - 1) as u128) as u32,
        };
        let hs = [self.hin, self.wake];
        // SAFETY: two valid handles.
        let r = unsafe { WaitForMultipleObjects(2, hs.as_ptr(), 0, ms) };
        match r {
            0 => self.read_input(),
            1 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(io::Error::last_os_error()),
        }
    }

    fn read_input(&mut self) -> io::Result<bool> {
        let mut avail = 0u32;
        let mut got = 0u32;
        // SAFETY: recs holds NREC records; out-pointers are valid locals.
        unsafe {
            if GetNumberOfConsoleInputEvents(self.hin, &mut avail) == 0 {
                return Err(io::Error::last_os_error());
            }
            if avail == 0 {
                return Ok(true);
            }
            if ReadConsoleInputW(self.hin, self.recs.as_mut_ptr(), avail.min(NREC as u32), &mut got) == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        for i in 0..got as usize {
            let rec = self.recs[i];
            match rec.EventType {
                KEY_EVENT => {
                    // SAFETY: EventType says the union holds a key event.
                    let k = unsafe { rec.Event.KeyEvent };
                    if k.bKeyDown == 0 {
                        continue;
                    }
                    // SAFETY: reading the UTF-16 member of a plain-data union.
                    let ch = unsafe { k.uChar.UnicodeChar };
                    if let Some(key) = self.translate(k.wVirtualKeyCode, ch, k.dwControlKeyState) {
                        for _ in 0..k.wRepeatCount.clamp(1, 8) {
                            self.keys.push(key);
                        }
                    }
                }
                // UI-029: resize wakes the wait.
                WINDOW_BUFFER_SIZE_EVENT => self.resized = true,
                _ => {}
            }
        }
        Ok(true)
    }

    /// Map a key record to the same `Key` values the Unix decoder produces (UI-032).
    fn translate(&mut self, vk: u16, ch: u16, state: u32) -> Option<Key> {
        let shift = state & 0x10 != 0;
        let ctrl = state & 0x0c != 0;
        let left_alt = state & 0x02 != 0;
        let key = match vk {
            0x08 => Key::Backspace,
            0x09 => Key::Tab,
            0x0d => Key::Enter,
            0x1b => Key::Esc,
            0x21 => Key::PageUp,
            0x22 => Key::PageDown,
            0x23 => Key::End,
            0x24 => Key::Home,
            0x25 if shift => Key::ShiftLeft,
            0x25 => Key::Left,
            0x26 => Key::Up,
            0x27 if shift => Key::ShiftRight,
            0x27 => Key::Right,
            0x28 => Key::Down,
            0x2d => Key::Insert,
            0x2e => Key::Delete,
            0x70..=0x7b => Key::F((vk - 0x6f) as u8),
            _ => return self.translate_char(ch, ctrl, left_alt),
        };
        Some(key)
    }

    fn translate_char(&mut self, ch: u16, ctrl: bool, left_alt: bool) -> Option<Key> {
        let cp = match ch {
            0 => return None,
            0xd800..=0xdbff => {
                self.hi_surrogate = ch;
                return None;
            }
            0xdc00..=0xdfff => {
                let hi = std::mem::take(&mut self.hi_surrogate);
                if hi == 0 {
                    return None;
                }
                0x10000 + (((hi as u32) - 0xd800) << 10) + ((ch as u32) - 0xdc00)
            }
            _ => ch as u32,
        };
        let c = char::from_u32(cp)?;
        match cp {
            // UI-023: Ctrl-Z is ignored on Windows.
            0x1a => None,
            0x01..=0x19 => Some(Key::Ctrl((b'a' + cp as u8 - 1) as char)),
            0x00..=0x1f | 0x7f..=0x9f => None,
            // Alt+key has no key variant; drop it like the Unix decoder (AltGr sets Ctrl).
            _ if left_alt && !ctrl => None,
            _ => Some(Key::Char(c)),
        }
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        restore_global();
        WAKE.store(std::ptr::null_mut(), Ordering::SeqCst);
        // SAFETY: closing the event we created; the ctrl handler no longer sees it.
        unsafe { CloseHandle(self.wake) };
    }
}
