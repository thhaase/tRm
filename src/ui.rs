//! Small, bounds-checked drawing helpers on top of ratatui's Buffer.
//! Everything clips silently instead of panicking.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use unicode_width::UnicodeWidthChar;

pub const TAB_WIDTH: usize = 4;

pub fn char_width(c: char) -> usize {
    if c == '\t' {
        return TAB_WIDTH;
    }
    c.width().unwrap_or(0)
}

/// Draw one character. Returns the x after the character.
pub fn put_char(buf: &mut Buffer, x: u16, y: u16, max_x: u16, ch: char, style: Style) -> u16 {
    let ch = if ch.is_control() { ' ' } else { ch };
    let w = ch.width().unwrap_or(0) as u16;
    if w == 0 {
        return x;
    }
    if x.saturating_add(w) > max_x {
        return max_x;
    }
    if let Some(c) = buf.cell_mut((x, y)) {
        c.set_char(ch).set_style(style);
    }
    if w == 2 {
        if let Some(c) = buf.cell_mut((x + 1, y)) {
            c.reset();
            c.set_style(style);
        }
    }
    x + w
}

/// Draw a string clipped at `max_x`. Returns the x after the last character.
pub fn put_str(buf: &mut Buffer, x: u16, y: u16, max_x: u16, s: &str, style: Style) -> u16 {
    let mut cx = x;
    for ch in s.chars() {
        if cx >= max_x {
            break;
        }
        if ch == '\t' {
            for _ in 0..TAB_WIDTH {
                cx = put_char(buf, cx, y, max_x, ' ', style);
            }
            continue;
        }
        cx = put_char(buf, cx, y, max_x, ch, style);
    }
    cx
}

pub fn fill(buf: &mut Buffer, area: Rect, style: Style) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.reset();
                c.set_style(style);
            }
        }
    }
}

pub fn str_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// Truncate a string to a display width, adding an ellipsis when cut.
pub fn truncate(s: &str, width: usize) -> String {
    if str_width(s) <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = char_width(c);
        if w + cw + 1 > width {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

pub fn pad(s: &str, width: usize) -> String {
    let t = truncate(s, width);
    let w = str_width(&t);
    let mut out = t;
    for _ in w..width {
        out.push(' ');
    }
    out
}

pub fn pad_left(s: &str, width: usize) -> String {
    let t = truncate(s, width);
    let w = str_width(&t);
    let mut out = String::new();
    for _ in w..width {
        out.push(' ');
    }
    out.push_str(&t);
    out
}

pub fn human_size(bytes: f64) -> String {
    if bytes < 0.0 {
        return String::new();
    }
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} B", v as u64)
    } else {
        format!("{:.1} {}", v, units[i])
    }
}

/// Theme built only from the 16 basic ANSI colors so it works on the Linux
/// console and old terminals as well as modern ones.
pub struct Theme;

impl Theme {
    pub fn title_focused() -> Style {
        Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
    }
    pub fn title() -> Style {
        Style::default().fg(Color::White).bg(Color::Blue)
    }
    pub fn tab_active() -> Style {
        Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)
    }
    pub fn menubar() -> Style {
        Style::default().fg(Color::Black).bg(Color::White)
    }
    pub fn menu_sel() -> Style {
        Style::default().fg(Color::White).bg(Color::Blue).add_modifier(Modifier::BOLD)
    }
    pub fn popup() -> Style {
        Style::default().fg(Color::Black).bg(Color::White)
    }
    pub fn statusbar() -> Style {
        Style::default().fg(Color::White).bg(Color::Blue)
    }
    pub fn selection() -> Style {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    }
    pub fn list_sel() -> Style {
        Style::default().add_modifier(Modifier::REVERSED)
    }
    pub fn dim() -> Style {
        Style::default().fg(Color::DarkGray)
    }
    pub fn gutter() -> Style {
        Style::default().fg(Color::DarkGray)
    }
    pub fn gutter_cur() -> Style {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    }
    pub fn separator() -> Style {
        Style::default().fg(Color::Blue)
    }
    pub fn header() -> Style {
        Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    }
    pub fn error() -> Style {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    }
    pub fn warning() -> Style {
        Style::default().fg(Color::Yellow)
    }
    pub fn info() -> Style {
        Style::default().fg(Color::Cyan)
    }
    pub fn matching_bracket() -> Style {
        Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    }
}
