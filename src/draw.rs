//! Rendering of all panes, menus and dialogs.

use crate::app::{App, EnvRow, Modal, Pane, PromptKind, RightTab, MENUS};
use crate::editor::LintKind;
use crate::highlight::{self, Tok};
use crate::plot::{ImageProtocol, PlotMode};
use crate::ui::{fill, human_size, pad, pad_left, put_char, put_str, str_width, truncate, Theme};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::Frame;

impl App {
    pub fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        self.image_req = None;
        self.help_cursor = None;
        let mut cursor: Option<(u16, u16)> = None;
        {
            let buf = f.buffer_mut();
            if area.width < 40 || area.height < 8 {
                put_str(buf, 0, 0, area.width, "terminal too small", Theme::error());
                return;
            }
            self.layout(area);
            self.draw_menubar(buf);
            let r = self.rects.clone();
            let zoom = self.zoom;
            let show = |p: Pane| zoom.is_none() || zoom == Some(p);
            if show(Pane::Source) {
                if let Some(c) = self.draw_source(buf, r.source) {
                    cursor = Some(c);
                }
            }
            if show(Pane::Console) {
                if let Some(c) = self.draw_console(buf, r.console) {
                    if self.focus == Pane::Console {
                        cursor = Some(c);
                    }
                }
            }
            if show(Pane::Env) {
                self.draw_env(buf, r.env);
            }
            if show(Pane::Right) {
                self.draw_right(buf, r.right);
                if self.focus == Pane::Right {
                    if let Some(c) = self.help_cursor {
                        cursor = Some(c);
                    }
                }
            }
            if r.vsep.width > 0 {
                for y in r.vsep.y..r.vsep.bottom() {
                    put_char(buf, r.vsep.x, y, r.vsep.right(), '│', Theme::separator());
                }
            }
            self.draw_status(buf, r.status);
            if let Some(c) = self.draw_modal(buf, area) {
                cursor = Some(c);
            } else if !matches!(self.modal, Modal::None) {
                cursor = None;
            }
        }
        if let Some((x, y)) = cursor {
            f.set_cursor_position(Position::new(x, y));
        }
    }

    fn layout(&mut self, area: Rect) {
        let r = &mut self.rects;
        r.menubar = Rect::new(area.x, area.y, area.width, 1);
        r.status = Rect::new(area.x, area.bottom() - 1, area.width, 1);
        let body = Rect::new(area.x, area.y + 1, area.width, area.height.saturating_sub(2));
        r.body = body;
        r.source = Rect::default();
        r.console = Rect::default();
        r.env = Rect::default();
        r.right = Rect::default();
        r.vsep = Rect::default();
        if let Some(z) = self.zoom {
            match z {
                Pane::Source => r.source = body,
                Pane::Console => r.console = body,
                Pane::Env => r.env = body,
                Pane::Right => r.right = body,
            }
            return;
        }
        let lw = ((body.width as f32 * self.vsplit) as u16).min(body.width.saturating_sub(11)).max(10.min(body.width));
        let left = Rect::new(body.x, body.y, lw, body.height);
        r.vsep = Rect::new(body.x + lw, body.y, 1, body.height);
        let right = Rect::new(body.x + lw + 1, body.y, body.width.saturating_sub(lw + 1), body.height);
        let split = |rect: Rect, frac: f32| -> (Rect, Rect) {
            let h = ((rect.height as f32 * frac) as u16).min(rect.height.saturating_sub(2)).max(2.min(rect.height));
            (Rect::new(rect.x, rect.y, rect.width, h), Rect::new(rect.x, rect.y + h, rect.width, rect.height - h))
        };
        let (s, c) = split(left, self.lsplit);
        let (e, rr) = split(right, self.rsplit);
        r.source = s;
        r.console = c;
        r.env = e;
        r.right = rr;
    }

    fn title_style(&self, p: Pane) -> Style {
        if self.focus == p {
            Theme::title_focused()
        } else {
            Theme::title()
        }
    }

    fn draw_menubar(&mut self, buf: &mut Buffer) {
        let r = self.rects.menubar;
        fill(buf, r, Theme::menubar());
        let mut x = put_str(buf, r.x, r.y, r.right(), " tRm ", Theme::menubar().add_modifier(Modifier::BOLD));
        self.rects.menu_titles.clear();
        let open = if let Modal::Menu { menu, .. } = self.modal { Some(menu) } else { None };
        for (i, (name, _)) in MENUS.iter().enumerate() {
            let label = format!(" {name} ");
            let st = if open == Some(i) { Theme::menu_sel() } else { Theme::menubar() };
            let x0 = x;
            x = put_str(buf, x, r.y, r.right(), &label, st);
            self.rects.menu_titles.push((Rect::new(x0, r.y, x - x0, 1), i));
        }
        // R state on the right
        let busy = self.r_busy();
        let label = if !self.r_running() {
            " R stopped ".to_string()
        } else if busy {
            " R busy… ".to_string()
        } else {
            " R ready ".to_string()
        };
        let st = if busy { Theme::menu_sel() } else { Theme::menubar() };
        let w = str_width(&label) as u16;
        if r.width > x - r.x + w + 2 {
            put_str(buf, r.right() - w, r.y, r.right(), &label, st);
        }
    }

    fn draw_status(&mut self, buf: &mut Buffer, r: Rect) {
        fill(buf, r, Theme::statusbar());
        let d = self.doc();
        let right = format!(
            " Ln {}, Col {} │ {} │ F1 help ",
            d.cur.line + 1,
            d.cur.col + 1,
            if self.plots.mode == PlotMode::Image { "img" } else { self.plots.mode.name() }
        );
        let rw = str_width(&right) as u16;
        let left = if let Some((m, _, err)) = &self.status {
            Some((m.clone(), *err))
        } else {
            // lint message at the cursor line
            d.lints.iter().find(|l| l.line == d.cur.line).map(|l| {
                let k = match l.kind {
                    LintKind::Error => "error",
                    LintKind::Warning => "warning",
                    LintKind::Style => "style",
                };
                (format!("{k}: {}", l.msg), l.kind == LintKind::Error)
            })
        };
        let maxl = r.width.saturating_sub(rw + 1);
        if let Some((m, err)) = left {
            let st = if err { Theme::statusbar().fg(ratatui::style::Color::LightYellow).add_modifier(Modifier::BOLD) } else { Theme::statusbar() };
            put_str(buf, r.x + 1, r.y, r.x + maxl, &truncate(&m, maxl.saturating_sub(1) as usize), st);
        } else {
            let hint = match self.focus {
                Pane::Source => "Ctrl+Enter run · Ctrl+Shift+M |> · Alt+- <- · F10 menu",
                Pane::Console => "Shift+PgUp scroll · Ctrl+C interrupt · Ctrl+1 back to source",
                Pane::Env => "Enter expand · v View() · Del rm() · s sort",
                Pane::Right => match self.right_tab {
                    RightTab::Files => "Enter open · Bksp up · w setwd · g R wd · . hidden · n new",
                    RightTab::Plots => "←/→ history · m mode · i image · r redraw · s save",
                    RightTab::Viewer => "arrows/PgUp/PgDn scroll",
                    RightTab::Help => "type to search · Enter open · Esc clear · Bksp back · Alt+←/→ history",
                },
            };
            put_str(buf, r.x + 1, r.y, r.x + maxl, &truncate(hint, maxl.saturating_sub(1) as usize), Theme::statusbar());
        }
        if r.width > rw {
            put_str(buf, r.right() - rw, r.y, r.right(), &right, Theme::statusbar());
        }
    }

    // ---------- source ----------

    fn draw_source(&mut self, buf: &mut Buffer, area: Rect) -> Option<(u16, u16)> {
        if area.height < 2 {
            return None;
        }
        let title = Rect::new(area.x, area.y, area.width, 1);
        fill(buf, title, self.title_style(Pane::Source));
        self.rects.source_tabs.clear();
        let mut x = title.x;
        let n = self.docs.len();
        // make sure the active tab is visible: start from a tab that fits
        let widths: Vec<u16> = self.docs.iter().map(|d| str_width(&d.title()) as u16 + 4).collect();
        let mut first = 0;
        while first < self.active {
            let w: u16 = widths[first..=self.active].iter().sum();
            if w <= title.width.saturating_sub(2) {
                break;
            }
            first += 1;
        }
        if first > 0 {
            x = put_str(buf, x, title.y, title.right(), "‹", self.title_style(Pane::Source));
        }
        for i in first..n {
            let d = &self.docs[i];
            let label = format!(" {}{} ", d.title(), if d.modified { "*" } else { " " });
            let st = if i == self.active { Theme::tab_active() } else { self.title_style(Pane::Source) };
            let x0 = x;
            x = put_str(buf, x, title.y, title.right(), &label, st);
            self.rects.source_tabs.push((Rect::new(x0, title.y, x - x0, 1), i));
            x = put_str(buf, x, title.y, title.right(), "│", self.title_style(Pane::Source));
            if x >= title.right() {
                break;
            }
        }

        let body = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
        let outline_w = if self.show_outline && body.width >= 40 { (body.width / 3).min(30) } else { 0 };
        let text = Rect::new(body.x, body.y, body.width - outline_w, body.height);
        self.rects.source_text = text;
        self.rects.outline = if outline_w > 0 { Rect::new(text.right(), body.y, outline_w, body.height) } else { Rect::default() };

        let focused = self.focus == Pane::Source && matches!(self.modal, Modal::None | Modal::Prompt(_));
        let i = self.active.min(self.docs.len() - 1);
        let nlines = self.docs[i].lines.len();
        let digits = nlines.to_string().len().max(2) as u16;
        let gw = digits + 2;
        self.rects.gutter_w = gw;
        let tw = text.width.saturating_sub(gw) as usize;
        let th = text.height as usize;
        {
            let d = &mut self.docs[i];
            // keep scroll sane after resizes
            if d.top >= nlines {
                d.top = nlines.saturating_sub(1);
            }
            if focused {
                // nothing: cursor scrolling handled by key handlers
            }
            let _ = (tw, th);
        }
        let matching = if focused { self.docs[i].matching_bracket() } else { None };
        let d = &mut self.docs[i];
        let sel = d.selection();
        let cur_line = d.cur.line;
        let mut spans = Vec::new();
        for row in 0..th {
            let l = d.top + row;
            let y = text.y + row as u16;
            if l >= d.lines.len() {
                break;
            }
            // gutter
            let lint = d.lints.iter().filter(|x| x.line == l).map(|x| x.kind).min_by_key(|k| match k {
                LintKind::Error => 0,
                LintKind::Warning => 1,
                LintKind::Style => 2,
            });
            let (mark, mst) = match lint {
                Some(LintKind::Error) => ("●", Theme::error()),
                Some(LintKind::Warning) => ("▲", Theme::warning()),
                Some(LintKind::Style) => ("·", Theme::info()),
                None => (" ", Theme::gutter()),
            };
            put_str(buf, text.x, y, text.x + 1, mark, mst);
            let ln = pad_left(&(l + 1).to_string(), digits as usize);
            put_str(buf, text.x + 1, y, text.x + 1 + digits, &ln, if l == cur_line { Theme::gutter_cur() } else { Theme::gutter() });
            // text
            d.spans(l, &mut spans);
            let line = &d.lines[l];
            let x0 = text.x + gw;
            let mut col = 0usize; // display column
            let mut si = 0;
            for (ci, ch) in line.iter().enumerate() {
                while si < spans.len() && spans[si].end <= ci {
                    si += 1;
                }
                let tok = spans.get(si).filter(|s| s.start <= ci && ci < s.end).map(|s| s.tok).unwrap_or(Tok::Ident);
                let mut st = highlight::style(tok);
                let p = crate::editor::Pos::new(l, ci);
                if let Some((a, b)) = sel {
                    if p >= a && p < b {
                        st = Theme::selection();
                    }
                }
                if let Some((m1, m2)) = matching {
                    if p == m1 || p == m2 {
                        st = st.patch(Theme::matching_bracket());
                    }
                }
                let w = if *ch == '\t' { crate::ui::TAB_WIDTH - (col % crate::ui::TAB_WIDTH) } else { crate::ui::char_width(*ch) };
                if col + w > d.left && col >= d.left {
                    let sx = x0 + (col - d.left) as u16;
                    if sx >= text.right() {
                        break;
                    }
                    if *ch == '\t' {
                        for k in 0..w {
                            put_char(buf, sx + k as u16, y, text.right(), ' ', st);
                        }
                    } else {
                        put_char(buf, sx, y, text.right(), *ch, st);
                    }
                }
                col += w;
            }
            // show selected line ends
            if let Some((a, b)) = sel {
                if l >= a.line && l < b.line && col >= d.left {
                    let sx = x0 + (col - d.left) as u16;
                    put_char(buf, sx, y, text.right(), ' ', Theme::selection());
                }
            }
        }

        if outline_w > 0 {
            self.draw_outline(buf, self.rects.outline);
        }

        if focused {
            let d = &self.docs[i];
            let dc = d.disp_col(d.cur.line, d.cur.col);
            if d.cur.line >= d.top && d.cur.line < d.top + th && dc >= d.left {
                let x = text.x + gw + (dc - d.left) as u16;
                let y = text.y + (d.cur.line - d.top) as u16;
                if x < text.right() {
                    return Some((x, y));
                }
            }
        }
        None
    }

    fn draw_outline(&mut self, buf: &mut Buffer, r: Rect) {
        for y in r.y..r.bottom() {
            put_char(buf, r.x, y, r.right(), '│', Theme::separator());
        }
        let inner = Rect::new(r.x + 1, r.y, r.width.saturating_sub(1), r.height);
        let items = self.doc_mut().outline();
        if items.is_empty() {
            put_str(buf, inner.x + 1, inner.y, inner.right(), "No sections", Theme::dim());
            put_str(buf, inner.x + 1, inner.y + 1, inner.right(), "# Name ----", Theme::dim());
            return;
        }
        let off = self.outline_offset(items.len(), inner.height as usize);
        let cur = self.doc().cur.line;
        let current = items.iter().rposition(|it| it.line <= cur);
        for (row, (idx, it)) in items.iter().enumerate().skip(off).take(inner.height as usize).enumerate() {
            let y = inner.y + row as u16;
            let indent = "  ".repeat(it.level.saturating_sub(1).min(4));
            let icon = if it.is_fn { "ƒ " } else { "§ " };
            let label = format!("{indent}{icon}{}", it.title);
            let mut st = if it.is_fn { Style::default().fg(ratatui::style::Color::LightBlue) } else { Style::default().add_modifier(Modifier::BOLD) };
            if Some(idx) == current {
                st = Theme::list_sel();
            }
            put_str(buf, inner.x, y, inner.right(), &pad(&label, inner.width as usize), st);
        }
    }

    // ---------- console ----------

    fn draw_console(&mut self, buf: &mut Buffer, area: Rect) -> Option<(u16, u16)> {
        if area.height < 2 {
            return None;
        }
        let title = Rect::new(area.x, area.y, area.width, 1);
        let ts = self.title_style(Pane::Console);
        fill(buf, title, ts);
        let wd = self.r_wd.as_ref().map(|p| {
            let s = p.to_string_lossy().to_string();
            let h = crate::files::home().to_string_lossy().to_string();
            if s.starts_with(&h) { format!("~{}", &s[h.len()..]) } else { s }
        }).unwrap_or_default();
        let label = format!(" Console  {wd}");
        put_str(buf, title.x, title.y, title.right(), &truncate(&label, title.width as usize), ts);
        let body = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
        self.rects.console_body = body;
        if self.console.is_none() {
            if self.console_error.is_none() {
                self.start_r(body.height, body.width);
            }
            if let Some(e) = &self.console_error {
                put_str(buf, body.x + 1, body.y, body.right(), &truncate(e, body.width as usize - 1), Theme::error());
                put_str(buf, body.x + 1, body.y + 1, body.right(), "Set r_command in ~/.config/trm/config, then Session > Restart R", Theme::dim());
                return None;
            }
        }
        let c = self.console.as_mut()?;
        c.render(buf, body)
    }

    // ---------- environment ----------

    fn draw_env(&mut self, buf: &mut Buffer, area: Rect) {
        if area.height < 2 {
            return;
        }
        let title = Rect::new(area.x, area.y, area.width, 1);
        let ts = self.title_style(Pane::Env);
        fill(buf, title, ts);
        let total: f64 = self.env.snap.objects.iter().map(|o| o.size.max(0.0)).sum();
        let mut label = format!(" Environment  {} obj · {}", self.env.snap.total, human_size(total));
        if self.env.sort_size {
            label.push_str(" · by size");
        }
        put_str(buf, title.x, title.y, title.right(), &truncate(&label, title.width as usize), ts);
        let body = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
        let rows = self.env.rows();
        // split: list on top, preview below
        let preview_h = if body.height >= 12 { body.height * 2 / 5 } else { 0 };
        let list_area = Rect::new(body.x, body.y, body.width, body.height - preview_h);
        let w = list_area.width as usize;
        let size_w = 9usize;
        let name_w = (w * 26 / 100).clamp(6, 24);
        let type_w = (w * 22 / 100).clamp(8, 18);
        let val_w = w.saturating_sub(name_w + type_w + size_w + 4);
        if list_area.height > 0 {
            let hdr = format!(" {} {} {} {}", pad("Name", name_w + 1), pad("Type", type_w), pad_left("Size", size_w), pad("Value", val_w));
            put_str(buf, list_area.x, list_area.y, list_area.right(), &hdr, Theme::header());
        }
        let list = Rect::new(list_area.x, list_area.y + 1, list_area.width, list_area.height.saturating_sub(1));
        self.rects.env_list = list;
        if rows.is_empty() {
            let msg = if self.r_running() { "Environment is empty" } else { "R is not running" };
            put_str(buf, list.x + 1, list.y, list.right(), msg, Theme::dim());
        }
        let h = list.height as usize;
        if self.env.sel < self.env.scroll {
            self.env.scroll = self.env.sel;
        } else if h > 0 && self.env.sel >= self.env.scroll + h {
            self.env.scroll = self.env.sel + 1 - h;
        }
        for (row, r) in rows.iter().enumerate().skip(self.env.scroll).take(h) {
            let y = list.y + (row - self.env.scroll) as u16;
            let (line, base) = match r {
                EnvRow::Obj(i) => {
                    let o = &self.env.snap.objects[*i];
                    let arrow = if o.children.is_empty() { " " } else if self.env.expanded.contains(&o.name) { "▾" } else { "▸" };
                    let ty = if o.dims.is_empty() || o.class == "data.frame" || o.class == "tibble" { o.class.clone() } else { format!("{} {}", o.class, o.dims) };
                    let val = if o.class == "data.frame" || o.class == "tibble" || o.class == "data.table" { o.dims.clone() } else { o.summary.clone() };
                    (
                        format!("{arrow}{} {} {} {}", pad(&o.name, name_w + 1), pad(&ty, type_w), pad_left(&human_size(o.size), size_w), pad(&val, val_w)),
                        Style::default(),
                    )
                }
                EnvRow::Child(i, c) => {
                    let ch = &self.env.snap.objects[*i].children[*c];
                    let ty = if ch.dims.is_empty() { ch.class.clone() } else { format!("{} {}", ch.class, ch.dims) };
                    (
                        format!("  {} {} {} {}", pad(&format!("$ {}", ch.name), name_w), pad(&ty, type_w), pad_left(&human_size(ch.size), size_w), pad(&ch.summary, val_w)),
                        Theme::dim().fg(ratatui::style::Color::Reset),
                    )
                }
            };
            let st = if row == self.env.sel && self.focus == Pane::Env {
                Theme::list_sel()
            } else if row == self.env.sel {
                Style::default().add_modifier(Modifier::UNDERLINED)
            } else {
                base
            };
            put_str(buf, list.x, y, list.right(), &pad(&line, list.width as usize), st);
        }
        // preview
        if preview_h > 0 {
            let pr = Rect::new(body.x, list_area.bottom(), body.width, preview_h);
            self.rects.env_preview = pr;
            let obj = self.env.selected_obj().and_then(|i| self.env.snap.objects.get(i));
            let head = match obj {
                Some(o) => format!("── {} ", o.name),
                None => "── preview ".to_string(),
            };
            let line = format!("{head}{}", "─".repeat((pr.width as usize).saturating_sub(str_width(&head))));
            put_str(buf, pr.x, pr.y, pr.right(), &line, Theme::separator());
            if let Some(o) = obj {
                let max = o.preview.len().saturating_sub(1);
                if self.env.preview_scroll > max {
                    self.env.preview_scroll = max;
                }
                for (k, l) in o.preview.iter().skip(self.env.preview_scroll).take(pr.height.saturating_sub(1) as usize).enumerate() {
                    put_str(buf, pr.x + 1, pr.y + 1 + k as u16, pr.right(), l, Style::default());
                }
            }
        } else {
            self.rects.env_preview = Rect::default();
        }
    }

    // ---------- right pane: files / plots / viewer ----------

    fn draw_right(&mut self, buf: &mut Buffer, area: Rect) {
        if area.height < 2 {
            return;
        }
        let title = Rect::new(area.x, area.y, area.width, 1);
        let ts = self.title_style(Pane::Right);
        fill(buf, title, ts);
        self.rects.right_tabs.clear();
        let mut x = title.x;
        for (t, name) in [(RightTab::Files, "Files"), (RightTab::Plots, "Plots"), (RightTab::Viewer, "Viewer"), (RightTab::Help, "Help")] {
            let st = if t == self.right_tab { Theme::tab_active() } else { ts };
            let x0 = x;
            x = put_str(buf, x, title.y, title.right(), &format!(" {name} "), st);
            self.rects.right_tabs.push((Rect::new(x0, title.y, x - x0, 1), t));
        }
        let body = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
        self.rects.right_body = body;
        // let R format help text to the pane width
        self.session.write_view_width_if_changed(body.width.saturating_sub(2));
        self.session.write_text_size_if_changed(body.width, body.height);
        match self.right_tab {
            RightTab::Files => self.draw_files(buf, body),
            RightTab::Plots => {
                let info = if self.plots.history.is_empty() {
                    String::new()
                } else {
                    format!(" ◀ {}/{} ▶  {} ", self.plots.idx + 1, self.plots.history.len(), self.plots.mode.name())
                };
                let w = str_width(&info) as u16;
                if x + w < title.right() {
                    put_str(buf, title.right() - w, title.y, title.right(), &info, ts);
                }
                self.draw_plots(buf, body);
            }
            RightTab::Viewer => self.draw_viewer(buf, body, title),
            RightTab::Help => self.draw_help(buf, body, title, x),
        }
    }

    fn draw_files(&mut self, buf: &mut Buffer, body: Rect) {
        if body.height == 0 {
            return;
        }
        let path = self.files.dir.to_string_lossy().to_string();
        put_str(buf, body.x + 1, body.y, body.right(), &truncate(&path, body.width.saturating_sub(1) as usize), Theme::header());
        let list = Rect::new(body.x, body.y + 1, body.width, body.height - 1);
        if let Some(e) = &self.files.error {
            put_str(buf, list.x + 1, list.y, list.right(), e, Theme::error());
        }
        let h = list.height as usize;
        let n = self.files.len();
        if self.files.sel < self.files.scroll {
            self.files.scroll = self.files.sel;
        } else if h > 0 && self.files.sel >= self.files.scroll + h {
            self.files.scroll = self.files.sel + 1 - h;
        }
        let size_w = 9usize;
        let name_w = (list.width as usize).saturating_sub(size_w + 3);
        for row in self.files.scroll..(self.files.scroll + h).min(n) {
            let y = list.y + (row - self.files.scroll) as u16;
            let (label, st) = if row == 0 {
                (format!(" ↑ {}", pad("..", name_w)), Theme::dim())
            } else {
                let e = &self.files.entries[row - 1];
                if e.is_dir {
                    (format!(" ▸ {}", pad(&format!("{}/", e.name), name_w)), Style::default().fg(ratatui::style::Color::LightBlue).add_modifier(Modifier::BOLD))
                } else {
                    let st = if e.name.to_lowercase().ends_with(".r") { Style::default().fg(ratatui::style::Color::Cyan) } else { Style::default() };
                    (format!("   {}{}", pad(&e.name, name_w), pad_left(&human_size(e.size as f64), size_w)), st)
                }
            };
            let st = if row == self.files.sel && self.focus == Pane::Right { Theme::list_sel() } else { st };
            put_str(buf, list.x, y, list.right(), &pad(&label, list.width as usize), st);
        }
    }

    fn draw_plots(&mut self, buf: &mut Buffer, body: Rect) {
        if body.width < 4 || body.height < 2 {
            return;
        }
        self.update_plot_size(body);
        if self.plots.history.is_empty() {
            put_str(buf, body.x + 1, body.y, body.right(), "No plots yet. Try: plot(cars)", Theme::dim());
            let pm = format!("Display: {}  (m cycles ascii/braille/blocks/image)", self.plots.mode.name());
            put_str(buf, body.x + 1, body.y + 1, body.right(), &pm, Theme::dim());
            return;
        }
        let mode = self.plots.mode;
        if mode == PlotMode::Image && self.image_protocol != ImageProtocol::None {
            if let Some(p) = self.plots.current().cloned() {
                if self.plots.image().is_some() {
                    self.image_req = Some((p, body));
                } else if let Some(e) = &self.plots.error {
                    put_str(buf, body.x + 1, body.y, body.right(), e, Theme::error());
                }
            }
            return;
        }
        let mode = if mode == PlotMode::Image { PlotMode::Blocks } else { mode };
        let depth = self.cfg.color_depth;
        let aspect = self.cell_px.1 as f64 / self.cell_px.0.max(1) as f64;
        let r = self.plots.rendered(body.width as usize, body.height as usize, mode, depth, aspect);
        match r {
            Some(r) => crate::plot::draw_rendered(buf, body, r),
            None => {
                let e = self.plots.error.clone().unwrap_or_else(|| "cannot load plot".into());
                put_str(buf, body.x + 1, body.y, body.right(), &e, Theme::error());
            }
        }
    }

    fn draw_viewer(&mut self, buf: &mut Buffer, body: Rect, title: Rect) {
        let Some(v) = &self.viewer.data else {
            put_str(buf, body.x + 1, body.y, body.right(), "Use View(df) or press v in the Environment pane", Theme::dim());
            return;
        };
        let info = format!(" {} · {} x {} ", v.title, v.nrow, v.ncol);
        let w = str_width(&info) as u16;
        if title.width > w + 25 {
            put_str(buf, title.right() - w, title.y, title.right(), &info, self.title_style(Pane::Right));
        }
        if body.height < 3 {
            return;
        }
        let ncols = v.headers.len();
        // column widths from header + visible cells
        let first_row = self.viewer.row;
        let vis_rows = (body.height as usize).saturating_sub(2);
        let mut widths: Vec<usize> = (0..ncols)
            .map(|c| {
                let mut w = str_width(v.headers.get(c).map(|s| s.as_str()).unwrap_or(""))
                    .max(str_width(v.types.get(c).map(|s| s.as_str()).unwrap_or("")).min(10));
                for r in v.rows.iter().skip(first_row).take(vis_rows) {
                    w = w.max(str_width(r.get(c).map(|s| s.as_str()).unwrap_or("")));
                }
                w.clamp(2, 24)
            })
            .collect();
        if let Some(w0) = widths.get_mut(0) {
            *w0 = (*w0).max(3);
        }
        let mut cols_to_draw = vec![0usize];
        cols_to_draw.extend((1 + self.viewer.col)..ncols);
        let draw_row = |buf: &mut Buffer, y: u16, cells: &dyn Fn(usize) -> String, st0: Style, st: Style| {
            let mut x = body.x;
            for (k, c) in cols_to_draw.iter().enumerate() {
                let w = widths[*c];
                let txt = cells(*c);
                let s = if k == 0 { pad_left(&txt, w) } else { pad(&txt, w) };
                x = put_str(buf, x, y, body.right(), &s, if k == 0 { st0 } else { st });
                x = put_str(buf, x, y, body.right(), " │ ", Theme::dim());
                if x >= body.right() {
                    break;
                }
            }
        };
        draw_row(buf, body.y, &|c| v.headers.get(c).cloned().unwrap_or_default(), Theme::header(), Theme::header());
        draw_row(buf, body.y + 1, &|c| v.types.get(c).cloned().unwrap_or_default(), Theme::dim(), Theme::dim());
        for (k, r) in v.rows.iter().skip(first_row).take(vis_rows).enumerate() {
            draw_row(buf, body.y + 2 + k as u16, &|c| r.get(c).cloned().unwrap_or_default(), Theme::dim(), Style::default());
        }
        let _ = &mut widths;
    }

    fn draw_help(&mut self, buf: &mut Buffer, body: Rect, title: Rect, tabs_end: u16) {
        if body.height < 2 {
            return;
        }
        // page title on the tab row
        let info = match (&self.help.loading, &self.help.page) {
            (Some(t), _) => format!(" loading {t}… "),
            (None, Some(p)) => format!(" {} ", p.title),
            _ => String::new(),
        };
        let iw = str_width(&info) as u16;
        if tabs_end + iw < title.right() {
            put_str(buf, title.right() - iw, title.y, title.right(), &info, self.title_style(Pane::Right));
        }
        // search box
        let focused = self.focus == Pane::Right && matches!(self.modal, Modal::None);
        let sb = Rect::new(body.x, body.y, body.width, 1);
        fill(buf, sb, Theme::popup());
        let x = put_str(buf, sb.x, sb.y, sb.right(), " Search: ", Theme::popup().add_modifier(Modifier::BOLD));
        let q = &self.help.query;
        let shown = if q.is_empty() && !focused { "type a topic, e.g. lm, plot, ggplot2::aes" } else { q.as_str() };
        let st = if q.is_empty() { Theme::popup().add_modifier(Modifier::DIM) } else { Theme::popup() };
        let end = put_str(buf, x, sb.y, sb.right(), &truncate(shown, sb.right().saturating_sub(x + 1) as usize), st);
        if focused {
            self.help_cursor = Some((if q.is_empty() { x } else { end }, sb.y));
        }
        let area = Rect::new(body.x, body.y + 1, body.width, body.height - 1);
        let rows = area.height as usize;

        if !q.is_empty() {
            // result list
            let n = self.help.results.len();
            if self.help.index.is_none() {
                put_str(buf, area.x + 1, area.y, area.right(), "building help index…", Theme::dim());
                return;
            }
            if n == 0 {
                put_str(buf, area.x + 1, area.y, area.right(), "no matching topics", Theme::dim());
                return;
            }
            if self.help.sel < self.help.results_scroll {
                self.help.results_scroll = self.help.sel;
            } else if rows > 0 && self.help.sel >= self.help.results_scroll + rows {
                self.help.results_scroll = self.help.sel + 1 - rows;
            }
            let pkg_w = 14usize.min(area.width as usize / 3);
            let name_w = (area.width as usize).saturating_sub(pkg_w + 3);
            let Some(idx) = &self.help.index else { return };
            for (k, ri) in self.help.results.iter().enumerate().skip(self.help.results_scroll).take(rows) {
                let y = area.y + (k - self.help.results_scroll) as u16;
                let line = format!(" {} {}", pad(idx.alias(*ri), name_w), pad(idx.pkg(*ri), pkg_w));
                let st = if k == self.help.sel { Theme::list_sel() } else { Style::default() };
                put_str(buf, area.x, y, area.right(), &pad(&line, area.width as usize), st);
            }
            return;
        }

        let Some(page) = &self.help.page else {
            let lines = [
                "Type in the search box to find help in all installed packages.",
                "",
                "?mean or help(lm) in the console also opens here - when a topic",
                "exists in several packages the first one opens directly.",
                "",
                "Enter open · Esc clear · Backspace back · Alt+Left/Right history",
                "F1 shows tRm's keyboard shortcuts.",
            ];
            for (k, l) in lines.iter().enumerate().take(rows) {
                put_str(buf, area.x + 1, area.y + k as u16, area.right(), l, Theme::dim());
            }
            return;
        };
        let max = page.lines.len().saturating_sub(rows);
        if self.help.row > max {
            self.help.row = max;
        }
        let (row0, col0) = (self.help.row, self.help.col);
        for (k, (line, heading)) in page.lines.iter().skip(row0).take(rows).enumerate() {
            let st = if *heading { Style::default().add_modifier(Modifier::BOLD).fg(ratatui::style::Color::Cyan) } else { Style::default() };
            let text: String = line.chars().skip(col0).collect();
            put_str(buf, area.x + 1, area.y + k as u16, area.right(), &text, st);
        }
        if page.lines.len() > rows && rows > 0 {
            let pct = if max == 0 { 100 } else { row0 * 100 / max };
            let label = format!(" {pct}% ");
            let lw = label.len() as u16;
            put_str(buf, area.right().saturating_sub(lw), area.bottom() - 1, area.right(), &label, Theme::menu_sel());
        }
    }

    // ---------- modals ----------

    fn draw_modal(&mut self, buf: &mut Buffer, area: Rect) -> Option<(u16, u16)> {
        match &self.modal {
            Modal::None => None,
            Modal::Menu { menu, item } => {
                let (menu, item) = (*menu, *item);
                let items = MENUS[menu].1;
                let x = self.rects.menu_titles.get(menu).map(|(r, _)| r.x).unwrap_or(0);
                let lw = items.iter().map(|i| str_width(i.label)).max().unwrap_or(10);
                let kw = items.iter().map(|i| str_width(i.key)).max().unwrap_or(0);
                let w = (lw + kw + 6) as u16;
                let h = items.len() as u16 + 2;
                let x = x.min(area.right().saturating_sub(w));
                let r = Rect::new(x, area.y + 1, w.min(area.width), h.min(area.height.saturating_sub(1)));
                self.rects.menu_popup = r;
                draw_box(buf, r, "");
                for (k, it) in items.iter().enumerate() {
                    let y = r.y + 1 + k as u16;
                    if y >= r.bottom() - 1 {
                        break;
                    }
                    let st = if k == item { Theme::menu_sel() } else { Theme::popup() };
                    let line = format!(" {} {} ", pad(it.label, lw), pad_left(it.key, kw + 1));
                    put_str(buf, r.x + 1, y, r.right() - 1, &line, st);
                }
                None
            }
            Modal::Confirm(_, text) => {
                let w = (str_width(text) as u16 + 4).min(area.width.saturating_sub(2)).max(30);
                let r = centered(area, w, 3);
                draw_box(buf, r, " Confirm ");
                put_str(buf, r.x + 2, r.y + 1, r.right() - 1, &truncate(text, w as usize - 4), Theme::popup());
                None
            }
            Modal::Prompt(p) => {
                let inline = matches!(p.kind, PromptKind::Find | PromptKind::Goto);
                let r = if inline {
                    let s = self.rects.status;
                    Rect::new(s.x, s.y, s.width, 1)
                } else {
                    let w = 70.min(area.width.saturating_sub(2));
                    centered(area, w, 3)
                };
                let (ix, iy, iw) = if inline {
                    fill(buf, r, Theme::popup());
                    let hint = if p.kind == PromptKind::Find { "  (Enter next, Up/Down, Esc)" } else { "" };
                    let lbl = format!(" {}: ", p.title);
                    let x = put_str(buf, r.x, r.y, r.right(), &lbl, Theme::popup().add_modifier(Modifier::BOLD));
                    let hw = str_width(hint) as u16;
                    if r.width > x - r.x + hw + 10 {
                        put_str(buf, r.right() - hw, r.y, r.right(), hint, Theme::popup());
                    }
                    (x, r.y, r.right().saturating_sub(x + hw + 1))
                } else {
                    draw_box(buf, r, &format!(" {} (Enter ok, Esc cancel, Tab complete) ", p.title));
                    (r.x + 2, r.y + 1, r.width.saturating_sub(4))
                };
                let input: String = p.input.iter().collect();
                // scroll the input so the cursor stays visible
                let before: String = p.input[..p.cursor.min(p.input.len())].iter().collect();
                let bw = str_width(&before) as u16;
                let skip = bw.saturating_sub(iw.saturating_sub(1));
                let mut x = ix;
                let mut acc = 0u16;
                for ch in input.chars() {
                    let cw = crate::ui::char_width(ch) as u16;
                    if acc >= skip {
                        x = put_char(buf, x, iy, ix + iw, ch, Theme::popup().add_modifier(Modifier::UNDERLINED));
                    }
                    acc += cw;
                }
                Some((ix + bw - skip, iy))
            }
            Modal::Open(d) => {
                let w = 70.min(area.width.saturating_sub(2));
                let h = (area.height * 2 / 3).max(8).min(area.height.saturating_sub(2));
                let r = centered(area, w, h);
                draw_box(buf, r, " Open file (type to filter, Enter open, Bksp up, Esc) ");
                let dir = d.dir.to_string_lossy().to_string();
                put_str(buf, r.x + 2, r.y + 1, r.right() - 1, &truncate(&dir, w as usize - 4), Theme::popup().add_modifier(Modifier::BOLD));
                let fl = format!("filter: {}", d.filter);
                let cx = put_str(buf, r.x + 2, r.y + 2, r.right() - 1, &fl, Theme::popup());
                let list = Rect::new(r.x + 1, r.y + 3, r.width.saturating_sub(2), r.height.saturating_sub(4));
                self.rects.open_list = list;
                let vis = d.visible();
                let lh = list.height as usize;
                let mut scroll = d.scroll;
                if d.sel < scroll {
                    scroll = d.sel;
                } else if lh > 0 && d.sel >= scroll + lh {
                    scroll = d.sel + 1 - lh;
                }
                if let Some(e) = &d.error {
                    put_str(buf, list.x + 1, list.y, list.right(), e, Theme::popup().fg(ratatui::style::Color::Red));
                }
                if vis.is_empty() && !d.filter.is_empty() {
                    put_str(buf, list.x + 1, list.y, list.right(), "Enter: open / create this name", Theme::popup());
                }
                for (k, e) in vis.iter().enumerate().skip(scroll).take(lh) {
                    let y = list.y + (k - scroll) as u16;
                    let label = if e.is_dir { format!(" ▸ {}/", e.name) } else { format!("   {}", e.name) };
                    let st = if k == d.sel { Theme::menu_sel() } else { Theme::popup() };
                    put_str(buf, list.x, y, list.right(), &pad(&label, list.width as usize), st);
                }
                if let Modal::Open(d) = &mut self.modal {
                    d.scroll = scroll;
                }
                Some((cx, r.y + 2))
            }
        }
    }
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

fn draw_box(buf: &mut Buffer, r: Rect, title: &str) {
    if r.width < 2 || r.height < 2 {
        return;
    }
    fill(buf, r, Theme::popup());
    let st = Theme::popup();
    for x in r.x..r.right() {
        put_char(buf, x, r.y, r.right(), '─', st);
        put_char(buf, x, r.bottom() - 1, r.right(), '─', st);
    }
    for y in r.y..r.bottom() {
        put_char(buf, r.x, y, r.right(), '│', st);
        put_char(buf, r.right() - 1, y, r.right(), '│', st);
    }
    put_char(buf, r.x, r.y, r.right(), '┌', st);
    put_char(buf, r.right() - 1, r.y, r.right(), '┐', st);
    put_char(buf, r.x, r.bottom() - 1, r.right(), '└', st);
    put_char(buf, r.right() - 1, r.bottom() - 1, r.right(), '┘', st);
    if !title.is_empty() {
        put_str(buf, r.x + 2, r.y, r.right() - 1, title, st.add_modifier(Modifier::BOLD));
    }
}
