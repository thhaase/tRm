//! Plot display: PNG decoding and rendering as ASCII art, braille, colored
//! half blocks, or real images through the kitty / sixel graphics protocols.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use std::io::BufReader;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlotMode {
    Ascii,
    Braille,
    Blocks,
    Image,
}

impl PlotMode {
    pub fn name(self) -> &'static str {
        match self {
            PlotMode::Ascii => "ascii",
            PlotMode::Braille => "braille",
            PlotMode::Blocks => "blocks",
            PlotMode::Image => "image",
        }
    }
    pub fn parse(s: &str) -> Option<PlotMode> {
        match s.trim() {
            "ascii" | "text" => Some(PlotMode::Ascii),
            "braille" => Some(PlotMode::Braille),
            "blocks" | "block" => Some(PlotMode::Blocks),
            "image" | "kitty" | "sixel" => Some(PlotMode::Image),
            _ => None,
        }
    }
    pub fn next(self) -> PlotMode {
        match self {
            PlotMode::Ascii => PlotMode::Braille,
            PlotMode::Braille => PlotMode::Blocks,
            PlotMode::Blocks => PlotMode::Image,
            PlotMode::Image => PlotMode::Ascii,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageProtocol {
    None,
    Kitty,
    Sixel,
}

impl ImageProtocol {
    pub fn detect() -> ImageProtocol {
        let var = |k: &str| std::env::var(k).unwrap_or_default().to_lowercase();
        let term = var("TERM");
        let prog = var("TERM_PROGRAM");
        if !var("KITTY_WINDOW_ID").is_empty()
            || term.contains("kitty")
            || term.contains("ghostty")
            || prog.contains("ghostty")
            || prog.contains("wezterm")
            || !var("GHOSTTY_RESOURCES_DIR").is_empty()
            || prog.contains("konsole")
            || !var("KONSOLE_VERSION").is_empty()
        {
            return ImageProtocol::Kitty;
        }
        if term.contains("foot")
            || term.contains("mlterm")
            || term.contains("sixel")
            || prog.contains("iterm")
            || term.contains("contour")
            || !var("WT_SESSION").is_empty()
        {
            return ImageProtocol::Sixel;
        }
        ImageProtocol::None
    }
    pub fn parse(s: &str) -> Option<Option<ImageProtocol>> {
        match s.trim() {
            "auto" => Some(None),
            "kitty" => Some(Some(ImageProtocol::Kitty)),
            "sixel" => Some(Some(ImageProtocol::Sixel)),
            "none" | "off" => Some(Some(ImageProtocol::None)),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorDepth {
    True,
    C256,
    C16,
    Mono,
}

impl ColorDepth {
    pub fn detect() -> ColorDepth {
        let ct = std::env::var("COLORTERM").unwrap_or_default().to_lowercase();
        let term = std::env::var("TERM").unwrap_or_default().to_lowercase();
        if ct.contains("truecolor") || ct.contains("24bit") || term.contains("ghostty") || term.contains("kitty") {
            ColorDepth::True
        } else if term.contains("256") {
            ColorDepth::C256
        } else if term == "dumb" {
            ColorDepth::Mono
        } else {
            ColorDepth::C16
        }
    }
    pub fn parse(s: &str) -> Option<Option<ColorDepth>> {
        match s.trim() {
            "auto" => Some(None),
            "truecolor" | "24bit" => Some(Some(ColorDepth::True)),
            "256" => Some(Some(ColorDepth::C256)),
            "16" => Some(Some(ColorDepth::C16)),
            "none" | "mono" => Some(Some(ColorDepth::Mono)),
            _ => None,
        }
    }
}

pub struct Image {
    pub w: usize,
    pub h: usize,
    pub rgb: Vec<u8>,
}

impl Image {
    fn px(&self, x: usize, y: usize) -> (u8, u8, u8) {
        let i = (y.min(self.h - 1) * self.w + x.min(self.w - 1)) * 3;
        match self.rgb.get(i..i + 3) {
            Some(p) => (p[0], p[1], p[2]),
            None => (255, 255, 255),
        }
    }
}

pub fn load_png(path: &Path) -> Result<Image, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut dec = png::Decoder::new(BufReader::new(f));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let (w, h) = {
        let info = reader.info();
        (info.width as usize, info.height as usize)
    };
    if w == 0 || h == 0 || w * h > 25_000_000 {
        return Err("image too large".into());
    }
    let size = reader.output_buffer_size().ok_or("image too large")?;
    let mut buf = vec![0u8; size];
    let frame = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (ct, _) = reader.output_color_type();
    let stride = frame.line_size;
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        let row = buf.get(y * stride..(y + 1) * stride).unwrap_or(&[]);
        for x in 0..w {
            let (r, g, b) = match ct {
                png::ColorType::Grayscale => {
                    let v = row.get(x).copied().unwrap_or(255);
                    (v, v, v)
                }
                png::ColorType::GrayscaleAlpha => {
                    let v = row.get(x * 2).copied().unwrap_or(255);
                    let a = row.get(x * 2 + 1).copied().unwrap_or(255);
                    let v = blend(v, a);
                    (v, v, v)
                }
                png::ColorType::Rgb => {
                    let p = row.get(x * 3..x * 3 + 3).unwrap_or(&[255, 255, 255]);
                    (p[0], p[1], p[2])
                }
                png::ColorType::Rgba => {
                    let p = row.get(x * 4..x * 4 + 4).unwrap_or(&[255, 255, 255, 255]);
                    (blend(p[0], p[3]), blend(p[1], p[3]), blend(p[2], p[3]))
                }
                png::ColorType::Indexed => (255, 255, 255),
            };
            rgb.extend_from_slice(&[r, g, b]);
        }
    }
    Ok(Image { w, h, rgb })
}

fn blend(v: u8, a: u8) -> u8 {
    // composite over white
    ((v as u32 * a as u32 + 255 * (255 - a as u32)) / 255) as u8
}

/// Box-filter resize.
pub fn resize(img: &Image, tw: usize, th: usize) -> Image {
    let tw = tw.max(1);
    let th = th.max(1);
    let mut rgb = Vec::with_capacity(tw * th * 3);
    for ty in 0..th {
        let y0 = ty * img.h / th;
        let y1 = ((ty + 1) * img.h / th).max(y0 + 1).min(img.h);
        for tx in 0..tw {
            let x0 = tx * img.w / tw;
            let x1 = ((tx + 1) * img.w / tw).max(x0 + 1).min(img.w);
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = img.px(x, y);
                    r += p.0 as u32;
                    g += p.1 as u32;
                    b += p.2 as u32;
                    n += 1;
                }
            }
            let n = n.max(1);
            rgb.extend_from_slice(&[(r / n) as u8, (g / n) as u8, (b / n) as u8]);
        }
    }
    Image { w: tw, h: th, rgb }
}

fn ink(p: (u8, u8, u8)) -> f32 {
    let lum = 0.299 * p.0 as f32 + 0.587 * p.1 as f32 + 0.114 * p.2 as f32;
    1.0 - lum / 255.0
}

pub fn to_color(r: u8, g: u8, b: u8, depth: ColorDepth) -> Color {
    match depth {
        ColorDepth::True => Color::Rgb(r, g, b),
        ColorDepth::C256 => {
            let mx = r.max(g).max(b) as i32;
            let mn = r.min(g).min(b) as i32;
            if mx - mn < 12 {
                let v = (r as u32 + g as u32 + b as u32) / 3;
                if v < 8 {
                    return Color::Indexed(16);
                }
                if v > 246 {
                    return Color::Indexed(231);
                }
                return Color::Indexed(232 + ((v - 8) * 24 / 239).min(23) as u8);
            }
            let q = |v: u8| -> u8 { ((v as u32 * 5 + 127) / 255) as u8 };
            Color::Indexed(16 + 36 * q(r) + 6 * q(g) + q(b))
        }
        ColorDepth::C16 => nearest16(r, g, b),
        ColorDepth::Mono => Color::Reset,
    }
}

fn nearest16(r: u8, g: u8, b: u8) -> Color {
    const PAL: [(Color, (i32, i32, i32)); 16] = [
        (Color::Black, (0, 0, 0)),
        (Color::Red, (170, 0, 0)),
        (Color::Green, (0, 170, 0)),
        (Color::Yellow, (170, 85, 0)),
        (Color::Blue, (0, 0, 170)),
        (Color::Magenta, (170, 0, 170)),
        (Color::Cyan, (0, 170, 170)),
        (Color::Gray, (170, 170, 170)),
        (Color::DarkGray, (85, 85, 85)),
        (Color::LightRed, (255, 85, 85)),
        (Color::LightGreen, (85, 255, 85)),
        (Color::LightYellow, (255, 255, 85)),
        (Color::LightBlue, (85, 85, 255)),
        (Color::LightMagenta, (255, 85, 255)),
        (Color::LightCyan, (85, 255, 255)),
        (Color::White, (255, 255, 255)),
    ];
    let mut best = Color::White;
    let mut bd = i32::MAX;
    for (c, (pr, pg, pb)) in PAL {
        let d = (r as i32 - pr).pow(2) + (g as i32 - pg).pow(2) + (b as i32 - pb).pow(2);
        if d < bd {
            bd = d;
            best = c;
        }
    }
    best
}

/// Color for "ink" drawn on the terminal's own background: dark/gray ink
/// becomes the default foreground so plots are readable on dark terminals.
pub fn ink_color(r: u8, g: u8, b: u8, depth: ColorDepth) -> Color {
    let mx = r.max(g).max(b) as i32;
    let mn = r.min(g).min(b) as i32;
    if depth == ColorDepth::Mono {
        return Color::Reset;
    }
    if mx - mn < 40 {
        let lum = (r as u32 + g as u32 + b as u32) / 3;
        return if lum < 110 { Color::Reset } else { Color::DarkGray };
    }
    // brighten dark saturated colors a little for contrast
    let boost = |v: u8| -> u8 { (v as u32 + (255 - v as u32) / 4).min(255) as u8 };
    to_color(boost(r), boost(g), boost(b), depth)
}

#[derive(Clone, Copy)]
pub struct TCell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
}

pub struct Rendered {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<TCell>,
}

/// Size (in cells) of the image fitted into cols x rows, keeping aspect ratio.
pub fn fit(img_w: usize, img_h: usize, cols: usize, rows: usize, cell_aspect: f64) -> (usize, usize) {
    if img_w == 0 || img_h == 0 || cols == 0 || rows == 0 {
        return (0, 0);
    }
    let a = img_w as f64 / img_h as f64;
    let mut c = cols as f64;
    let mut r = c / a / cell_aspect;
    if r > rows as f64 {
        r = rows as f64;
        c = r * cell_aspect * a;
    }
    ((c.floor() as usize).clamp(1, cols), (r.floor() as usize).clamp(1, rows))
}

pub fn render_text(img: &Image, cols: usize, rows: usize, mode: PlotMode, depth: ColorDepth, cell_aspect: f64) -> Rendered {
    let (c, r) = fit(img.w, img.h, cols, rows, cell_aspect);
    let mut cells = Vec::with_capacity(c * r);
    let (sx, sy) = match mode {
        PlotMode::Braille => (2, 4),
        PlotMode::Blocks => (1, 2),
        _ => (2, 3),
    };
    // sample grid: each cell is sx x sy sub-blocks of the source image
    let gw = c * sx;
    let gh = r * sy;
    let mut frac = vec![0f32; gw * gh];
    let mut cols_rgb = vec![(0u32, 0u32, 0u32, 0u32); c * r];
    let mut avg = vec![(0u32, 0u32, 0u32, 0u32); gw * gh];
    for gy in 0..gh {
        let y0 = gy * img.h / gh;
        let y1 = ((gy + 1) * img.h / gh).max(y0 + 1).min(img.h);
        for gx in 0..gw {
            let x0 = gx * img.w / gw;
            let x1 = ((gx + 1) * img.w / gw).max(x0 + 1).min(img.w);
            let mut inked = 0u32;
            let mut total = 0u32;
            let cell = (gy / sy) * c + gx / sx;
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = img.px(x, y);
                    total += 1;
                    let a = &mut avg[gy * gw + gx];
                    a.0 += p.0 as u32;
                    a.1 += p.1 as u32;
                    a.2 += p.2 as u32;
                    a.3 += 1;
                    if ink(p) > 0.3 {
                        inked += 1;
                        let cc = &mut cols_rgb[cell];
                        cc.0 += p.0 as u32;
                        cc.1 += p.1 as u32;
                        cc.2 += p.2 as u32;
                        cc.3 += 1;
                    }
                }
            }
            frac[gy * gw + gx] = inked as f32 / total.max(1) as f32;
        }
    }
    for cy in 0..r {
        for cx in 0..c {
            let on = |dx: usize, dy: usize, thr: f32| frac[(cy * sy + dy) * gw + cx * sx + dx] > thr;
            let cc = cols_rgb[cy * c + cx];
            let fg = if cc.3 > 0 {
                ink_color((cc.0 / cc.3) as u8, (cc.1 / cc.3) as u8, (cc.2 / cc.3) as u8, depth)
            } else {
                Color::Reset
            };
            let cell = match mode {
                PlotMode::Braille => {
                    const BITS: [[u32; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
                    let mut code = 0u32;
                    for dx in 0..2 {
                        for dy in 0..4 {
                            if on(dx, dy, 0.02) {
                                code |= BITS[dx][dy];
                            }
                        }
                    }
                    let ch = if code == 0 { ' ' } else { char::from_u32(0x2800 + code).unwrap_or(' ') };
                    TCell { ch, fg, bg: Color::Reset }
                }
                PlotMode::Blocks => {
                    let get = |dy: usize| {
                        let a = avg[(cy * 2 + dy) * gw + cx];
                        let n = a.3.max(1);
                        to_color((a.0 / n) as u8, (a.1 / n) as u8, (a.2 / n) as u8, depth)
                    };
                    TCell { ch: '▀', fg: get(0), bg: get(1) }
                }
                _ => {
                    let mut bits = [[false; 3]; 2];
                    let mut n = 0;
                    let mut sum = 0.0;
                    for dx in 0..2 {
                        for dy in 0..3 {
                            bits[dx][dy] = on(dx, dy, 0.04);
                            if bits[dx][dy] {
                                n += 1;
                            }
                            sum += frac[(cy * 3 + dy) * gw + cx * 2 + dx];
                        }
                    }
                    TCell { ch: ascii_glyph(&bits, n, sum / 6.0), fg, bg: Color::Reset }
                }
            };
            cells.push(cell);
        }
    }
    Rendered { cols: c, rows: r, cells }
}

/// Pick an ASCII character from a 2x3 grid of "ink" flags.
fn ascii_glyph(b: &[[bool; 3]; 2], n: usize, density: f32) -> char {
    if n == 0 {
        return ' ';
    }
    if density > 0.55 {
        return '#';
    }
    let (tl, ml, bl) = (b[0][0], b[0][1], b[0][2]);
    let (tr, mr, br) = (b[1][0], b[1][1], b[1][2]);
    let top = tl || tr;
    let mid = ml || mr;
    let bot = bl || br;
    let left = tl || ml || bl;
    let right = tr || mr || br;
    let rows_on = top as u8 + mid as u8 + bot as u8;
    if n >= 5 {
        return if density > 0.3 { '#' } else { '+' };
    }
    if rows_on == 1 {
        if n == 1 {
            return if top { '\'' } else if mid { '-' } else { '.' };
        }
        return if bot { '_' } else { '-' };
    }
    if rows_on == 3 {
        if tr && bl && !tl && !br {
            return '/';
        }
        if tl && br && !tr && !bl {
            return '\\';
        }
        if !(left && right) {
            return '|';
        }
        return if mid && (ml && mr) { '+' } else { '|' };
    }
    // two rows active
    if top && bot && !mid {
        return ':';
    }
    if n == 4 {
        return '=';
    }
    if (tr && ml && !tl) || (mr && bl && !ml) {
        return '/';
    }
    if (tl && mr && !tr) || (ml && br && !mr) {
        return '\\';
    }
    if !(left && right) {
        return '|';
    }
    '+'
}

pub fn draw_rendered(buf: &mut Buffer, area: Rect, r: &Rendered) {
    let ox = area.x + (area.width.saturating_sub(r.cols as u16)) / 2;
    let oy = area.y + (area.height.saturating_sub(r.rows as u16)) / 2;
    for y in 0..r.rows {
        for x in 0..r.cols {
            let Some(c) = r.cells.get(y * r.cols + x) else { continue };
            let px = ox + x as u16;
            let py = oy + y as u16;
            if px >= area.right() || py >= area.bottom() {
                continue;
            }
            if let Some(cell) = buf.cell_mut((px, py)) {
                cell.set_char(c.ch).set_style(Style::default().fg(c.fg).bg(c.bg));
            }
        }
    }
}

// ---------- graphics protocols ----------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 4 / 3 + 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

pub const KITTY_ID: u32 = 7291;

pub fn kitty_clear() -> Vec<u8> {
    format!("\x1b_Ga=d,d=I,i={KITTY_ID},q=2\x1b\\").into_bytes()
}

pub fn kitty_place(png: &[u8], x: u16, y: u16, cols: u16, rows: u16) -> Vec<u8> {
    let data = base64(png);
    let mut out = kitty_clear();
    out.extend_from_slice(format!("\x1b7\x1b[{};{}H", y + 1, x + 1).as_bytes());
    let chunks: Vec<&[u8]> = data.as_bytes().chunks(4096).collect();
    let n = chunks.len();
    for (i, ch) in chunks.iter().enumerate() {
        let more = if i + 1 < n { 1 } else { 0 };
        if i == 0 {
            out.extend_from_slice(
                format!("\x1b_Ga=T,f=100,t=d,i={KITTY_ID},q=2,C=1,z=-1,c={cols},r={rows},m={more};").as_bytes(),
            );
        } else {
            out.extend_from_slice(format!("\x1b_Gm={more};").as_bytes());
        }
        out.extend_from_slice(ch);
        out.extend_from_slice(b"\x1b\\");
    }
    out.extend_from_slice(b"\x1b8");
    out
}

pub fn sixel_place(img: &Image, x: u16, y: u16, tw: usize, th: usize) -> Vec<u8> {
    let small = resize(img, tw, th);
    let (w, h) = (small.w, small.h);
    let q = |v: u8| -> usize { (v as usize * 5 + 127) / 255 };
    let idx: Vec<u8> = small.rgb.chunks(3).map(|p| (q(p[0]) * 36 + q(p[1]) * 6 + q(p[2])) as u8).collect();
    let mut used = [false; 216];
    for i in &idx {
        used[*i as usize] = true;
    }
    let mut out = Vec::with_capacity(w * h / 2);
    out.extend_from_slice(format!("\x1b7\x1b[{};{}H", y + 1, x + 1).as_bytes());
    out.extend_from_slice(format!("\x1bP0;1;0q\"1;1;{w};{h}").as_bytes());
    for (i, u) in used.iter().enumerate() {
        if *u {
            let (r, g, b) = (i / 36, (i / 6) % 6, i % 6);
            out.extend_from_slice(format!("#{};2;{};{};{}", i, r * 20, g * 20, b * 20).as_bytes());
        }
    }
    let mut band_used = [false; 216];
    for by in (0..h).step_by(6) {
        band_used.iter_mut().for_each(|b| *b = false);
        for yy in by..(by + 6).min(h) {
            for xx in 0..w {
                band_used[idx[yy * w + xx] as usize] = true;
            }
        }
        let mut first = true;
        for color in 0..216usize {
            if !band_used[color] {
                continue;
            }
            if !first {
                out.push(b'$');
            }
            first = false;
            out.extend_from_slice(format!("#{color}").as_bytes());
            let mut run_char = 0u8;
            let mut run_len = 0usize;
            let flush = |out: &mut Vec<u8>, c: u8, n: usize| {
                if n == 0 {
                    return;
                }
                if n > 3 {
                    out.extend_from_slice(format!("!{n}").as_bytes());
                    out.push(c);
                } else {
                    for _ in 0..n {
                        out.push(c);
                    }
                }
            };
            for xx in 0..w {
                let mut bits = 0u8;
                for k in 0..6 {
                    let yy = by + k;
                    if yy < h && idx[yy * w + xx] as usize == color {
                        bits |= 1 << k;
                    }
                }
                let c = 63 + bits;
                if c == run_char {
                    run_len += 1;
                } else {
                    flush(&mut out, run_char, run_len);
                    run_char = c;
                    run_len = 1;
                }
            }
            flush(&mut out, run_char, run_len);
        }
        out.push(b'-');
    }
    out.extend_from_slice(b"\x1b\\\x1b8");
    out
}

/// Plot history + render cache.
pub struct PlotView {
    pub history: Vec<PathBuf>,
    pub idx: usize,
    pub mode: PlotMode,
    image: Option<(PathBuf, Image)>,
    vector: Option<(PathBuf, Option<crate::vplot::VPlot>)>,
    rendered: Option<((PathBuf, usize, usize, PlotMode), Rendered)>,
    pub error: Option<String>,
}

impl PlotView {
    pub fn new(mode: PlotMode) -> PlotView {
        PlotView { history: Vec::new(), idx: 0, mode, image: None, vector: None, rendered: None, error: None }
    }

    pub fn current(&self) -> Option<&PathBuf> {
        self.history.get(self.idx)
    }

    pub fn push(&mut self, p: PathBuf) {
        if self.history.last() == Some(&p) {
            self.idx = self.history.len() - 1;
            self.image = None;
            self.rendered = None;
            return;
        }
        self.history.push(p);
        if self.history.len() > 200 {
            self.history.remove(0);
        }
        self.idx = self.history.len() - 1;
    }

    pub fn image(&mut self) -> Option<&Image> {
        let path = self.current()?.clone();
        let stale = self.image.as_ref().map(|(p, _)| *p != path).unwrap_or(true);
        if stale {
            match load_png(&path) {
                Ok(img) => {
                    self.image = Some((path, img));
                    self.error = None;
                }
                Err(e) => {
                    self.image = None;
                    self.error = Some(e);
                    return None;
                }
            }
        }
        self.image.as_ref().map(|(_, i)| i)
    }

    /// The vector (PDF) version of the current plot, when R wrote one.
    fn vector(&mut self) -> Option<&crate::vplot::VPlot> {
        let path = self.current()?.clone();
        if self.vector.as_ref().map(|(p, _)| *p != path).unwrap_or(true) {
            let pdf = path.with_extension("pdf");
            let v = std::fs::read(&pdf).ok().and_then(|b| crate::vplot::parse_pdf(&b).ok());
            self.vector = Some((path, v));
        }
        self.vector.as_ref().and_then(|(_, v)| v.as_ref())
    }

    pub fn rendered(&mut self, cols: usize, rows: usize, mode: PlotMode, depth: ColorDepth, cell_aspect: f64) -> Option<&Rendered> {
        let path = self.current()?.clone();
        let key = (path, cols, rows, mode);
        if self.rendered.as_ref().map(|(k, _)| *k != key).unwrap_or(true) {
            let vector_mode = matches!(mode, PlotMode::Ascii | PlotMode::Braille);
            let r = match (vector_mode, self.vector().cloned()) {
                (true, Some(v)) => {
                    // keep the aspect ratio R laid the plot out with
                    let (nc, nr) = (v.w / crate::vplot::CELL_W, v.h / crate::vplot::CELL_H);
                    let (c, r) = fit(nc.round().max(1.0) as usize, nr.round().max(1.0) as usize, cols, rows, 1.0);
                    crate::vplot::render(&v, c, r, mode == PlotMode::Braille, depth)
                }
                _ => {
                    let img = self.image()?;
                    render_text(img, cols, rows, mode, depth, cell_aspect)
                }
            };
            self.rendered = Some((key, r));
        }
        self.rendered.as_ref().map(|(_, r)| r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }

    #[test]
    fn fit_keeps_aspect() {
        let (c, r) = fit(800, 600, 80, 100, 2.0);
        assert_eq!(c, 80);
        assert_eq!(r, 30);
    }

    #[test]
    fn glyphs() {
        let none = [[false; 3]; 2];
        assert_eq!(ascii_glyph(&none, 0, 0.0), ' ');
        let vert = [[true, true, true], [false, false, false]];
        assert_eq!(ascii_glyph(&vert, 3, 0.3), '|');
        let bottom = [[false, false, true], [false, false, true]];
        assert_eq!(ascii_glyph(&bottom, 2, 0.2), '_');
        let slash = [[false, false, true], [true, false, false]];
        assert_eq!(ascii_glyph(&slash, 2, 0.1), ':');
    }
}
