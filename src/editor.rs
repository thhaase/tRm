//! Text document model: lines of chars, cursor/selection, operation-based
//! undo, lexer state cache, and R-aware helpers (statement detection,
//! outline, commenting, indentation).

use crate::highlight::{lex_line, line_info, LexState, LineInfo, Span, Tok};
use crate::ui::{char_width, TAB_WIDTH};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl Pos {
    pub fn new(line: usize, col: usize) -> Pos {
        Pos { line, col }
    }
}

#[derive(Clone, Debug)]
struct Edit {
    pos: Pos,
    removed: String,
    inserted: String,
}

#[derive(Clone, Debug)]
struct Group {
    edits: Vec<Edit>,
    before: (Pos, Option<Pos>),
    after: (Pos, Option<Pos>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditKind {
    Typing,
    Deleting,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LintKind {
    Error,
    Warning,
    Style,
}

#[derive(Clone, Debug)]
pub struct Lint {
    pub line: usize,
    pub col: usize,
    pub kind: LintKind,
    pub msg: String,
}

#[derive(Clone, Debug)]
pub struct OutlineItem {
    pub line: usize,
    pub level: usize,
    pub title: String,
    pub is_fn: bool,
}

pub struct Doc {
    pub id: u64,
    pub lines: Vec<Vec<char>>,
    pub path: Option<PathBuf>,
    pub modified: bool,
    pub cur: Pos,
    pub anchor: Option<Pos>,
    pub want_col: Option<usize>,
    pub top: usize,
    pub left: usize,
    undo: Vec<Group>,
    redo: Vec<Group>,
    open_kind: Option<EditKind>,
    saved_undo_len: Option<usize>,
    states: Vec<LexState>,
    valid: usize,
    pub version: u64,
    pub lints: Vec<Lint>,
    pub crlf: bool,
    outline_cache: Option<(u64, Vec<OutlineItem>)>,
}

const MAX_UNDO: usize = 500;

impl Doc {
    pub fn new() -> Doc {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Doc {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            lines: vec![Vec::new()],
            path: None,
            modified: false,
            cur: Pos::default(),
            anchor: None,
            want_col: None,
            top: 0,
            left: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            open_kind: None,
            saved_undo_len: Some(0),
            states: vec![LexState::Normal],
            valid: 0,
            version: 0,
            lints: Vec::new(),
            crlf: false,
            outline_cache: None,
        }
    }

    pub fn from_text(text: &str, path: Option<PathBuf>) -> Doc {
        let mut d = Doc::new();
        d.crlf = text.contains("\r\n");
        let text = text.replace("\r\n", "\n");
        d.lines = text.split('\n').map(|l| l.chars().collect()).collect();
        if d.lines.is_empty() {
            d.lines.push(Vec::new());
        }
        d.path = path;
        d.states = vec![LexState::Normal; d.lines.len()];
        d
    }

    pub fn load(path: &Path) -> Result<Doc, String> {
        let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
        if meta.len() > 20 * 1024 * 1024 {
            return Err("file is larger than 20 MB".into());
        }
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        if bytes.iter().take(8192).any(|b| *b == 0) {
            return Err("looks like a binary file".into());
        }
        let text = String::from_utf8_lossy(&bytes);
        Ok(Doc::from_text(&text, Some(path.to_path_buf())))
    }

    pub fn text(&self) -> String {
        let nl = if self.crlf { "\r\n" } else { "\n" };
        let mut s = String::new();
        for (i, l) in self.lines.iter().enumerate() {
            if i > 0 {
                s.push_str(nl);
            }
            s.extend(l.iter());
        }
        s
    }

    pub fn save(&mut self) -> Result<(), String> {
        let path = self.path.clone().ok_or("no file name")?;
        let text = self.text();
        // write to a temp file first so a failed write never destroys the original
        let tmp = path.with_file_name(format!(
            ".{}.trm-tmp",
            path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
        ));
        std::fs::write(&tmp, text.as_bytes()).map_err(|e| e.to_string())?;
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            // fall back to direct write (e.g. cross-device or permissions on dir)
            std::fs::write(&path, text.as_bytes()).map_err(|e2| format!("{e}; {e2}"))?;
        }
        self.modified = false;
        self.saved_undo_len = Some(self.undo.len());
        self.close_group();
        Ok(())
    }

    pub fn title(&self) -> String {
        match &self.path {
            Some(p) => p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "?".into()),
            None => "Untitled".into(),
        }
    }

    // ---------- basic geometry ----------

    pub fn line_len(&self, l: usize) -> usize {
        self.lines.get(l).map(|l| l.len()).unwrap_or(0)
    }

    pub fn clamp(&self, p: Pos) -> Pos {
        let line = p.line.min(self.lines.len().saturating_sub(1));
        Pos { line, col: p.col.min(self.line_len(line)) }
    }

    pub fn end_pos(&self) -> Pos {
        let l = self.lines.len().saturating_sub(1);
        Pos::new(l, self.line_len(l))
    }

    /// display column of char index `col` on line `l`
    pub fn disp_col(&self, l: usize, col: usize) -> usize {
        let Some(line) = self.lines.get(l) else { return 0 };
        let mut w = 0;
        for c in line.iter().take(col) {
            w += if *c == '\t' { TAB_WIDTH - (w % TAB_WIDTH) } else { char_width(*c) };
        }
        w
    }

    /// char index on line `l` closest to display column `x`
    pub fn col_at_disp(&self, l: usize, x: usize) -> usize {
        let Some(line) = self.lines.get(l) else { return 0 };
        let mut w = 0;
        for (i, c) in line.iter().enumerate() {
            let cw = if *c == '\t' { TAB_WIDTH - (w % TAB_WIDTH) } else { char_width(*c) };
            if w + cw > x {
                return if x >= w + (cw + 1) / 2 && cw > 1 { i + 1 } else { i };
            }
            w += cw;
        }
        line.len()
    }

    // ---------- selection ----------

    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let a = self.anchor?;
        if a == self.cur {
            return None;
        }
        Some(if a < self.cur { (a, self.cur) } else { (self.cur, a) })
    }

    pub fn text_range(&self, a: Pos, b: Pos) -> String {
        let a = self.clamp(a);
        let b = self.clamp(b);
        let mut s = String::new();
        if a.line == b.line {
            if let Some(l) = self.lines.get(a.line) {
                s.extend(l[a.col..b.col.max(a.col)].iter());
            }
            return s;
        }
        for l in a.line..=b.line {
            let line = &self.lines[l];
            if l == a.line {
                s.extend(line[a.col..].iter());
                s.push('\n');
            } else if l == b.line {
                s.extend(line[..b.col].iter());
            } else {
                s.extend(line.iter());
                s.push('\n');
            }
        }
        s
    }

    pub fn selected_text(&self) -> Option<String> {
        self.selection().map(|(a, b)| self.text_range(a, b))
    }

    pub fn move_to(&mut self, p: Pos, select: bool) {
        if select {
            if self.anchor.is_none() {
                self.anchor = Some(self.cur);
            }
        } else {
            self.anchor = None;
        }
        self.cur = self.clamp(p);
        self.close_group();
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(Pos::new(0, 0));
        self.cur = self.end_pos();
        self.close_group();
    }

    // ---------- primitive edits (recorded for undo) ----------

    fn invalidate(&mut self, line: usize) {
        self.valid = self.valid.min(line);
        self.states.resize(self.lines.len(), LexState::Normal);
        self.version = self.version.wrapping_add(1);
        self.modified = true;
        if !self.lints.is_empty() {
            self.lints.clear();
        }
    }

    fn insert_raw(&mut self, p: Pos, text: &str) -> Pos {
        let p = self.clamp(p);
        let tail: Vec<char> = self.lines[p.line].split_off(p.col);
        let mut parts = text.split('\n');
        let first = parts.next().unwrap_or("");
        self.lines[p.line].extend(first.chars());
        let mut line = p.line;
        let mut end_col = self.lines[line].len();
        for part in parts {
            line += 1;
            let v: Vec<char> = part.chars().collect();
            end_col = v.len();
            self.lines.insert(line, v);
        }
        self.lines[line].extend(tail);
        self.invalidate(p.line);
        Pos::new(line, end_col)
    }

    fn delete_raw(&mut self, a: Pos, b: Pos) -> String {
        let a = self.clamp(a);
        let b = self.clamp(b);
        if a >= b {
            return String::new();
        }
        let removed = self.text_range(a, b);
        if a.line == b.line {
            self.lines[a.line].drain(a.col..b.col);
        } else {
            let tail: Vec<char> = self.lines[b.line][b.col..].to_vec();
            self.lines[a.line].truncate(a.col);
            self.lines[a.line].extend(tail);
            self.lines.drain(a.line + 1..=b.line);
        }
        self.invalidate(a.line);
        removed
    }

    fn group_for(&mut self, kind: EditKind) {
        let continue_group = self.open_kind == Some(kind) && kind != EditKind::Other && !self.undo.is_empty();
        if !continue_group {
            self.undo.push(Group {
                edits: Vec::new(),
                before: (self.cur, self.anchor),
                after: (self.cur, self.anchor),
            });
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
                self.saved_undo_len = self.saved_undo_len.and_then(|n| n.checked_sub(1));
            }
            if let Some(n) = self.saved_undo_len {
                if n >= self.undo.len() {
                    self.saved_undo_len = None;
                }
            }
        }
        self.open_kind = Some(kind);
        if !self.redo.is_empty() {
            self.redo.clear();
        }
    }

    pub fn close_group(&mut self) {
        self.open_kind = None;
    }

    fn record(&mut self, e: Edit) {
        if let Some(g) = self.undo.last_mut() {
            g.edits.push(e);
        }
    }

    fn finish(&mut self) {
        let st = (self.cur, self.anchor);
        if let Some(g) = self.undo.last_mut() {
            g.after = st;
        }
        self.want_col = None;
    }

    /// Insert at `p` as part of an undo group of `kind`.
    pub fn insert_at(&mut self, p: Pos, text: &str, kind: EditKind) -> Pos {
        self.group_for(kind);
        let p = self.clamp(p);
        let end = self.insert_raw(p, text);
        self.record(Edit { pos: p, removed: String::new(), inserted: text.to_string() });
        end
    }

    pub fn delete_range(&mut self, a: Pos, b: Pos, kind: EditKind) -> String {
        self.group_for(kind);
        let a = self.clamp(a);
        let removed = self.delete_raw(a, b);
        if !removed.is_empty() {
            self.record(Edit { pos: a, removed: removed.clone(), inserted: String::new() });
        }
        removed
    }

    pub fn undo(&mut self) -> bool {
        self.close_group();
        let Some(g) = self.undo.pop() else { return false };
        for e in g.edits.iter().rev() {
            if !e.inserted.is_empty() {
                let end = end_of(e.pos, &e.inserted);
                self.delete_raw(e.pos, end);
            }
            if !e.removed.is_empty() {
                self.insert_raw(e.pos, &e.removed);
            }
        }
        self.cur = self.clamp(g.before.0);
        self.anchor = g.before.1.map(|p| self.clamp(p));
        self.redo.push(g);
        self.modified = self.saved_undo_len != Some(self.undo.len());
        true
    }

    pub fn redo(&mut self) -> bool {
        self.close_group();
        let Some(g) = self.redo.pop() else { return false };
        for e in g.edits.iter() {
            if !e.removed.is_empty() {
                let end = end_of(e.pos, &e.removed);
                self.delete_raw(e.pos, end);
            }
            if !e.inserted.is_empty() {
                self.insert_raw(e.pos, &e.inserted);
            }
        }
        self.cur = self.clamp(g.after.0);
        self.anchor = g.after.1.map(|p| self.clamp(p));
        self.undo.push(g);
        self.modified = self.saved_undo_len != Some(self.undo.len());
        true
    }

    // ---------- high level editing ----------

    pub fn delete_selection(&mut self, kind: EditKind) -> bool {
        if let Some((a, b)) = self.selection() {
            self.delete_range(a, b, kind);
            self.cur = a;
            self.anchor = None;
            self.finish();
            true
        } else {
            self.anchor = None;
            false
        }
    }

    /// Insert text at the cursor, replacing the selection.
    pub fn insert_text(&mut self, text: &str) {
        let kind = if text.chars().count() == 1 && !text.contains('\n') { EditKind::Typing } else { EditKind::Other };
        if self.selection().is_some() {
            self.delete_selection(EditKind::Other);
        }
        self.anchor = None;
        let end = self.insert_at(self.cur, text, kind);
        self.cur = end;
        self.finish();
    }

    /// Type a single character with auto-pairing of brackets and quotes.
    pub fn type_char(&mut self, c: char, auto_pairs: bool) {
        let line = self.lines.get(self.cur.line).cloned().unwrap_or_default();
        let next = line.get(self.cur.col).copied();
        if auto_pairs {
            // wrap selection in brackets/quotes
            if let Some((a, b)) = self.selection() {
                if let Some(close) = pair_close(c) {
                    let inner = self.text_range(a, b);
                    self.delete_selection(EditKind::Other);
                    let s = format!("{c}{inner}{close}");
                    let end = self.insert_at(self.cur, &s, EditKind::Other);
                    self.cur = end;
                    self.finish();
                    return;
                }
            }
            // skip over an existing closer
            if matches!(c, ')' | ']' | '}' | '"' | '\'' | '`') && next == Some(c) {
                self.move_to(Pos::new(self.cur.line, self.cur.col + 1), false);
                return;
            }
            if let Some(close) = pair_close(c) {
                let prev = if self.cur.col > 0 { line.get(self.cur.col - 1).copied() } else { None };
                let is_quote = matches!(c, '"' | '\'' | '`');
                let next_ok = next.map(|n| n.is_whitespace() || ")]},;".contains(n)).unwrap_or(true);
                let prev_ok = !is_quote || prev.map(|p| !p.is_alphanumeric() && p != '\\').unwrap_or(true);
                let in_str = self.in_string_or_comment(self.cur);
                if next_ok && prev_ok && !in_str {
                    let s = format!("{c}{close}");
                    self.insert_at(self.cur, &s, EditKind::Typing);
                    self.cur = Pos::new(self.cur.line, self.cur.col + 1);
                    self.finish();
                    return;
                }
            }
        }
        // a closing brace on an otherwise blank line dedents
        if c == '}' && line.iter().all(|c| c.is_whitespace()) && self.cur.col == line.len() && self.selection().is_none() {
            let ind = self.indent_before(self.cur.line);
            let cur_line = self.cur.line;
            self.delete_range(Pos::new(cur_line, 0), Pos::new(cur_line, line.len()), EditKind::Other);
            let s = format!("{ind}}}");
            let end = self.insert_at(Pos::new(cur_line, 0), &s, EditKind::Other);
            self.cur = end;
            self.anchor = None;
            self.finish();
            return;
        }
        let mut b = [0u8; 4];
        self.insert_text(c.encode_utf8(&mut b));
    }

    /// Indentation to use for a closing brace: the indentation of the line that opened the block.
    fn indent_before(&mut self, line: usize) -> String {
        let mut depth = 0i32;
        let mut l = line;
        while l > 0 {
            l -= 1;
            let info = self.info(l);
            depth += info.delta;
            if depth > 0 {
                return leading_ws(&self.lines[l]);
            }
            if line - l > 2000 {
                break;
            }
        }
        String::new()
    }

    pub fn newline(&mut self) {
        if self.selection().is_some() {
            self.delete_selection(EditKind::Other);
        }
        let l = self.cur.line;
        let line = self.lines[l].clone();
        let before: String = line[..self.cur.col].iter().collect();
        let after: String = line[self.cur.col..].iter().collect();
        let base = leading_ws(&line);
        let mut ind = base.clone();
        let bt = before.trim_end();
        let opens = bt.ends_with('{') || bt.ends_with('(') || bt.ends_with('[');
        let ends_pipe = bt.ends_with("|>") || bt.ends_with("%>%") || bt.ends_with('+');
        if opens {
            ind.push_str("  ");
        } else if ends_pipe {
            // indent the first continuation line of a pipe chain only
            let prev_pipe = l > 0 && {
                let p: String = self.lines[l - 1].iter().collect();
                let p = p.trim_end();
                p.ends_with("|>") || p.ends_with("%>%") || p.ends_with('+')
            };
            if !prev_pipe {
                ind.push_str("  ");
            }
        } else if l > 0 && !bt.is_empty() {
            // end of a pipe chain: return to the chain's start indentation
            let p: String = self.lines[l - 1].iter().collect();
            let p = p.trim_end();
            if p.ends_with("|>") || p.ends_with("%>%") || p.ends_with('+') {
                let mut k = l - 1;
                while k > 0 {
                    let q: String = self.lines[k - 1].iter().collect();
                    let q = q.trim_end();
                    if q.ends_with("|>") || q.ends_with("%>%") || q.ends_with('+') {
                        k -= 1;
                    } else {
                        break;
                    }
                }
                ind = leading_ws(&self.lines[k]);
            }
        }
        let at = after.trim_start();
        let between_pair = opens && (at.starts_with('}') || at.starts_with(')') || at.starts_with(']'));
        let text = if between_pair {
            format!("\n{ind}\n{base}")
        } else {
            format!("\n{ind}")
        };
        // drop whitespace right after the cursor so the new line starts clean
        let ws_after = after.len() - after.trim_start().len();
        let cur = self.cur;
        if ws_after > 0 {
            self.delete_range(cur, Pos::new(cur.line, cur.col + after[..ws_after].chars().count()), EditKind::Other);
        }
        self.insert_at(cur, &text, EditKind::Other);
        self.cur = Pos::new(l + 1, ind.chars().count());
        self.anchor = None;
        self.finish();
        self.close_group();
    }

    pub fn backspace(&mut self, auto_pairs: bool) {
        if self.delete_selection(EditKind::Other) {
            return;
        }
        let p = self.cur;
        if p.col == 0 {
            if p.line == 0 {
                return;
            }
            let prev = Pos::new(p.line - 1, self.line_len(p.line - 1));
            self.delete_range(prev, p, EditKind::Deleting);
            self.cur = prev;
        } else {
            let line = &self.lines[p.line];
            let prev_c = line[p.col - 1];
            let next_c = line.get(p.col).copied();
            // remove an empty auto-pair
            if auto_pairs && pair_close(prev_c).is_some() && next_c == pair_close(prev_c) {
                self.delete_range(Pos::new(p.line, p.col - 1), Pos::new(p.line, p.col + 1), EditKind::Deleting);
                self.cur = Pos::new(p.line, p.col - 1);
            } else if line[..p.col].iter().all(|c| *c == ' ') && p.col >= 2 {
                // dedent in steps of two spaces
                let n = if p.col % 2 == 0 { 2 } else { 1 };
                self.delete_range(Pos::new(p.line, p.col - n), p, EditKind::Deleting);
                self.cur = Pos::new(p.line, p.col - n);
            } else {
                self.delete_range(Pos::new(p.line, p.col - 1), p, EditKind::Deleting);
                self.cur = Pos::new(p.line, p.col - 1);
            }
        }
        self.finish();
    }

    pub fn delete_forward(&mut self) {
        if self.delete_selection(EditKind::Other) {
            return;
        }
        let p = self.cur;
        let end = if p.col < self.line_len(p.line) {
            Pos::new(p.line, p.col + 1)
        } else if p.line + 1 < self.lines.len() {
            Pos::new(p.line + 1, 0)
        } else {
            return;
        };
        self.delete_range(p, end, EditKind::Deleting);
        self.finish();
    }

    pub fn delete_word_back(&mut self) {
        if self.delete_selection(EditKind::Other) {
            return;
        }
        let start = self.word_left(self.cur);
        let cur = self.cur;
        self.delete_range(start, cur, EditKind::Other);
        self.cur = start;
        self.finish();
    }

    fn selected_lines(&self) -> (usize, usize) {
        match self.selection() {
            Some((a, b)) => {
                let last = if b.col == 0 && b.line > a.line { b.line - 1 } else { b.line };
                (a.line, last)
            }
            None => (self.cur.line, self.cur.line),
        }
    }

    pub fn indent(&mut self, outdent: bool) {
        let (a, b) = self.selected_lines();
        if !outdent && self.selection().is_none() {
            // plain tab: insert spaces up to the next multiple of two
            let n = 2 - (self.cur.col % 2);
            self.insert_text(&" ".repeat(n));
            return;
        }
        self.group_for(EditKind::Other);
        let had_sel = self.selection().is_some();
        for l in a..=b {
            if outdent {
                let n = self.lines[l].iter().take(2).take_while(|c| **c == ' ').count();
                let n = if n == 0 && self.lines[l].first() == Some(&'\t') { 1 } else { n };
                if n > 0 {
                    let removed = self.delete_raw(Pos::new(l, 0), Pos::new(l, n));
                    self.record(Edit { pos: Pos::new(l, 0), removed, inserted: String::new() });
                    if l == self.cur.line {
                        self.cur.col = self.cur.col.saturating_sub(n);
                    }
                    if let Some(an) = self.anchor.as_mut() {
                        if an.line == l {
                            an.col = an.col.saturating_sub(n);
                        }
                    }
                }
            } else if !self.lines[l].is_empty() {
                self.insert_raw(Pos::new(l, 0), "  ");
                self.record(Edit { pos: Pos::new(l, 0), removed: String::new(), inserted: "  ".into() });
                if l == self.cur.line {
                    self.cur.col += 2;
                }
                if let Some(an) = self.anchor.as_mut() {
                    if an.line == l && an.col > 0 {
                        an.col += 2;
                    }
                }
            }
        }
        if !had_sel {
            self.anchor = None;
        }
        self.finish();
        self.close_group();
    }

    pub fn toggle_comment(&mut self) {
        let (a, b) = self.selected_lines();
        let all_commented = (a..=b).all(|l| {
            let t: String = self.lines[l].iter().collect();
            let t = t.trim_start();
            t.is_empty() || t.starts_with('#')
        });
        let min_ind = (a..=b)
            .filter(|l| self.lines[*l].iter().any(|c| !c.is_whitespace()))
            .map(|l| self.lines[l].iter().take_while(|c| c.is_whitespace()).count())
            .min()
            .unwrap_or(0);
        self.group_for(EditKind::Other);
        for l in a..=b {
            if self.lines[l].iter().all(|c| c.is_whitespace()) {
                continue;
            }
            if all_commented {
                let i = self.lines[l].iter().position(|c| *c == '#').unwrap_or(0);
                let n = if self.lines[l].get(i + 1) == Some(&' ') { 2 } else { 1 };
                let removed = self.delete_raw(Pos::new(l, i), Pos::new(l, i + n));
                self.record(Edit { pos: Pos::new(l, i), removed, inserted: String::new() });
                if l == self.cur.line && self.cur.col > i {
                    self.cur.col = self.cur.col.saturating_sub(n).max(i);
                }
            } else {
                self.insert_raw(Pos::new(l, min_ind), "# ");
                self.record(Edit { pos: Pos::new(l, min_ind), removed: String::new(), inserted: "# ".into() });
                if l == self.cur.line && self.cur.col >= min_ind {
                    self.cur.col += 2;
                }
            }
        }
        if let Some(an) = self.anchor {
            self.anchor = Some(self.clamp(an));
        }
        self.cur = self.clamp(self.cur);
        self.finish();
        self.close_group();
    }

    pub fn delete_lines(&mut self) {
        let (a, b) = self.selected_lines();
        let (start, end) = if b + 1 < self.lines.len() {
            (Pos::new(a, 0), Pos::new(b + 1, 0))
        } else if a > 0 {
            (Pos::new(a - 1, self.line_len(a - 1)), Pos::new(b, self.line_len(b)))
        } else {
            (Pos::new(a, 0), Pos::new(b, self.line_len(b)))
        };
        self.delete_range(start, end, EditKind::Other);
        self.anchor = None;
        self.cur = self.clamp(Pos::new(a, 0));
        self.finish();
        self.close_group();
    }

    pub fn move_lines(&mut self, up: bool) {
        let (a, b) = self.selected_lines();
        if (up && a == 0) || (!up && b + 1 >= self.lines.len()) {
            return;
        }
        let block: Vec<String> = (a..=b).map(|l| self.lines[l].iter().collect()).collect();
        let block = block.join("\n");
        self.group_for(EditKind::Other);
        let cur = self.cur;
        let anchor = self.anchor;
        if up {
            let other: String = self.lines[a - 1].iter().collect();
            let start = Pos::new(a - 1, 0);
            let end = Pos::new(b, self.line_len(b));
            let removed = self.delete_raw(start, end);
            self.record(Edit { pos: start, removed, inserted: String::new() });
            let text = format!("{block}\n{other}");
            self.insert_raw(start, &text);
            self.record(Edit { pos: start, removed: String::new(), inserted: text });
            self.cur = Pos::new(cur.line - 1, cur.col);
            self.anchor = anchor.map(|p| Pos::new(p.line.saturating_sub(1), p.col));
        } else {
            let other: String = self.lines[b + 1].iter().collect();
            let start = Pos::new(a, 0);
            let end = Pos::new(b + 1, self.line_len(b + 1));
            let removed = self.delete_raw(start, end);
            self.record(Edit { pos: start, removed, inserted: String::new() });
            let text = format!("{other}\n{block}");
            self.insert_raw(start, &text);
            self.record(Edit { pos: start, removed: String::new(), inserted: text });
            self.cur = Pos::new(cur.line + 1, cur.col);
            self.anchor = anchor.map(|p| Pos::new(p.line + 1, p.col));
        }
        self.cur = self.clamp(self.cur);
        self.finish();
        self.close_group();
    }

    /// Replace every occurrence; returns the number of replacements.
    pub fn replace_all(&mut self, needle: &str, repl: &str) -> usize {
        if needle.is_empty() {
            return 0;
        }
        let mut count = 0;
        let mut p = Pos::new(0, 0);
        let needle_len = needle.chars().count();
        self.group_for(EditKind::Other);
        let mut guard = 0;
        while let Some(hit) = self.find_from(needle, p, false, false) {
            if hit < p || guard > 100_000 {
                break;
            }
            guard += 1;
            let end = Pos::new(hit.line, hit.col + needle_len);
            let removed = self.delete_raw(hit, end);
            self.record(Edit { pos: hit, removed, inserted: String::new() });
            let e = self.insert_raw(hit, repl);
            self.record(Edit { pos: hit, removed: String::new(), inserted: repl.to_string() });
            p = e;
            count += 1;
        }
        self.cur = self.clamp(self.cur);
        self.anchor = None;
        self.finish();
        self.close_group();
        count
    }

    // ---------- movement ----------

    pub fn word_left(&self, p: Pos) -> Pos {
        if p.col == 0 {
            return if p.line > 0 { Pos::new(p.line - 1, self.line_len(p.line - 1)) } else { p };
        }
        let line = &self.lines[p.line];
        let mut i = p.col;
        while i > 0 && line[i - 1].is_whitespace() {
            i -= 1;
        }
        if i > 0 && is_word(line[i - 1]) {
            while i > 0 && is_word(line[i - 1]) {
                i -= 1;
            }
        } else if i > 0 {
            i -= 1;
        }
        Pos::new(p.line, i)
    }

    pub fn word_right(&self, p: Pos) -> Pos {
        let len = self.line_len(p.line);
        if p.col >= len {
            return if p.line + 1 < self.lines.len() { Pos::new(p.line + 1, 0) } else { p };
        }
        let line = &self.lines[p.line];
        let mut i = p.col;
        if is_word(line[i]) {
            while i < len && is_word(line[i]) {
                i += 1;
            }
        } else if !line[i].is_whitespace() {
            i += 1;
        }
        while i < len && line[i].is_whitespace() {
            i += 1;
        }
        Pos::new(p.line, i)
    }

    pub fn word_at(&self, p: Pos) -> (Pos, Pos) {
        let Some(line) = self.lines.get(p.line) else { return (p, p) };
        let mut a = p.col.min(line.len());
        let mut b = a;
        while a > 0 && is_word(line[a - 1]) {
            a -= 1;
        }
        while b < line.len() && is_word(line[b]) {
            b += 1;
        }
        (Pos::new(p.line, a), Pos::new(p.line, b))
    }

    pub fn vertical(&mut self, delta: isize, select: bool) {
        let want = self.want_col.unwrap_or_else(|| self.disp_col(self.cur.line, self.cur.col));
        let line = (self.cur.line as isize + delta).clamp(0, self.lines.len() as isize - 1) as usize;
        let col = self.col_at_disp(line, want);
        self.move_to(Pos::new(line, col), select);
        self.want_col = Some(want);
    }

    pub fn home(&mut self, select: bool) {
        let ind = self.lines[self.cur.line].iter().take_while(|c| c.is_whitespace()).count();
        let col = if self.cur.col == ind { 0 } else { ind };
        self.move_to(Pos::new(self.cur.line, col), select);
    }

    pub fn ensure_visible(&mut self, height: usize, width: usize) {
        let height = height.max(1);
        if self.cur.line < self.top {
            self.top = self.cur.line;
        } else if self.cur.line >= self.top + height {
            self.top = self.cur.line + 1 - height;
        }
        let x = self.disp_col(self.cur.line, self.cur.col);
        let width = width.max(2);
        if x < self.left {
            self.left = x;
        } else if x >= self.left + width - 1 {
            self.left = x + 2 - width;
        }
    }

    // ---------- search ----------

    pub fn find_from(&self, needle: &str, from: Pos, backwards: bool, smart_case: bool) -> Option<Pos> {
        if needle.is_empty() {
            return None;
        }
        let ci = smart_case && !needle.chars().any(|c| c.is_uppercase());
        let norm = |c: char| if ci { c.to_lowercase().next().unwrap_or(c) } else { c };
        let pat: Vec<char> = needle.chars().map(norm).collect();
        if pat.contains(&'\n') {
            return None;
        }
        let n = self.lines.len();
        let matches_at = |l: usize, i: usize| -> bool {
            let line = &self.lines[l];
            i + pat.len() <= line.len() && pat.iter().enumerate().all(|(k, pc)| norm(line[i + k]) == *pc)
        };
        if !backwards {
            for k in 0..=n {
                let l = (from.line + k) % n;
                let start = if k == 0 { from.col } else { 0 };
                let len = self.lines[l].len();
                let mut i = start;
                while i + pat.len() <= len {
                    if matches_at(l, i) {
                        return Some(Pos::new(l, i));
                    }
                    i += 1;
                }
                if k == n {
                    break;
                }
            }
        } else {
            for k in 0..=n {
                let l = (from.line + n * 2 - k) % n;
                let len = self.lines[l].len();
                let end = if k == 0 { from.col.min(len) } else { len };
                let mut i = end;
                while i > 0 {
                    i -= 1;
                    if matches_at(l, i) {
                        return Some(Pos::new(l, i));
                    }
                }
            }
        }
        None
    }

    // ---------- lexing ----------

    pub fn ensure_states(&mut self, upto: usize) {
        if self.states.len() != self.lines.len() {
            self.states.resize(self.lines.len(), LexState::Normal);
        }
        let upto = upto.min(self.lines.len().saturating_sub(1));
        if let Some(s) = self.states.get_mut(0) {
            *s = LexState::Normal;
        }
        let mut spans = Vec::new();
        while self.valid < upto {
            let l = self.valid;
            spans.clear();
            let st = lex_line(&self.lines[l], self.states[l], &mut spans);
            self.states[l + 1] = st;
            self.valid += 1;
        }
    }

    pub fn state_at(&mut self, l: usize) -> LexState {
        self.ensure_states(l);
        self.states.get(l).copied().unwrap_or_default()
    }

    pub fn spans(&mut self, l: usize, out: &mut Vec<Span>) {
        out.clear();
        let st = self.state_at(l);
        if let Some(line) = self.lines.get(l) {
            lex_line(line, st, out);
        }
    }

    pub fn info(&mut self, l: usize) -> LineInfo {
        let st = self.state_at(l);
        match self.lines.get(l) {
            Some(line) => line_info(line, st),
            None => LineInfo::default(),
        }
    }

    fn in_string_or_comment(&mut self, p: Pos) -> bool {
        let mut spans = Vec::new();
        self.spans(p.line, &mut spans);
        spans.iter().any(|s| {
            matches!(s.tok, Tok::Str | Tok::Comment | Tok::Roxygen | Tok::Section) && s.start < p.col && p.col < s.end
                || (matches!(s.tok, Tok::Comment | Tok::Roxygen | Tok::Section) && s.start < p.col)
        })
    }

    /// Find the bracket matching the one at/before the cursor.
    pub fn matching_bracket(&mut self) -> Option<(Pos, Pos)> {
        let mut spans = Vec::new();
        self.spans(self.cur.line, &mut spans);
        let line = self.lines.get(self.cur.line)?.clone();
        let at = |col: usize| spans.iter().any(|s| s.tok == Tok::Bracket && s.start == col);
        let col = if self.cur.col < line.len() && at(self.cur.col) {
            self.cur.col
        } else if self.cur.col > 0 && at(self.cur.col - 1) {
            self.cur.col - 1
        } else {
            return None;
        };
        let c = line[col];
        let start = Pos::new(self.cur.line, col);
        let (open, close, forward) = match c {
            '(' => ('(', ')', true),
            '[' => ('[', ']', true),
            '{' => ('{', '}', true),
            ')' => ('(', ')', false),
            ']' => ('[', ']', false),
            '}' => ('{', '}', false),
            _ => return None,
        };
        let mut depth = 0i32;
        let limit = 3000;
        if forward {
            for l in start.line..(start.line + limit).min(self.lines.len()) {
                self.spans(l, &mut spans);
                for s in &spans {
                    if s.tok != Tok::Bracket || (l == start.line && s.start < col) {
                        continue;
                    }
                    let ch = self.lines[l][s.start];
                    if ch == open {
                        depth += 1;
                    } else if ch == close {
                        depth -= 1;
                        if depth == 0 {
                            return Some((start, Pos::new(l, s.start)));
                        }
                    }
                }
            }
        } else {
            let lo = start.line.saturating_sub(limit);
            let mut l = start.line + 1;
            while l > lo {
                l -= 1;
                self.spans(l, &mut spans);
                for s in spans.iter().rev() {
                    if s.tok != Tok::Bracket || (l == start.line && s.start > col) {
                        continue;
                    }
                    let ch = self.lines[l][s.start];
                    if ch == close {
                        depth += 1;
                    } else if ch == open {
                        depth -= 1;
                        if depth == 0 {
                            return Some((start, Pos::new(l, s.start)));
                        }
                    }
                }
            }
        }
        None
    }

    /// The complete R statement around `line` (inclusive line range).
    pub fn statement_range(&mut self, line: usize) -> (usize, usize) {
        let n = self.lines.len();
        let line = line.min(n.saturating_sub(1));
        let mut s = line;
        let mut attempts = 0;
        loop {
            attempts += 1;
            // extend upward while the previous non-blank line continues into this one
            loop {
                let mut p = s;
                let mut found = None;
                while p > 0 {
                    p -= 1;
                    let inf = self.info(p);
                    if !inf.blank {
                        found = Some((p, inf));
                        break;
                    }
                    if s - p > 50 {
                        break;
                    }
                }
                match found {
                    Some((p, inf)) if inf.cont || inf.end != LexState::Normal => s = p,
                    _ => break,
                }
            }
            // inside the arguments of a multi-line call: start at the call
            if let Some((p, c)) = self.enclosing_open(s) {
                if c != '{' && p < s {
                    s = p;
                    continue;
                }
            }
            // scan down from s
            let mut depth = 0i32;
            let mut ok = true;
            let mut e = s;
            let mut i = s;
            while i < n && i < s + 5000 {
                let inf = self.info(i);
                if depth + inf.min < 0 {
                    ok = false;
                    break;
                }
                depth += inf.delta;
                e = i;
                if i >= line && depth <= 0 && !inf.cont && inf.end == LexState::Normal {
                    // `if (x)` / `function(x)` followed by `{` or `else` on the next line
                    let mut j = i + 1;
                    while j < n && self.info(j).blank && j < i + 20 {
                        j += 1;
                    }
                    let next_first = if j < n { self.info(j).first } else { None };
                    let next_line: String = self.lines.get(j).map(|l| l.iter().collect()).unwrap_or_default();
                    let continues = (next_first == Some('{') && inf.last == Some(')'))
                        || (depth == 0 && s != i && next_line.trim_start().starts_with("else") && inf.last == Some('}'));
                    if !continues {
                        break;
                    }
                }
                i += 1;
            }
            if ok {
                return (s, e.max(line));
            }
            if s == 0 || attempts > 2000 {
                return (line, line);
            }
            s -= 1;
        }
    }

    /// The nearest unclosed bracket before line `line` (line index and char).
    fn enclosing_open(&mut self, line: usize) -> Option<(usize, char)> {
        let mut stack: Vec<char> = Vec::new();
        let mut spans = Vec::new();
        let mut l = line;
        while l > 0 && line - l < 2000 {
            l -= 1;
            self.spans(l, &mut spans);
            for sp in spans.iter().rev() {
                if sp.tok != Tok::Bracket {
                    continue;
                }
                let c = self.lines[l][sp.start];
                match c {
                    ')' | ']' | '}' => stack.push(c),
                    _ => {
                        let close = match c {
                            '(' => ')',
                            '[' => ']',
                            _ => '}',
                        };
                        if stack.last() == Some(&close) {
                            stack.pop();
                        } else if stack.is_empty() {
                            return Some((l, c));
                        } else {
                            // unbalanced code; give up gracefully
                            return None;
                        }
                    }
                }
            }
        }
        None
    }

    // ---------- outline ----------

    pub fn outline(&mut self) -> Vec<OutlineItem> {
        if let Some((v, items)) = &self.outline_cache {
            if *v == self.version {
                return items.clone();
            }
        }
        let mut items = Vec::new();
        for (i, line) in self.lines.iter().enumerate() {
            let s: String = line.iter().collect();
            let t = s.trim();
            if t.starts_with('#') && !t.starts_with("#'") && crate::highlight::is_section_comment(line) {
                let level = t.chars().take_while(|c| *c == '#').count();
                let title = t
                    .trim_start_matches('#')
                    .trim_end_matches(|c| c == '-' || c == '=' || c == '#' || c == ' ')
                    .trim()
                    .to_string();
                if !title.is_empty() {
                    items.push(OutlineItem { line: i, level: level.max(1), title, is_fn: false });
                }
                continue;
            }
            if let Some(name) = function_def_name(t) {
                let indent = s.len() - s.trim_start().len();
                items.push(OutlineItem { line: i, level: 1 + indent / 2, title: name, is_fn: true });
            }
        }
        self.outline_cache = Some((self.version, items.clone()));
        items
    }
}

fn function_def_name(t: &str) -> Option<String> {
    let ops = ["<<-", "<-", "="];
    for op in ops {
        if let Some(idx) = t.find(op) {
            let name = t[..idx].trim();
            let rest = t[idx + op.len()..].trim_start();
            let is_fn = rest.starts_with("function") || rest.starts_with("\\(");
            let valid = !name.is_empty()
                && name.chars().all(|c| c.is_alphanumeric() || c == '.' || c == '_' || c == '`' || c == '$');
            if is_fn && valid {
                return Some(name.trim_matches('`').to_string());
            }
        }
    }
    None
}

pub fn end_of(p: Pos, text: &str) -> Pos {
    let nl = text.matches('\n').count();
    if nl == 0 {
        Pos::new(p.line, p.col + text.chars().count())
    } else {
        let last = text.rsplit('\n').next().unwrap_or("");
        Pos::new(p.line + nl, last.chars().count())
    }
}

fn leading_ws(line: &[char]) -> String {
    line.iter().take_while(|c| c.is_whitespace()).collect()
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.'
}

fn pair_close(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' => Some('"'),
        '\'' => Some('\''),
        '`' => Some('`'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(s: &str) -> Doc {
        Doc::from_text(s, None)
    }

    #[test]
    fn statement_simple() {
        let mut d = doc("x <- 1\ny <- 2\n");
        assert_eq!(d.statement_range(0), (0, 0));
        assert_eq!(d.statement_range(1), (1, 1));
    }

    #[test]
    fn statement_function() {
        let mut d = doc("f <- function(x) {\n  y <- x + 1\n  y\n}\nz <- 3");
        assert_eq!(d.statement_range(0), (0, 3));
        assert_eq!(d.statement_range(1), (1, 1));
        assert_eq!(d.statement_range(3), (0, 3));
        assert_eq!(d.statement_range(4), (4, 4));
    }

    #[test]
    fn statement_pipe() {
        let mut d = doc("mtcars |>\n  subset(cyl == 4) |>\n  head()\nx");
        assert_eq!(d.statement_range(0), (0, 2));
        assert_eq!(d.statement_range(1), (0, 2));
        assert_eq!(d.statement_range(2), (0, 2));
        assert_eq!(d.statement_range(3), (3, 3));
    }

    #[test]
    fn statement_call_args() {
        let mut d = doc("plot(\n  1:10,\n  main = \"a\"\n)\n");
        assert_eq!(d.statement_range(0), (0, 3));
        assert_eq!(d.statement_range(2), (0, 3));
    }

    #[test]
    fn statement_inside_function_call_args() {
        let mut d = doc("f <- function(x) {\n  y <- c(1,\n    2)\n  y\n}\n");
        assert_eq!(d.statement_range(2), (1, 2));
        assert_eq!(d.statement_range(3), (3, 3));
    }

    #[test]
    fn statement_if_else() {
        let mut d = doc("if (a) {\n  1\n} else {\n  2\n}\n");
        assert_eq!(d.statement_range(0), (0, 4));
    }

    #[test]
    fn undo_redo() {
        let mut d = doc("abc");
        d.cur = Pos::new(0, 3);
        d.type_char('d', false);
        d.type_char('e', false);
        assert_eq!(d.text(), "abcde");
        d.undo();
        assert_eq!(d.text(), "abc");
        assert!(!d.modified);
        d.redo();
        assert_eq!(d.text(), "abcde");
        d.newline();
        assert_eq!(d.lines.len(), 2);
        d.undo();
        assert_eq!(d.text(), "abcde");
    }

    #[test]
    fn comment_toggle() {
        let mut d = doc("a\n  b");
        d.select_all();
        d.toggle_comment();
        assert_eq!(d.text(), "# a\n#   b");
        d.select_all();
        d.toggle_comment();
        assert_eq!(d.text(), "a\n  b");
    }

    #[test]
    fn newline_indent_braces() {
        let mut d = doc("f <- function() {}");
        d.cur = Pos::new(0, 17);
        d.newline();
        assert_eq!(d.text(), "f <- function() {\n  \n}");
        assert_eq!(d.cur, Pos::new(1, 2));
    }

    #[test]
    fn outline_items() {
        let mut d = doc("# Load ----\nf <- function(x) x\n## Sub ====\n");
        let o = d.outline();
        assert_eq!(o.len(), 3);
        assert_eq!(o[0].title, "Load");
        assert_eq!(o[1].title, "f");
        assert_eq!(o[2].level, 2);
    }

    #[test]
    fn move_lines_roundtrip() {
        let mut d = doc("a\nb\nc");
        d.cur = Pos::new(1, 0);
        d.move_lines(true);
        assert_eq!(d.text(), "b\na\nc");
        d.undo();
        assert_eq!(d.text(), "a\nb\nc");
    }

    #[test]
    fn replace() {
        let mut d = doc("aa x aa");
        assert_eq!(d.replace_all("aa", "b"), 2);
        assert_eq!(d.text(), "b x b");
    }
}
