//! The dense stand-in child of the demo's `DEMO_DENSE=1` mode: a full grid
//! of dense varied text with kitty images and light periodic traffic,
//! running on the slave pty after the driver re-execs this same binary with
//! `--dense-child`. It retransmits the images after every `SIGWINCH` like a
//! real TUI host does; image bytes come from a system icon PNG, as in the C
//! demo.

use std::fmt::Write as _;
use std::io::Write as _;
use std::os::raw::c_char;
use std::os::raw::c_void;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// The dense child's image source (a system icon PNG, as in the C demo).
const DENSE_IMAGE: &str = "/usr/share/icons/hicolor/512x512/apps/com.mitchellh.ghostty.png";

/// The dense child's `SIGWINCH` latch, the `volatile sig_atomic_t` of the C.
static DENSE_RESIZED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// The dense child's `SIGWINCH` handler: set the repaint latch, nothing else
/// (async-signal-safe). Ports `dense_on_winch`.
extern "C" fn dense_on_winch(_sig: std::os::raw::c_int) {
    DENSE_RESIZED.store(true, Ordering::Release);
}

/// RFC 4648 base64 (standard alphabet, padded) for the kitty payload: the
/// local replacement for the `glib::base64_encode` the GTK path used. The
/// RFC 4648 test vectors are asserted in the demo's test module.
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).map_or(0, |&byte| u32::from(byte));
        let b2 = chunk.get(2).map_or(0, |&byte| u32::from(byte));
        let n = (b0 << 16) | (b1 << 8) | b2;
        let quad = [
            TABLE[num_traits::cast::<u32, usize>((n >> 18) & 0x3f).expect("six bits")],
            TABLE[num_traits::cast::<u32, usize>((n >> 12) & 0x3f).expect("six bits")],
            if chunk.len() > 1 {
                TABLE[num_traits::cast::<u32, usize>((n >> 6) & 0x3f).expect("six bits")]
            } else {
                b'='
            },
            if chunk.len() > 2 {
                TABLE[num_traits::cast::<u32, usize>(n & 0x3f).expect("six bits")]
            } else {
                b'='
            },
        ];
        out.push_str(std::str::from_utf8(&quad).expect("table bytes are ascii"));
    }
    out
}

/// The dense child's kitty image transmission: the PNG base64-encoded into
/// four placements of distinct image ids, one per quadrant — a ~480 KB kitty
/// burst per repaint, the same order as the real host's art re-encode.
/// `q=1` so parse errors come back on stdin, where [`dump_stdin`] shows
/// them.
fn dense_send_image() {
    // The C capped its fixed buffer at 128 KiB; keep the cap so the burst
    // size stays the one the demo was tuned against.
    let Ok(raw) = std::fs::read(DENSE_IMAGE) else {
        return;
    };
    let raw = &raw[..raw.len().min(1 << 17)];
    let b64 = base64_encode(raw);

    let mut out = String::new();
    for id in 1..=4u32 {
        let slot = id - 1;
        let _ = write!(
            out,
            "\x1b[{};{}H\x1b_Gf=24,a=T,i={id},q=1,c=20,r=20;{}\x1b\\",
            2 + slot / 2 * 22,
            2 + slot % 2 * 25,
            b64
        );
    }
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(out.as_bytes());
    let _ = stdout.flush();
}

/// The dense child drains its own stdin (the panel side of the pty) so kitty
/// responses (`q=1` errors) reach the demo log instead of sitting unread:
/// non-blocking read, dumped to stderr. Ports `dump_stdin`.
fn dump_stdin() {
    // SAFETY: `F_GETFL`/`F_SETFL` on our own stdin descriptor, restored after
    // the drain; `read` on our own descriptor with a writable buffer.
    unsafe {
        let flags = libc::fcntl(0, libc::F_GETFL);
        if flags < 0 || libc::fcntl(0, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return;
        }
        let mut buf = [0u8; 256];
        loop {
            let n = libc::read(0, buf.as_mut_ptr().cast::<c_void>(), buf.len());
            if n > 0 {
                eprint!("child got {n} bytes: ");
                let _ = std::io::stderr().write_all(&buf[..n.cast_unsigned()]);
                eprintln!();
            } else {
                if n < 0 {
                    let error = std::io::Error::last_os_error();
                    if error.kind() != std::io::ErrorKind::WouldBlock {
                        eprintln!("child stdin: {error}");
                    }
                }
                break;
            }
        }
        libc::fcntl(0, libc::F_SETFL, flags);
    }
}

/// The dense child's full-grid repaint: header/footer bars with reverse
/// video, a body of dense varied text (the renderer pays per cell) and
/// background-colour spans, like a real TUI's cards — then the kitty images.
/// Ports `dense_draw_screen`.
fn dense_draw_screen() {
    let mut ws = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: `ws` is a writable winsize for the ioctl on our stdout (the
    // slave pty).
    if unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) } != 0 {
        return;
    }
    let (rows, cols) = (usize::from(ws.ws_row), usize::from(ws.ws_col));
    if rows < 1 || cols < 1 {
        return;
    }

    let mut frame = String::with_capacity(rows * cols * 12);
    frame.push_str("\x1b[?25l\x1b[H\x1b[2J");
    for r in 0..rows {
        let _ = write!(frame, "\x1b[{};1H", r + 1);
        for c in 0..cols {
            if r == 0 || r == rows - 1 {
                if c == 0 {
                    frame.push_str("\x1b[7m");
                }
                frame.push(if c % 2 == 0 {
                    ' '
                } else if r == 0 {
                    '-'
                } else {
                    '='
                });
                if c == cols - 1 {
                    frame.push_str("\x1b[27m");
                }
            } else if (c / 9) % 2 == 0 {
                let _ = write!(frame, "\x1b[48;5;{}m", (r * 7 + c / 9) % 200 + 16);
                let shade = u8::try_from((r * 31 + c * 7) % 26).expect("letter index in 0..=25");
                frame.push((b'a' + shade) as char);
            } else {
                frame.push_str("\x1b[49m");
                let digit = u8::try_from((r + c) % 10).expect("digit index in 0..=9");
                frame.push((b'0' + digit) as char);
            }
        }
        frame.push_str("\x1b[49m");
    }
    {
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(frame.as_bytes());
        let _ = stdout.flush();
    }
    dense_send_image();
    let mut stdout = std::io::stdout().lock();
    let _ = write!(stdout, "\x1b[{rows};1H");
    let _ = stdout.flush();
}

/// The dense child's clock row: light periodic traffic so the pty is not
/// idle, saved/restored around the cursor position like the C.
fn dense_clock_row(now: libc::time_t) {
    const FMT: &[u8] = b"\x1b[s%H:%M:%S\x1b[u\0";
    // SAFETY: `zeroed` is a valid all-zero `tm` for `localtime_r` to
    // overwrite; nothing reads it before that call.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `now` is a plain `time_t` value and `tm` is our writable
    // struct; `strftime` writes only inside our bounded buffer, whose format
    // string is a NUL-terminated literal.
    unsafe {
        libc::localtime_r(&raw const now, &raw mut tm);
        let mut buf = [0u8; 64];
        let n = libc::strftime(
            buf.as_mut_ptr().cast::<c_char>(),
            buf.len(),
            FMT.as_ptr().cast::<c_char>(),
            &raw const tm,
        );
        let clock = &buf[..n];
        let mut stdout = std::io::stdout().lock();
        let _ = write!(stdout, "\x1b[1;1H\x1b[7m ");
        let _ = stdout.write_all(clock);
        let _ = write!(stdout, " \x1b[27m");
        let _ = stdout.flush();
    }
}

/// The dense child's main loop: repaint on `SIGWINCH`, one clock row per
/// second otherwise, drain stdin, sleep. Ports `dense_child_main`. The
/// driver calls this after re-exec'ing itself with `--dense-child`.
pub fn dense_child_main() {
    // SAFETY: installing our own `SIGWINCH` disposition before any thread
    // exists; the handler only stores to an atomic.
    unsafe {
        libc::signal(
            libc::SIGWINCH,
            dense_on_winch as *const () as libc::sighandler_t,
        )
    };
    let mut last: libc::time_t = 0;
    loop {
        // SAFETY: `time` takes no argument.
        let now = unsafe { libc::time(std::ptr::null_mut()) };
        if DENSE_RESIZED.swap(false, Ordering::AcqRel) {
            dense_draw_screen();
        } else if now != last {
            last = now;
            dense_clock_row(now);
        }
        dump_stdin();
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The encoder's RFC 4648 test vectors (the empty string through all
    /// three padding shapes plus the full alphabet prefix).
    #[test]
    fn base64_encode_matches_the_rfc_4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }
}
