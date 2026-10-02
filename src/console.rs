//! The R console: a real R process running in a pseudo terminal, rendered
//! through the `vt100` terminal emulator.

use crate::AppEvent;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::time::Instant;

pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<std::path::PathBuf>,
}

pub struct Console {
    pub parser: vt100::Parser,
    master: RawFd,
    writer: Sender<Vec<u8>>,
    child: Child,
    pub alive: bool,
    pub gen: u64,
    pub scroll: usize,
    size: (u16, u16),
    pub last_output: Instant,
}

const SCROLLBACK: usize = 3000;

impl Console {
    pub fn spawn(spec: &SpawnSpec, rows: u16, cols: u16, tx: Sender<AppEvent>, gen: u64) -> Result<Console, String> {
        let rows = rows.max(2);
        let cols = cols.max(10);
        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        let ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
        // SAFETY: plain FFI call with valid out-pointers.
        let r = unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), &ws) };
        if r != 0 {
            return Err(format!("openpty failed: {}", std::io::Error::last_os_error()));
        }
        unsafe {
            libc::fcntl(master, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        // SAFETY: openpty returned valid, owned descriptors.
        let slave_file = unsafe { File::from_raw_fd(slave) };
        let mk = |f: &File| -> Result<Stdio, String> { Ok(Stdio::from(f.try_clone().map_err(|e| e.to_string())?)) };

        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args)
            .stdin(mk(&slave_file)?)
            .stdout(mk(&slave_file)?)
            .stderr(mk(&slave_file)?);
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        if let Some(d) = &spec.cwd {
            cmd.current_dir(d);
        }
        // SAFETY: only async-signal-safe libc calls in the child.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                libc::ioctl(0, libc::TIOCSCTTY as _, 0);
                Ok(())
            });
        }
        let child = cmd.spawn();
        drop(cmd);
        drop(slave_file);
        let child = match child {
            Ok(c) => c,
            Err(e) => {
                unsafe { libc::close(master) };
                return Err(format!("could not start '{}': {e}", spec.program));
            }
        };

        // reader thread
        let rfd = unsafe { libc::dup(master) };
        if rfd >= 0 {
            let mut rfile = unsafe { File::from_raw_fd(rfd) };
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut buf = vec![0u8; 16384];
                loop {
                    match rfile.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if tx.send(AppEvent::Pty(gen, buf[..n].to_vec())).is_err() {
                                return;
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
                let _ = tx.send(AppEvent::PtyExit(gen));
            });
        }
        // writer thread: never block the UI on a full pty buffer
        let (wtx, wrx) = channel::<Vec<u8>>();
        let wfd = unsafe { libc::dup(master) };
        if wfd >= 0 {
            let mut wfile = unsafe { File::from_raw_fd(wfd) };
            std::thread::spawn(move || {
                while let Ok(data) = wrx.recv() {
                    if wfile.write_all(&data).is_err() {
                        break;
                    }
                    let _ = wfile.flush();
                }
            });
        }

        Ok(Console {
            parser: vt100::Parser::new(rows, cols, SCROLLBACK),
            master,
            writer: wtx,
            child,
            alive: true,
            gen,
            scroll: 0,
            size: (rows, cols),
            last_output: Instant::now(),
        })
    }

    pub fn pid(&self) -> i32 {
        self.child.id() as i32
    }

    pub fn write(&mut self, data: &[u8]) {
        if self.alive && !data.is_empty() {
            let _ = self.writer.send(data.to_vec());
        }
        self.set_scroll(0);
    }

    pub fn process(&mut self, data: &[u8]) {
        self.parser.process(data);
        self.last_output = Instant::now();
        if self.scroll > 0 {
            // keep the view stable while output arrives
            self.parser.screen_mut().set_scrollback(self.scroll);
        }
    }

    pub fn interrupt(&self) {
        if self.alive {
            unsafe {
                libc::kill(self.pid(), libc::SIGINT);
            }
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(2);
        let cols = cols.max(10);
        if (rows, cols) == self.size {
            return;
        }
        self.size = (rows, cols);
        self.parser.screen_mut().set_size(rows, cols);
        let ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
        unsafe {
            libc::ioctl(self.master, libc::TIOCSWINSZ as _, &ws);
        }
    }

    pub fn max_scroll(&mut self) -> usize {
        let s = self.parser.screen_mut();
        let cur = s.scrollback();
        s.set_scrollback(usize::MAX);
        let m = s.scrollback();
        s.set_scrollback(cur);
        m
    }

    pub fn set_scroll(&mut self, n: usize) {
        let m = if n > 0 { self.max_scroll() } else { 0 };
        self.scroll = n.min(m);
        self.parser.screen_mut().set_scrollback(self.scroll);
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let n = (self.scroll as isize + delta).max(0) as usize;
        self.set_scroll(n);
    }

    /// Text of the cursor row up to the cursor (used to detect a prompt).
    pub fn prompt_text(&self) -> String {
        let screen = self.parser.screen();
        let (row, col) = screen.cursor_position();
        let mut s = String::new();
        for c in 0..col {
            if let Some(cell) = screen.cell(row, c) {
                let t = cell.contents();
                if t.is_empty() {
                    s.push(' ');
                } else {
                    s.push_str(t);
                }
            }
        }
        s
    }

    /// Heuristic: R is waiting for input at a prompt.
    pub fn at_prompt(&mut self) -> bool {
        if !self.alive {
            return false;
        }
        let scroll = self.scroll;
        if scroll > 0 {
            self.parser.screen_mut().set_scrollback(0);
        }
        let p = self.prompt_text();
        if scroll > 0 {
            self.parser.screen_mut().set_scrollback(scroll);
        }
        p.ends_with("> ") || p.ends_with("+ ") || p.ends_with("? ") || p.ends_with(": ")
    }

    pub fn alternate_screen(&self) -> bool {
        self.parser.screen().alternate_screen()
    }

    pub fn bracketed_paste(&self) -> bool {
        self.parser.screen().bracketed_paste()
    }

    pub fn application_cursor(&self) -> bool {
        self.parser.screen().application_cursor()
    }

    /// Render into `area`; returns the cursor position when it should be shown.
    pub fn render(&mut self, buf: &mut Buffer, area: Rect) -> Option<(u16, u16)> {
        if area.width == 0 || area.height == 0 {
            return None;
        }
        self.resize(area.height, area.width);
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        for r in 0..rows.min(area.height) {
            for c in 0..cols.min(area.width) {
                let Some(cell) = screen.cell(r, c) else { continue };
                if cell.is_wide_continuation() {
                    continue;
                }
                let mut style = Style::default().fg(conv(cell.fgcolor())).bg(conv(cell.bgcolor()));
                if cell.bold() {
                    style = style.add_modifier(Modifier::BOLD);
                }
                if cell.italic() {
                    style = style.add_modifier(Modifier::ITALIC);
                }
                if cell.underline() {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                if cell.inverse() {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                if cell.dim() {
                    style = style.add_modifier(Modifier::DIM);
                }
                let x = area.x + c;
                let y = area.y + r;
                let text = cell.contents();
                if let Some(bc) = buf.cell_mut((x, y)) {
                    bc.reset();
                    if text.is_empty() {
                        bc.set_char(' ');
                    } else {
                        bc.set_symbol(text);
                    }
                    bc.set_style(style);
                }
            }
        }
        if self.scroll > 0 {
            let label = format!(" scrollback {} ", self.scroll);
            let x = area.right().saturating_sub(label.len() as u16);
            crate::ui::put_str(buf, x, area.y, area.right(), &label, crate::ui::Theme::menu_sel());
            return None;
        }
        if screen.hide_cursor() || !self.alive {
            return None;
        }
        let (cr, cc) = screen.cursor_position();
        if cr < area.height && cc < area.width {
            Some((area.x + cc, area.y + cr))
        } else {
            None
        }
    }

    pub fn kill(&mut self) {
        if self.alive {
            unsafe {
                libc::kill(self.pid(), libc::SIGHUP);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.try_wait();
        self.alive = false;
    }

    pub fn reap(&mut self) {
        let _ = self.child.try_wait();
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        self.kill();
        // reap without blocking forever
        for _ in 0..20 {
            if let Ok(Some(_)) = self.child.try_wait() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        unsafe {
            libc::close(self.master);
        }
    }
}

fn conv(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => match i {
            0 => Color::Black,
            1 => Color::Red,
            2 => Color::Green,
            3 => Color::Yellow,
            4 => Color::Blue,
            5 => Color::Magenta,
            6 => Color::Cyan,
            7 => Color::Gray,
            8 => Color::DarkGray,
            9 => Color::LightRed,
            10 => Color::LightGreen,
            11 => Color::LightYellow,
            12 => Color::LightBlue,
            13 => Color::LightMagenta,
            14 => Color::LightCyan,
            15 => Color::White,
            i => Color::Indexed(i),
        },
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Encode a key press as the bytes a terminal would send.
pub fn key_bytes(k: &KeyEvent, app_cursor: bool) -> Vec<u8> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
    let mut out = Vec::new();
    let modnum = 1 + (shift as u8) + 2 * (alt as u8) + 4 * (ctrl as u8);
    let csi_mod = |out: &mut Vec<u8>, final_: u8| {
        if modnum > 1 {
            out.extend_from_slice(format!("\x1b[1;{modnum}").as_bytes());
            out.push(final_);
        } else if app_cursor {
            out.extend_from_slice(&[0x1b, b'O', final_]);
        } else {
            out.extend_from_slice(&[0x1b, b'[', final_]);
        }
    };
    let tilde = |out: &mut Vec<u8>, n: u8| {
        if modnum > 1 {
            out.extend_from_slice(format!("\x1b[{n};{modnum}~").as_bytes());
        } else {
            out.extend_from_slice(format!("\x1b[{n}~").as_bytes());
        }
    };
    match k.code {
        KeyCode::Char(c) => {
            if alt {
                out.push(0x1b);
            }
            if ctrl {
                let lc = c.to_ascii_lowercase();
                let b = match lc {
                    'a'..='z' => Some(lc as u8 - b'a' + 1),
                    ' ' | '@' | '2' => Some(0),
                    '[' | '3' => Some(27),
                    '\\' | '4' => Some(28),
                    ']' | '5' => Some(29),
                    '^' | '6' => Some(30),
                    '_' | '-' | '7' | '/' => Some(31),
                    '?' | '8' => Some(127),
                    _ => None,
                };
                match b {
                    Some(b) => out.push(b),
                    None => {
                        let mut t = [0u8; 4];
                        out.extend_from_slice(c.encode_utf8(&mut t).as_bytes());
                    }
                }
            } else {
                let mut t = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut t).as_bytes());
            }
        }
        KeyCode::Enter => {
            if alt {
                out.push(0x1b);
            }
            out.push(b'\r');
        }
        KeyCode::Tab => out.push(b'\t'),
        KeyCode::BackTab => out.extend_from_slice(b"\x1b[Z"),
        KeyCode::Backspace => {
            if alt {
                out.push(0x1b);
            }
            out.push(if ctrl { 0x17 } else { 0x7f });
        }
        KeyCode::Esc => out.push(0x1b),
        KeyCode::Up => csi_mod(&mut out, b'A'),
        KeyCode::Down => csi_mod(&mut out, b'B'),
        KeyCode::Right => csi_mod(&mut out, b'C'),
        KeyCode::Left => csi_mod(&mut out, b'D'),
        KeyCode::Home => csi_mod(&mut out, b'H'),
        KeyCode::End => csi_mod(&mut out, b'F'),
        KeyCode::Insert => tilde(&mut out, 2),
        KeyCode::Delete => tilde(&mut out, 3),
        KeyCode::PageUp => tilde(&mut out, 5),
        KeyCode::PageDown => tilde(&mut out, 6),
        KeyCode::F(n) => {
            let s: &[u8] = match n {
                1 => b"\x1bOP",
                2 => b"\x1bOQ",
                3 => b"\x1bOR",
                4 => b"\x1bOS",
                5 => b"\x1b[15~",
                6 => b"\x1b[17~",
                7 => b"\x1b[18~",
                8 => b"\x1b[19~",
                9 => b"\x1b[20~",
                10 => b"\x1b[21~",
                11 => b"\x1b[23~",
                12 => b"\x1b[24~",
                _ => b"",
            };
            out.extend_from_slice(s);
        }
        _ => {}
    }
    out
}
