//! Vector plots for text mode. R also replays every plot into an
//! uncompressed PDF; we interpret its content stream (lines, fills, small
//! circles = points, and text) and draw it on the character grid, so titles,
//! axis labels and tick numbers come out as real, readable text.

use crate::plot::{ink_color, ColorDepth, Rendered, TCell};
use ratatui::style::Color;

/// Size of one terminal cell in PDF points. R renders the PDF at
/// cols*CELL_W x rows*CELL_H so its layout matches the pane.
pub const CELL_W: f32 = 5.0;
pub const CELL_H: f32 = 11.0;

type Rgb = (u8, u8, u8);

#[derive(Clone, Debug)]
pub enum Prim {
    Line { a: (f32, f32), b: (f32, f32), col: Rgb, w: f32 },
    Fill { pts: Vec<(f32, f32)>, col: Rgb },
    Point { x: f32, y: f32, filled: bool, col: Rgb },
    Text { x: f32, y: f32, size: f32, angle: f32, s: String, col: Rgb },
}

#[derive(Clone, Debug, Default)]
pub struct VPlot {
    pub w: f32,
    pub h: f32,
    pub prims: Vec<Prim>,
}

// ---------- PDF parsing ----------

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= hay.len() {
        return None;
    }
    hay[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

/// The page content stream and media box of a single-page PDF written by R.
fn page_stream(pdf: &[u8]) -> Option<(&[u8], f32, f32)> {
    // media box
    let mb = find(pdf, b"/MediaBox [", 0)?;
    let end = find(pdf, b"]", mb)?;
    let nums: Vec<f32> = std::str::from_utf8(&pdf[mb + 11..end]).ok()?.split_whitespace().filter_map(|t| t.parse().ok()).collect();
    if nums.len() != 4 {
        return None;
    }
    let (w, h) = (nums[2] - nums[0], nums[3] - nums[1]);
    // content object
    let c = find(pdf, b"/Contents ", 0)?;
    let rest = std::str::from_utf8(&pdf[c + 10..(c + 30).min(pdf.len())]).ok()?;
    let obj: u32 = rest.split_whitespace().next()?.parse().ok()?;
    let head = format!("\n{obj} 0 obj");
    let o = find(pdf, head.as_bytes(), 0)?;
    let s = find(pdf, b"stream", o + head.len())?;
    let mut start = s + 6;
    while start < pdf.len() && (pdf[start] == b'\r' || pdf[start] == b'\n') {
        start += 1;
    }
    let e = find(pdf, b"endstream", start)?;
    Some((&pdf[start..e], w, h))
}

#[derive(Clone, Debug)]
enum Tok {
    Num(f32),
    Str(Vec<u8>),
    Name,
    Op(String),
    ArrOpen,
    ArrClose,
}

fn tokenize(s: &[u8]) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut i = 0;
    let n = s.len();
    while i < n {
        let c = s[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'%' {
            while i < n && s[i] != b'\n' {
                i += 1;
            }
        } else if c == b'(' {
            let mut depth = 1;
            let mut buf = Vec::new();
            i += 1;
            while i < n && depth > 0 {
                let ch = s[i];
                if ch == b'\\' && i + 1 < n {
                    i += 1;
                    let e = s[i];
                    match e {
                        b'n' => buf.push(b'\n'),
                        b'r' => buf.push(b'\r'),
                        b't' => buf.push(b'\t'),
                        b'b' | b'f' => {}
                        b'0'..=b'7' => {
                            let mut v: u32 = 0;
                            let mut k = 0;
                            while k < 3 && i < n && (b'0'..=b'7').contains(&s[i]) {
                                v = v * 8 + (s[i] - b'0') as u32;
                                i += 1;
                                k += 1;
                            }
                            buf.push(v as u8);
                            continue;
                        }
                        b'\n' => {}
                        other => buf.push(other),
                    }
                    i += 1;
                    continue;
                }
                if ch == b'(' {
                    depth += 1;
                } else if ch == b')' {
                    depth -= 1;
                    if depth == 0 {
                        i += 1;
                        break;
                    }
                }
                buf.push(ch);
                i += 1;
            }
            out.push(Tok::Str(buf));
        } else if c == b'[' {
            out.push(Tok::ArrOpen);
            i += 1;
        } else if c == b']' {
            out.push(Tok::ArrClose);
            i += 1;
        } else if c == b'<' {
            // hex strings / dictionaries: skip to the closing bracket
            while i < n && s[i] != b'>' {
                i += 1;
            }
            while i < n && s[i] == b'>' {
                i += 1;
            }
        } else if c == b'/' {
            i += 1;
            while i < n && !s[i].is_ascii_whitespace() && !b"/[]()<>".contains(&s[i]) {
                i += 1;
            }
            out.push(Tok::Name);
        } else {
            let st = i;
            while i < n && !s[i].is_ascii_whitespace() && !b"/[]()<>%".contains(&s[i]) {
                i += 1;
            }
            let t = &s[st..i];
            let txt = String::from_utf8_lossy(t);
            match txt.parse::<f32>() {
                Ok(v) => out.push(Tok::Num(v)),
                Err(_) => out.push(Tok::Op(txt.to_string())),
            }
            if i == st {
                i += 1;
            }
        }
    }
    out
}

/// WinAnsi bytes as written by R's pdf device.
fn decode_text(b: &[u8]) -> String {
    b.iter()
        .map(|&c| match c {
            0x80 => '€',
            0x91 | 0x92 => '\'',
            0x93 | 0x94 => '"',
            0x95 => '•',
            0x96 | 0x97 | 0xAD => '-',
            0x85 => '…',
            0x20..=0x7E => c as char,
            0xA0..=0xFF => char::from(c),
            _ => '?',
        })
        .collect()
}

#[derive(Clone, Copy)]
struct GState {
    stroke: Rgb,
    fill: Rgb,
    lw: f32,
    clip: (f32, f32, f32, f32), // x0, y0, x1, y1
}

fn to_rgb(v: &[f32]) -> Rgb {
    let c = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    match v.len() {
        1 => (c(v[0]), c(v[0]), c(v[0])),
        3 => (c(v[0]), c(v[1]), c(v[2])),
        4 => {
            // cmyk
            let k = v[3];
            (c((1.0 - v[0]) * (1.0 - k)), c((1.0 - v[1]) * (1.0 - k)), c((1.0 - v[2]) * (1.0 - k)))
        }
        _ => (0, 0, 0),
    }
}

pub fn parse_pdf(pdf: &[u8]) -> Result<VPlot, String> {
    let (stream, w, h) = page_stream(pdf).ok_or("no page content")?;
    if w <= 0.0 || h <= 0.0 {
        return Err("empty page".into());
    }
    let toks = tokenize(stream);
    let mut vp = VPlot { w, h, prims: Vec::new() };
    let full = (0.0, 0.0, w, h);
    let mut gs = GState { stroke: (0, 0, 0), fill: (0, 0, 0), lw: 1.0, clip: full };
    let mut stack: Vec<GState> = Vec::new();
    let mut nums: Vec<f32> = Vec::new();
    let mut strs: Vec<Vec<u8>> = Vec::new();
    // current path: subpaths of points + whether curves were used
    let mut path: Vec<(Vec<(f32, f32)>, bool, bool)> = Vec::new(); // (pts, curved, closed)
    let mut pending_clip = false;
    let mut tm = [1.0f32, 0.0, 0.0, 1.0, 0.0, 0.0];
    let mut in_array = false;

    for t in toks {
        match t {
            Tok::Num(v) => {
                if !in_array {
                    nums.push(v)
                }
            }
            Tok::Str(s) => strs.push(s),
            Tok::Name => {}
            Tok::ArrOpen => in_array = true,
            Tok::ArrClose => in_array = false,
            Tok::Op(op) => {
                let a = &nums;
                match op.as_str() {
                    "q" => stack.push(gs),
                    "Q" => gs = stack.pop().unwrap_or(GState { stroke: (0, 0, 0), fill: (0, 0, 0), lw: 1.0, clip: full }),
                    "w" if !a.is_empty() => gs.lw = a[a.len() - 1],
                    "rg" | "scn" | "sc" | "g" | "k" => gs.fill = to_rgb(a),
                    "RG" | "SCN" | "SC" | "G" | "K" => gs.stroke = to_rgb(a),
                    "m" if a.len() >= 2 => path.push((vec![(a[a.len() - 2], a[a.len() - 1])], false, false)),
                    "l" if a.len() >= 2 => {
                        if let Some(p) = path.last_mut() {
                            p.0.push((a[a.len() - 2], a[a.len() - 1]));
                        }
                    }
                    "c" | "v" | "y" if a.len() >= 4 => {
                        if let Some(p) = path.last_mut() {
                            // sample the control polygon mid point and the end point
                            let end = (a[a.len() - 2], a[a.len() - 1]);
                            if a.len() >= 6 {
                                let k = a.len() - 6;
                                p.0.push(((a[k] + a[k + 2]) / 2.0, (a[k + 1] + a[k + 3]) / 2.0));
                            }
                            p.0.push(end);
                            p.1 = true;
                        }
                    }
                    "h" => {
                        if let Some(p) = path.last_mut() {
                            p.2 = true;
                        }
                    }
                    "re" if a.len() >= 4 => {
                        let k = a.len() - 4;
                        let (x, y, rw, rh) = (a[k], a[k + 1], a[k + 2], a[k + 3]);
                        path.push((vec![(x, y), (x + rw, y), (x + rw, y + rh), (x, y + rh)], false, true));
                    }
                    "W" | "W*" => pending_clip = true,
                    "n" => {
                        if pending_clip {
                            if let Some(bb) = bbox(&path) {
                                let c = gs.clip;
                                gs.clip = (c.0.max(bb.0), c.1.max(bb.1), c.2.min(bb.2), c.3.min(bb.3));
                            }
                        }
                        pending_clip = false;
                        path.clear();
                    }
                    "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" => {
                        let fill = matches!(op.as_str(), "f" | "F" | "f*" | "B" | "B*" | "b" | "b*");
                        let stroke = matches!(op.as_str(), "S" | "s" | "B" | "B*" | "b" | "b*");
                        let close = matches!(op.as_str(), "s" | "b" | "b*");
                        if pending_clip {
                            if let Some(bb) = bbox(&path) {
                                let c = gs.clip;
                                gs.clip = (c.0.max(bb.0), c.1.max(bb.1), c.2.min(bb.2), c.3.min(bb.3));
                            }
                            pending_clip = false;
                        }
                        emit_path(&mut vp, &path, fill, stroke, close, &gs);
                        path.clear();
                    }
                    "BT" => tm = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                    "Tm" if a.len() >= 6 => {
                        let k = a.len() - 6;
                        tm = [a[k], a[k + 1], a[k + 2], a[k + 3], a[k + 4], a[k + 5]];
                    }
                    "Td" | "TD" if a.len() >= 2 => {
                        tm[4] += a[a.len() - 2] * tm[0] + a[a.len() - 1] * tm[2];
                        tm[5] += a[a.len() - 2] * tm[1] + a[a.len() - 1] * tm[3];
                    }
                    "Tj" | "'" | "\"" => {
                        if let Some(s) = strs.last() {
                            push_text(&mut vp, decode_text(s), &tm, &gs);
                        }
                    }
                    "TJ" => {
                        let s: Vec<u8> = strs.iter().flatten().copied().collect();
                        push_text(&mut vp, decode_text(&s), &tm, &gs);
                    }
                    _ => {}
                }
                nums.clear();
                strs.clear();
            }
        }
    }
    Ok(vp)
}

fn bbox(path: &[(Vec<(f32, f32)>, bool, bool)]) -> Option<(f32, f32, f32, f32)> {
    let mut it = path.iter().flat_map(|p| p.0.iter());
    let first = it.next()?;
    let mut b = (first.0, first.1, first.0, first.1);
    for p in it {
        b.0 = b.0.min(p.0);
        b.1 = b.1.min(p.1);
        b.2 = b.2.max(p.0);
        b.3 = b.3.max(p.1);
    }
    Some(b)
}

fn inside(clip: (f32, f32, f32, f32), x: f32, y: f32) -> bool {
    x >= clip.0 - 0.5 && x <= clip.2 + 0.5 && y >= clip.1 - 0.5 && y <= clip.3 + 0.5
}

/// Liang-Barsky clipping of a segment to the clip rectangle.
fn clip_seg(c: (f32, f32, f32, f32), a: (f32, f32), b: (f32, f32)) -> Option<((f32, f32), (f32, f32))> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let mut t0 = 0.0f32;
    let mut t1 = 1.0f32;
    for (p, q) in [(-dx, a.0 - c.0), (dx, c.2 - a.0), (-dy, a.1 - c.1), (dy, c.3 - a.1)] {
        if p.abs() < 1e-9 {
            if q < 0.0 {
                return None;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
            if t0 > t1 {
                return None;
            }
        }
    }
    Some(((a.0 + t0 * dx, a.1 + t0 * dy), (a.0 + t1 * dx, a.1 + t1 * dy)))
}

fn emit_path(vp: &mut VPlot, path: &[(Vec<(f32, f32)>, bool, bool)], fill: bool, stroke: bool, close: bool, gs: &GState) {
    for (pts, curved, closed) in path {
        if pts.is_empty() {
            continue;
        }
        let bb = bbox(&[(pts.clone(), *curved, *closed)]).unwrap_or((0.0, 0.0, 0.0, 0.0));
        let (bw, bh) = (bb.2 - bb.0, bb.3 - bb.1);
        // small closed curves are plotting symbols
        if *curved && bw < 12.0 && bh < 12.0 {
            let (cx, cy) = ((bb.0 + bb.2) / 2.0, (bb.1 + bb.3) / 2.0);
            if inside(gs.clip, cx, cy) {
                let col = if fill { gs.fill } else { gs.stroke };
                vp.prims.push(Prim::Point { x: cx, y: cy, filled: fill, col });
            }
            continue;
        }
        if fill && pts.len() >= 3 {
            // clip the polygon's points to the clip box (good enough for rects)
            let c = gs.clip;
            let poly: Vec<(f32, f32)> = pts.iter().map(|p| (p.0.clamp(c.0, c.2), p.1.clamp(c.1, c.3))).collect();
            vp.prims.push(Prim::Fill { pts: poly, col: gs.fill });
        }
        if stroke {
            let mut segs: Vec<((f32, f32), (f32, f32))> = pts.windows(2).map(|w| (w[0], w[1])).collect();
            if (*closed || close) && pts.len() > 2 {
                segs.push((pts[pts.len() - 1], pts[0]));
            }
            for (a, b) in segs {
                if let Some((a, b)) = clip_seg(gs.clip, a, b) {
                    vp.prims.push(Prim::Line { a, b, col: gs.stroke, w: gs.lw });
                }
            }
        }
    }
}

fn push_text(vp: &mut VPlot, s: String, tm: &[f32; 6], gs: &GState) {
    if s.trim().is_empty() {
        return;
    }
    let size = (tm[0] * tm[0] + tm[1] * tm[1]).sqrt().max(1.0);
    let angle = tm[1].atan2(tm[0]).to_degrees();
    if !inside(gs.clip, tm[4], tm[5]) {
        return;
    }
    vp.prims.push(Prim::Text { x: tm[4], y: tm[5], size, angle, s, col: gs.fill });
}

// ---------- rasterizing onto the character grid ----------

fn lum(c: Rgb) -> f32 {
    (0.299 * c.0 as f32 + 0.587 * c.1 as f32 + 0.114 * c.2 as f32) / 255.0
}

#[derive(Clone, Copy)]
struct Cell {
    ch: char,
    col: Option<Rgb>,
    prio: u8,
    dots: u8,
}

const P_FILL: u8 = 1;
const P_FAINT: u8 = 2;
const P_LINE: u8 = 3;
const P_POINT: u8 = 4;
const P_TEXT: u8 = 5;

struct Grid {
    cols: usize,
    rows: usize,
    cells: Vec<Cell>,
}

impl Grid {
    fn get(&mut self, c: isize, r: isize) -> Option<&mut Cell> {
        if c < 0 || r < 0 || c as usize >= self.cols || r as usize >= self.rows {
            return None;
        }
        self.cells.get_mut(r as usize * self.cols + c as usize)
    }
    fn put(&mut self, c: isize, r: isize, ch: char, col: Rgb, prio: u8) {
        if let Some(cell) = self.get(c, r) {
            if prio >= cell.prio {
                cell.ch = ch;
                cell.col = Some(col);
                cell.prio = prio;
                cell.dots = 0;
            }
        }
    }
}

/// Render a vector plot into exactly `cols` x `rows` cells.
pub fn render(vp: &VPlot, cols: usize, rows: usize, braille: bool, depth: ColorDepth) -> Rendered {
    let mut g = Grid { cols, rows, cells: vec![Cell { ch: ' ', col: None, prio: 0, dots: 0 }; cols * rows] };
    if cols == 0 || rows == 0 {
        return Rendered { cols, rows, cells: Vec::new() };
    }
    let sx = cols as f32 / vp.w;
    let sy = rows as f32 / vp.h;
    let tc = |x: f32, y: f32| -> (f32, f32) { (x * sx, (vp.h - y) * sy) };
    let page_area = vp.w * vp.h;

    // 1. fills (bars, areas); skip backgrounds and very light fills.
    // Each fill colour gets its own character so groups differ without colour.
    const ASCII_FILLS: [char; 8] = ['#', '=', '%', '@', '+', 'x', '~', '&'];
    const BRAILLE_FILLS: [char; 4] = ['█', '▓', '▒', '░'];
    let mut fill_cols: Vec<Rgb> = Vec::new();
    for p in &vp.prims {
        if let Prim::Fill { pts, col } = p {
            let area = poly_area(pts);
            if lum(*col) > 0.82 || area > page_area * 0.3 {
                continue;
            }
            let k = match fill_cols.iter().position(|c| c == col) {
                Some(k) => k,
                None => {
                    fill_cols.push(*col);
                    fill_cols.len() - 1
                }
            };
            let ch = if braille { BRAILLE_FILLS[k % BRAILLE_FILLS.len()] } else { ASCII_FILLS[k % ASCII_FILLS.len()] };
            fill_poly(&mut g, pts, &tc, ch, *col);
        }
    }
    // 2. lines (light ones = grid lines, drawn faintly)
    for p in &vp.prims {
        if let Prim::Line { a, b, col, w } = p {
            let faint = lum(*col) > 0.8;
            // light thin lines are minor grid lines: leave them out
            if faint && *w < 0.8 {
                continue;
            }
            let a = tc(a.0, a.1);
            let b = tc(b.0, b.1);
            if braille && !faint {
                braille_line(&mut g, a, b, *col);
            } else {
                ascii_line(&mut g, a, b, *col, faint);
            }
        }
    }
    // 3. points
    for p in &vp.prims {
        if let Prim::Point { x, y, filled, col } = p {
            let (cx, cy) = tc(*x, *y);
            let ch = match (braille, *filled) {
                (true, true) => '●',
                (true, false) => '○',
                (false, true) => '*',
                (false, false) => 'o',
            };
            g.put(cx.floor() as isize, cy.floor() as isize, ch, *col, P_POINT);
        }
    }
    // 4. text on top
    for p in &vp.prims {
        if let Prim::Text { x, y, size, angle, s, col } = p {
            let chars: Vec<char> = s.chars().collect();
            let n = chars.len() as f32;
            let w_r = n * size * 0.5; // R's approximate string width
            let a = angle.rem_euclid(360.0);
            let vertical = (60.0..120.0).contains(&a) || (240.0..300.0).contains(&a);
            if vertical && chars.len() > 4 {
                // column through the middle of the glyphs, text centered along y
                let up = (60.0..120.0).contains(&a);
                let (mx, my) = if up { (x - size * 0.3, y + w_r / 2.0) } else { (x + size * 0.3, y - w_r / 2.0) };
                let (cx, cy) = tc(mx, my);
                let start = (cy - n / 2.0).round() as isize;
                for (k, ch) in chars.iter().enumerate() {
                    g.put(cx.floor() as isize, start + k as isize, *ch, *col, P_TEXT);
                }
            } else {
                // centre of the string in page coordinates
                let rad = angle.to_radians();
                let (ux, uy) = (rad.cos(), rad.sin());
                let (mx, my) = (x + ux * w_r / 2.0 - uy * size * 0.3, y + uy * w_r / 2.0 + ux * size * 0.3);
                let (cx, cy) = tc(mx, my);
                let start = (cx - n / 2.0).round() as isize;
                let row = cy.floor() as isize;
                for (k, ch) in chars.iter().enumerate() {
                    g.put(start + k as isize, row, *ch, *col, P_TEXT);
                }
            }
        }
    }

    let cells = g
        .cells
        .iter()
        .map(|c| {
            let ch = if c.dots != 0 && c.ch == ' ' { char::from_u32(0x2800 + c.dots as u32).unwrap_or(' ') } else { c.ch };
            let fg = match c.col {
                Some(rgb) if c.prio == P_FAINT => {
                    let _ = rgb;
                    Color::DarkGray
                }
                Some((r, gg, b)) => ink_color(r, gg, b, depth),
                None => Color::Reset,
            };
            TCell { ch, fg, bg: Color::Reset }
        })
        .collect();
    Rendered { cols, rows, cells }
}

fn poly_area(pts: &[(f32, f32)]) -> f32 {
    let n = pts.len();
    let mut a = 0.0;
    for i in 0..n {
        let (x0, y0) = pts[i];
        let (x1, y1) = pts[(i + 1) % n];
        a += x0 * y1 - x1 * y0;
    }
    (a / 2.0).abs()
}

fn fill_poly(g: &mut Grid, pts: &[(f32, f32)], tc: &dyn Fn(f32, f32) -> (f32, f32), ch: char, col: Rgb) {
    let cp: Vec<(f32, f32)> = pts.iter().map(|p| tc(p.0, p.1)).collect();
    let (mut y0, mut y1) = (f32::MAX, f32::MIN);
    for p in &cp {
        y0 = y0.min(p.1);
        y1 = y1.max(p.1);
    }
    let r0 = y0.floor().max(0.0) as isize;
    let r1 = (y1.ceil() as isize).min(g.rows as isize);
    for r in r0..r1 {
        let yc = r as f32 + 0.5;
        let mut xs: Vec<f32> = Vec::new();
        for i in 0..cp.len() {
            let (a, b) = (cp[i], cp[(i + 1) % cp.len()]);
            if (a.1 <= yc && b.1 > yc) || (b.1 <= yc && a.1 > yc) {
                xs.push(a.0 + (yc - a.1) / (b.1 - a.1) * (b.0 - a.0));
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        for pair in xs.chunks(2) {
            if pair.len() < 2 {
                continue;
            }
            let c0 = (pair[0] + 0.5).floor() as isize;
            let c1 = (pair[1] - 0.5).ceil() as isize;
            for c in c0..=c1.max(c0) {
                g.put(c, r, ch, col, P_FILL);
            }
        }
    }
}

fn ascii_line(g: &mut Grid, a: (f32, f32), b: (f32, f32), col: Rgb, faint: bool) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    // visual angle: a row is about twice as tall as a column is wide
    let ang = (-dy * 2.0).atan2(dx).to_degrees().rem_euclid(180.0);
    let kind = if !(22.5..157.5).contains(&ang) {
        '-'
    } else if (67.5..112.5).contains(&ang) {
        '|'
    } else if ang < 90.0 {
        '/'
    } else {
        '\\'
    };
    let steps = ((dx.abs().max(dy.abs())) * 3.0).ceil().max(1.0) as usize;
    let mut last = (isize::MIN, isize::MIN);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let (x, y) = (a.0 + dx * t, a.1 + dy * t);
        let cell = (x.floor() as isize, y.floor() as isize);
        if cell == last {
            continue;
        }
        last = cell;
        let mut ch = kind;
        if kind == '-' {
            let frac = y - y.floor();
            if frac > 0.75 {
                ch = '_';
            }
        }
        if faint {
            if let Some(c) = g.get(cell.0, cell.1) {
                if c.prio == 0 {
                    c.ch = if kind == '|' { ':' } else { '.' };
                    c.col = Some(col);
                    c.prio = P_FAINT;
                }
            }
            continue;
        }
        if let Some(c) = g.get(cell.0, cell.1) {
            if c.prio > P_LINE {
                continue;
            }
            // crossing lines become '+'
            if c.prio == P_LINE && c.ch != ch && matches!((c.ch, ch), ('-', '|') | ('|', '-') | ('_', '|') | ('|', '_')) {
                ch = '+';
            }
            c.ch = ch;
            c.col = Some(col);
            c.prio = P_LINE;
        }
    }
}

fn braille_line(g: &mut Grid, a: (f32, f32), b: (f32, f32), col: Rgb) {
    const BITS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
    // work in dot space: 2 dots per column, 4 per row
    let (ax, ay, bx, by) = (a.0 * 2.0, a.1 * 4.0, b.0 * 2.0, b.1 * 4.0);
    let steps = ((bx - ax).abs().max((by - ay).abs())).ceil().max(1.0) as usize;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let (x, y) = (ax + (bx - ax) * t, ay + (by - ay) * t);
        let (dxi, dyi) = (x.floor() as isize, y.floor() as isize);
        let (c, r) = (dxi.div_euclid(2), dyi.div_euclid(4));
        let bit = BITS[dxi.rem_euclid(2) as usize][dyi.rem_euclid(4) as usize];
        if let Some(cell) = g.get(c, r) {
            if cell.prio > P_LINE {
                continue;
            }
            if cell.prio < P_LINE {
                cell.ch = ' ';
            }
            cell.dots |= bit;
            cell.col = Some(col);
            cell.prio = P_LINE;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PDF: &str = "%PDF-1.4\n7 0 obj\n<< /Type /Page /Contents 8 0 R >>\nendobj\n8 0 obj\n<< /Length 1 >>\nstream\n\
q 0 0 100 50 re W n\n0 0 0 RG\n10 10 m 90 10 l S\n\
BT /F2 1 Tf 10.00 0.00 0.00 10.00 40 30 Tm (Hi \\(x\\)) Tj ET\n\
5 20 m 5 22 7 22 7 20 c 7 18 5 18 5 20 c f\nQ\nendstream\nendobj\n\
3 0 obj << /Type /Pages /MediaBox [0 0 100 50] >>\n";

    #[test]
    fn parse_basic() {
        let v = parse_pdf(PDF.as_bytes()).expect("parse");
        assert_eq!(v.w, 100.0);
        assert!(v.prims.iter().any(|p| matches!(p, Prim::Line { .. })));
        assert!(v.prims.iter().any(|p| matches!(p, Prim::Point { filled: true, .. })));
        assert!(v.prims.iter().any(|p| matches!(p, Prim::Text { s, .. } if s == "Hi (x)")));
    }

    #[test]
    fn render_has_text() {
        let v = parse_pdf(PDF.as_bytes()).expect("parse");
        let r = render(&v, 20, 5, false, ColorDepth::Mono);
        let text: String = r.cells.iter().map(|c| c.ch).collect();
        assert!(text.contains("Hi (x)"), "{text}");
        assert!(text.contains("---"));
    }
}
