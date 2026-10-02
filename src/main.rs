//! tRm - a lightweight RStudio-like IDE for the terminal.

mod app;
mod backend;
mod config;
mod console;
mod draw;
mod editor;
mod files;
mod help;
mod highlight;
mod lint;
mod plot;
mod rsession;
mod ui;
mod vplot;

use app::{App, Modal};
use plot::ImageProtocol;
use backend::SafeBackend;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::crossterm::{cursor, execute};
use ratatui::Terminal;
use std::io::{BufWriter, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::{Duration, Instant};

pub enum AppEvent {
    Term(Event),
    Pty(u64, Vec<u8>),
    PtyExit(u64),
    Lint(u64, u64, Result<Vec<editor::Lint>, String>),
    HelpIndex(help::HelpIndex),
    HelpPage(Result<rsession::HelpPage, String>),
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

struct TermGuard {
    kbd: bool,
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        let mut out = std::io::stdout();
        let _ = out.write_all(&plot::kitty_clear());
        if self.kbd {
            let _ = execute!(out, PopKeyboardEnhancementFlags);
        }
        let _ = execute!(out, DisableBracketedPaste, DisableMouseCapture, LeaveAlternateScreen, cursor::Show);
        let _ = terminal::disable_raw_mode();
    }
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        // never print onto the UI; log to a file instead
        let dir = config::data_dir();
        let _ = std::fs::create_dir_all(&dir);
        let bt = std::backtrace::Backtrace::force_capture();
        let msg = format!("--- tRm {VERSION} panic ---\n{info}\n{bt}\n");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("crash.log")) {
            let _ = f.write_all(msg.as_bytes());
        }
    }));
}

fn usage() {
    println!(
        "tRm {VERSION} - a lightweight RStudio-like IDE for the terminal

usage: trm [FILE|DIR]...

Opens FILEs in the editor (or the last session when none are given) and
starts R in the console pane. Press F1 inside tRm for keyboard shortcuts.

Config file: {}
{}",
        config::config_dir().join("config").display(),
        config::EXAMPLE
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        usage();
        return;
    }
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("tRm {VERSION}");
        return;
    }
    if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        eprintln!("tRm needs to run in a terminal");
        std::process::exit(1);
    }
    install_panic_hook();
    let cfg = config::Config::load();
    let (tx, rx) = channel::<AppEvent>();
    let mut app = match App::new(cfg, tx.clone(), args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("tRm: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = terminal::enable_raw_mode() {
        eprintln!("tRm: cannot set up the terminal: {e}");
        std::process::exit(1);
    }
    let mut guard = TermGuard { kbd: false };
    let mut out = std::io::stdout();
    let _ = execute!(out, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste);
    // the kitty keyboard protocol lets us see Ctrl+Enter, Ctrl+Shift+M, Ctrl+1 ...
    if matches!(terminal::supports_keyboard_enhancement(), Ok(true)) {
        if execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)).is_ok() {
            guard.kbd = true;
            app.kbd_enhanced = true;
        }
    }
    if let Ok(ws) = terminal::window_size() {
        if ws.width > 0 && ws.columns > 0 && ws.height > 0 && ws.rows > 0 {
            app.cell_px = ((ws.width / ws.columns).max(1), (ws.height / ws.rows).max(1));
        }
    }

    // input thread: forwards terminal events into the single event channel
    {
        let tx = tx.clone();
        std::thread::spawn(move || loop {
            match event::read() {
                Ok(ev) => {
                    if tx.send(AppEvent::Term(ev)).is_err() {
                        break;
                    }
                }
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        });
    }
    drop(tx);

    let backend = SafeBackend::new(BufWriter::with_capacity(1 << 16, std::io::stdout()));
    let mut term = match Terminal::new(backend) {
        Ok(t) => t,
        Err(e) => {
            drop(guard);
            eprintln!("tRm: {e}");
            std::process::exit(1);
        }
    };

    run(&mut app, &mut term, &rx);

    drop(app);
    drop(term);
    drop(guard);
}

fn run(app: &mut App, term: &mut Terminal<SafeBackend<BufWriter<std::io::Stdout>>>, rx: &std::sync::mpsc::Receiver<AppEvent>) {
    let frame = Duration::from_millis(16);
    let mut last_draw = Instant::now() - Duration::from_secs(1);
    let mut last_tick = Instant::now() - Duration::from_secs(1);
    let mut panic_times: Vec<Instant> = Vec::new();
    let mut image_shown: Option<(std::path::PathBuf, ratatui::layout::Rect)> = None;
    let mut modal_was_open = false;

    let mut guarded = |app: &mut App, f: &mut dyn FnMut(&mut App)| {
        if catch_unwind(AssertUnwindSafe(|| f(app))).is_err() {
            app.panics += 1;
            app.emergency_save();
            app.msg_err("internal error caught - your files are safe (recovery copies saved, see ~/.local/share/trm/crash.log)");
            app.need_full_redraw = true;
            app.dirty = true;
            panic_times.push(Instant::now());
            panic_times.retain(|t| t.elapsed() < Duration::from_secs(10));
            if panic_times.len() > 30 {
                // something is badly wrong; leave cleanly rather than spin
                app.quit = true;
            }
        }
    };

    loop {
        let timeout = if app.dirty {
            frame.saturating_sub(last_draw.elapsed()).max(Duration::from_millis(1))
        } else {
            Duration::from_millis(250)
        };
        match rx.recv_timeout(timeout) {
            Ok(ev) => {
                debug_log("event");
                let mut ev = Some(ev);
                guarded(app, &mut |a| {
                    if let Some(e) = ev.take() {
                        a.handle(e)
                    }
                });
                // batch whatever else is pending before drawing
                for _ in 0..4000 {
                    match rx.try_recv() {
                        Ok(ev) => {
                            let mut ev = Some(ev);
                            guarded(app, &mut |a| {
                                if let Some(e) = ev.take() {
                                    a.handle(e)
                                }
                            });
                        }
                        Err(_) => break,
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if last_tick.elapsed() >= Duration::from_millis(200) {
            last_tick = Instant::now();
            guarded(app, &mut |a| a.tick());
        }
        if app.quit {
            break;
        }
        let modal_open = !matches!(app.modal, Modal::None);
        if modal_open != modal_was_open {
            modal_was_open = modal_open;
            if app.image_protocol == ImageProtocol::Sixel && image_shown.is_some() {
                app.need_full_redraw = true;
            }
        }
        if app.dirty && last_draw.elapsed() >= frame {
            if app.need_full_redraw {
                app.need_full_redraw = false;
                let _ = term.clear();
                image_shown = None;
            }
            let mut drew = false;
            let t0 = Instant::now();
            guarded(app, &mut |a| {
                if term.draw(|f| a.draw(f)).is_ok() {
                    drew = true;
                }
            });
            debug_log(&format!("draw {:?}", t0.elapsed()));
            app.dirty = false;
            last_draw = Instant::now();
            if drew {
                let mut extra: Vec<u8> = Vec::new();
                emit_image(app, &mut image_shown, modal_open, &mut extra);
                if let Some(t) = app.osc52.take() {
                    extra.extend_from_slice(format!("\x1b]52;c;{}\x07", plot::base64(t.as_bytes())).as_bytes());
                }
                if !extra.is_empty() {
                    let w = term.backend_mut();
                    let _ = w.write_all(&extra);
                    let _ = w.flush();
                }
            }
        }
    }
}

/// Place, move or remove the plot image after the text frame is drawn.
fn emit_image(app: &mut App, shown: &mut Option<(std::path::PathBuf, ratatui::layout::Rect)>, modal_open: bool, out: &mut Vec<u8>) {
    let req = app.image_req.clone();
    if req == *shown {
        return;
    }
    let proto = app.image_protocol;
    if shown.is_some() {
        match proto {
            ImageProtocol::Kitty => out.extend_from_slice(&plot::kitty_clear()),
            ImageProtocol::Sixel => {
                // sixel pixels live in the cells; repaint everything next frame
                if req.is_none() {
                    app.need_full_redraw = true;
                    app.dirty = true;
                    *shown = None;
                    return;
                }
            }
            ImageProtocol::None => {}
        }
    }
    *shown = None;
    let Some((path, rect)) = req else { return };
    if proto == ImageProtocol::Sixel && modal_open {
        return;
    }
    let aspect = app.cell_px.1 as f64 / app.cell_px.0.max(1) as f64;
    let Some(img) = app.plots.image() else { return };
    let (c, r) = plot::fit(img.w, img.h, rect.width as usize, rect.height as usize, aspect);
    let x = rect.x + (rect.width.saturating_sub(c as u16)) / 2;
    let y = rect.y + (rect.height.saturating_sub(r as u16)) / 2;
    match proto {
        ImageProtocol::Kitty => {
            if let Ok(bytes) = std::fs::read(&path) {
                out.extend_from_slice(&plot::kitty_place(&bytes, x, y, c as u16, r as u16));
            }
        }
        ImageProtocol::Sixel => {
            let tw = c * app.cell_px.0 as usize;
            let th = r * app.cell_px.1 as usize;
            out.extend_from_slice(&plot::sixel_place(img, x, y, tw.max(1), th.max(1)));
        }
        ImageProtocol::None => return,
    }
    *shown = Some((path, rect));
}

fn debug_log(msg: &str) {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("TRM_DEBUG").is_some()) {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("trm-debug.log")) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let _ = writeln!(f, "{t} {msg}");
    }
}
