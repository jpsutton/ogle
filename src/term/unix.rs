//! Unix tty: termios raw mode, alternate screen, self-pipe signal wakeups
//! (UI-020, UI-021, UI-023, UI-025, UI-029, PERF-028).

use std::io;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use libc::c_int;

/// Alternate screen, hide cursor, no auto-wrap.
const ENTER: &[u8] = b"\x1b[?1049h\x1b[?25l\x1b[?7l";
/// Reset attributes, wrap on, cursor visible, main screen.
const LEAVE: &[u8] = b"\x1b[0m\x1b[?7h\x1b[?25h\x1b[?1049l";

pub const SIG_WINCH: u32 = 1;
pub const SIG_QUIT: u32 = 2;
pub const SIG_CONT: u32 = 4;

/// Original terminal state, kept in a static so the panic hook can restore it (UI-021).
struct Saved {
    in_fd: c_int,
    out_fd: c_int,
    orig: libc::termios,
}

static SAVED: OnceLock<Saved> = OnceLock::new();
static ACTIVE: AtomicBool = AtomicBool::new(false);
static SIG_FLAGS: AtomicU32 = AtomicU32::new(0);
static SIG_R: AtomicI32 = AtomicI32::new(-1);
static SIG_W: AtomicI32 = AtomicI32::new(-1);

pub enum Raw {
    Input(usize),
    /// Signal or interrupted wait; check `take_signals`.
    Wake,
    Timeout,
    /// The tty is gone (hangup or EOF).
    Eof,
}

pub struct Tty {
    in_fd: c_int,
    out_fd: c_int,
    own_fd: bool,
}

extern "C" fn on_signal(sig: c_int) {
    // Async-signal-safe: atomics and write(2) only.
    let bit = match sig {
        libc::SIGWINCH => SIG_WINCH,
        libc::SIGCONT => SIG_CONT,
        _ => SIG_QUIT,
    };
    SIG_FLAGS.fetch_or(bit, Ordering::SeqCst);
    let fd = SIG_W.load(Ordering::Relaxed);
    if fd >= 0 {
        // SAFETY: writing one byte from a static buffer to our own non-blocking pipe.
        // A full pipe (EAGAIN) is fine: a wakeup is already pending.
        unsafe { libc::write(fd, b"s".as_ptr().cast(), 1) };
    }
}

fn cvt(r: c_int) -> io::Result<c_int> {
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

fn set_nonblock_cloexec(fd: c_int) -> io::Result<()> {
    // SAFETY: fcntl on a valid fd we own.
    unsafe {
        let fl = cvt(libc::fcntl(fd, libc::F_GETFL))?;
        cvt(libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK))?;
        let fd_fl = cvt(libc::fcntl(fd, libc::F_GETFD))?;
        cvt(libc::fcntl(fd, libc::F_SETFD, fd_fl | libc::FD_CLOEXEC))?;
    }
    Ok(())
}

/// Create the self-pipe and install handlers once per process.
fn install_signals() -> io::Result<()> {
    if SIG_R.load(Ordering::Relaxed) >= 0 {
        return Ok(());
    }
    let mut fds = [0 as c_int; 2];
    // SAFETY: fds points to two c_ints.
    cvt(unsafe { libc::pipe(fds.as_mut_ptr()) })?;
    set_nonblock_cloexec(fds[0])?;
    set_nonblock_cloexec(fds[1])?;
    SIG_R.store(fds[0], Ordering::Relaxed);
    SIG_W.store(fds[1], Ordering::Relaxed);
    // UI-021, UI-023, UI-029.
    for sig in [libc::SIGWINCH, libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGCONT] {
        // SAFETY: zeroed sigaction is a valid starting value; the handler only uses
        // async-signal-safe operations.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_signal as extern "C" fn(c_int) as libc::sighandler_t;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            cvt(libc::sigaction(sig, &sa, std::ptr::null_mut()))?;
        }
    }
    Ok(())
}

fn write_fd(fd: c_int, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        // SAFETY: valid buffer pointer and length.
        let n = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        bytes = &bytes[n as usize..];
    }
    Ok(())
}

/// Restore the original termios and leave the UI screen state. Idempotent; safe to call
/// from the panic hook (UI-021).
pub fn restore_global() {
    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    if let Some(s) = SAVED.get() {
        let _ = write_fd(s.out_fd, LEAVE);
        // SAFETY: restoring a termios previously read from this fd.
        unsafe { libc::tcsetattr(s.in_fd, libc::TCSAFLUSH, &s.orig) };
    }
}

impl Tty {
    pub fn open() -> io::Result<Tty> {
        // SAFETY (this block and the isatty calls below): plain libc calls with a
        // NUL-terminated path and standard fds.
        let fd = unsafe { libc::open(b"/dev/tty\0".as_ptr().cast(), libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC) };
        let tty = if fd >= 0 {
            Tty { in_fd: fd, out_fd: fd, own_fd: true }
        } else if unsafe { libc::isatty(0) } == 1 {
            let out_fd = if unsafe { libc::isatty(1) } == 1 { 1 } else { 0 };
            Tty { in_fd: 0, out_fd, own_fd: false }
        } else {
            return Err(io::Error::new(io::ErrorKind::Other, "no terminal: /dev/tty unavailable and stdin is not a tty"));
        };
        // SAFETY: zeroed termios is filled in by tcgetattr before use.
        let mut orig: libc::termios = unsafe { std::mem::zeroed() };
        cvt(unsafe { libc::tcgetattr(tty.in_fd, &mut orig) })?;
        let _ = SAVED.set(Saved { in_fd: tty.in_fd, out_fd: tty.out_fd, orig });
        install_signals()?;
        tty.enter()?;
        Ok(tty)
    }

    /// Enter raw mode and the UI screen state (UI-020 step 3, UI-032).
    pub fn enter(&self) -> io::Result<()> {
        let Some(s) = SAVED.get() else {
            return Err(io::ErrorKind::NotFound.into());
        };
        let mut t = s.orig;
        t.c_iflag &= !(libc::BRKINT | libc::ICRNL | libc::INPCK | libc::ISTRIP | libc::IXON);
        t.c_oflag &= !libc::OPOST;
        t.c_cflag |= libc::CS8;
        // ISIG off: Ctrl-C and Ctrl-Z arrive as bytes (UI-023, UI-037).
        t.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG);
        t.c_cc[libc::VMIN] = 1;
        t.c_cc[libc::VTIME] = 0;
        // SAFETY: valid fd and termios.
        cvt(unsafe { libc::tcsetattr(self.in_fd, libc::TCSAFLUSH, &t) })?;
        ACTIVE.store(true, Ordering::SeqCst);
        write_fd(self.out_fd, ENTER)
    }

    /// (cols, rows) via TIOCGWINSZ; 80x24 if unknown.
    pub fn size(&self) -> (u16, u16) {
        // SAFETY: winsize is plain data filled by the ioctl.
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        let r = unsafe { libc::ioctl(self.out_fd, libc::TIOCGWINSZ as _, &mut ws) };
        if r < 0 || ws.ws_col == 0 || ws.ws_row == 0 {
            return (80, 24);
        }
        (ws.ws_col, ws.ws_row)
    }

    pub fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
        write_fd(self.out_fd, bytes)
    }

    /// Atomically take the pending signal bits.
    pub fn take_signals(&self) -> u32 {
        SIG_FLAGS.swap(0, Ordering::SeqCst)
    }

    /// Put back bits that were taken but not handled yet.
    pub fn requeue_signals(&self, bits: u32) {
        SIG_FLAGS.fetch_or(bits, Ordering::SeqCst);
    }

    /// UI-023: restore, stop with SIGTSTP, re-enter after SIGCONT.
    pub fn suspend(&self) -> io::Result<()> {
        restore_global();
        // SAFETY: raising a signal in our own process; default action stops it.
        unsafe { libc::raise(libc::SIGTSTP) };
        self.enter()?;
        // Our own SIGCONT was handled here; do not report it again.
        SIG_FLAGS.fetch_and(!SIG_CONT, Ordering::SeqCst);
        Ok(())
    }

    /// The single blocking wait (UI-025, PERF-028): tty fd and signal pipe in one poll().
    pub fn wait(&self, timeout: Option<Duration>, buf: &mut [u8]) -> io::Result<Raw> {
        let ms: c_int = match timeout {
            None => -1,
            Some(d) => d.as_nanos().div_ceil(1_000_000).min(c_int::MAX as u128) as c_int,
        };
        let sig_r = SIG_R.load(Ordering::Relaxed);
        let mut fds = [
            libc::pollfd { fd: self.in_fd, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: sig_r, events: libc::POLLIN, revents: 0 },
        ];
        // SAFETY: fds is a valid array of two pollfd.
        let r = unsafe { libc::poll(fds.as_mut_ptr(), 2, ms) };
        if r < 0 {
            let e = io::Error::last_os_error();
            return if e.kind() == io::ErrorKind::Interrupted { Ok(Raw::Wake) } else { Err(e) };
        }
        if r == 0 {
            return Ok(Raw::Timeout);
        }
        if fds[1].revents != 0 {
            let mut drain = [0u8; 64];
            // SAFETY: reading into a local buffer from our non-blocking pipe.
            while unsafe { libc::read(sig_r, drain.as_mut_ptr().cast(), drain.len()) } > 0 {}
        }
        let rev = fds[0].revents;
        if rev & libc::POLLNVAL != 0 {
            return Ok(Raw::Eof);
        }
        if rev & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 && !buf.is_empty() {
            // SAFETY: reading into the caller's buffer within its length.
            let n = unsafe { libc::read(self.in_fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n > 0 {
                return Ok(Raw::Input(n as usize));
            }
            if n == 0 {
                return Ok(Raw::Eof);
            }
            let e = io::Error::last_os_error();
            return match e.kind() {
                io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock => Ok(Raw::Wake),
                _ if e.raw_os_error() == Some(libc::EIO) => Ok(Raw::Eof),
                _ => Err(e),
            };
        }
        Ok(Raw::Wake)
    }
}

impl Drop for Tty {
    fn drop(&mut self) {
        restore_global();
        if self.own_fd {
            // SAFETY: closing the fd we opened.
            unsafe { libc::close(self.in_fd) };
        }
    }
}
