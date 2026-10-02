//! Application state, actions and input handling.

use crate::config::{config_dir, data_dir, Config};
use crate::console::{key_bytes, Console, SpawnSpec};
use crate::editor::{Doc, LintKind, Pos};
use crate::files::{self, Entry, FilesView};
use crate::lint;
use crate::plot::{ImageProtocol, PlotMode, PlotView};
use crate::help::{self, HelpView};
use crate::rsession::{r_name, r_string, EnvSnapshot, HelpPage, Session, Update, ViewData};
use crate::AppEvent;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Source,
    Console,
    Env,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RightTab {
    Files,
    Plots,
    Viewer,
    Help,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EnvRow {
    Obj(usize),
    Child(usize, usize),
}

pub struct EnvView {
    pub snap: EnvSnapshot,
    pub sel: usize,
    pub scroll: usize,
    pub expanded: HashSet<String>,
    pub sort_size: bool,
    pub preview_scroll: usize,
}

impl EnvView {
    pub fn order(&self) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.snap.objects.len()).collect();
        if self.sort_size {
            idx.sort_by(|a, b| {
                self.snap.objects[*b].size.partial_cmp(&self.snap.objects[*a].size).unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        idx
    }
    pub fn rows(&self) -> Vec<EnvRow> {
        let mut rows = Vec::new();
        for i in self.order() {
            rows.push(EnvRow::Obj(i));
            let o = &self.snap.objects[i];
            if self.expanded.contains(&o.name) {
                for c in 0..o.children.len() {
                    rows.push(EnvRow::Child(i, c));
                }
            }
        }
        rows
    }
    pub fn selected_obj(&self) -> Option<usize> {
        match self.rows().get(self.sel) {
            Some(EnvRow::Obj(i)) | Some(EnvRow::Child(i, _)) => Some(*i),
            None => None,
        }
    }
}

#[derive(Default)]
pub struct ViewerView {
    pub data: Option<ViewData>,
    pub row: usize,
    pub col: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PromptKind {
    SaveAs,
    Goto,
    Find,
    ReplaceFind,
    ReplaceWith(String),
    SavePlot,
    NewFile,
}

pub struct Prompt {
    pub kind: PromptKind,
    pub title: String,
    pub input: Vec<char>,
    pub cursor: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConfirmKind {
    Quit,
    CloseTab(u64),
}

pub struct OpenDlg {
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    pub filter: String,
    pub sel: usize,
    pub scroll: usize,
    pub error: Option<String>,
}

impl OpenDlg {
    pub fn visible(&self) -> Vec<&Entry> {
        let f = self.filter.to_lowercase();
        self.entries.iter().filter(|e| f.is_empty() || e.name.to_lowercase().contains(&f)).collect()
    }
}

pub enum Modal {
    None,
    Menu { menu: usize, item: usize },
    Prompt(Prompt),
    Confirm(ConfirmKind, String),
    Open(OpenDlg),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    NewFile,
    OpenFile,
    Save,
    SaveAs,
    CloseTab,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Find,
    Replace,
    Goto,
    RunLine,
    SourceFile,
    SourceEcho,
    InsertPipe,
    InsertAssign,
    Comment,
    MoveUp,
    MoveDown,
    DeleteLine,
    Lint,
    Outline,
    FocusSource,
    FocusConsole,
    FocusEnv,
    FocusFiles,
    FocusPlots,
    FocusViewer,
    FocusHelp,
    Zoom,
    ShowAll,
    PlotPrev,
    PlotNext,
    PlotMode,
    PlotImage,
    PlotRedraw,
    PlotSave,
    Interrupt,
    Restart,
    ClearConsole,
    SetWd,
    ClearWorkspace,
    Help,
    AutoPairs,
}

pub struct MenuItem {
    pub label: &'static str,
    pub key: &'static str,
    pub action: Action,
}

const fn mi(label: &'static str, key: &'static str, action: Action) -> MenuItem {
    MenuItem { label, key, action }
}

pub const MENUS: &[(&str, &[MenuItem])] = &[
    (
        "File",
        &[
            mi("New file", "Ctrl+N", Action::NewFile),
            mi("Open...", "Ctrl+O", Action::OpenFile),
            mi("Save", "Ctrl+S", Action::Save),
            mi("Save as...", "", Action::SaveAs),
            mi("Close tab", "Ctrl+W", Action::CloseTab),
            mi("Quit", "Ctrl+Q", Action::Quit),
        ],
    ),
    (
        "Edit",
        &[
            mi("Undo", "Ctrl+Z", Action::Undo),
            mi("Redo", "Ctrl+Y", Action::Redo),
            mi("Cut", "Ctrl+X", Action::Cut),
            mi("Copy", "Ctrl+C", Action::Copy),
            mi("Paste", "Ctrl+V", Action::Paste),
            mi("Select all", "Ctrl+A", Action::SelectAll),
            mi("Find", "Ctrl+F", Action::Find),
            mi("Replace all", "Ctrl+H", Action::Replace),
            mi("Go to line", "Ctrl+G", Action::Goto),
            mi("Auto-close brackets", "toggle", Action::AutoPairs),
        ],
    ),
    (
        "Code",
        &[
            mi("Run line / selection", "Ctrl+Enter", Action::RunLine),
            mi("Source file", "Ctrl+Shift+S", Action::SourceFile),
            mi("Source with echo", "Ctrl+Shift+Enter", Action::SourceEcho),
            mi("Insert pipe |>", "Ctrl+Shift+M", Action::InsertPipe),
            mi("Insert <-", "Alt+-", Action::InsertAssign),
            mi("Comment / uncomment", "Ctrl+Shift+C", Action::Comment),
            mi("Move line up", "Alt+Up", Action::MoveUp),
            mi("Move line down", "Alt+Down", Action::MoveDown),
            mi("Delete line", "Ctrl+D", Action::DeleteLine),
            mi("Lint file", "F7", Action::Lint),
            mi("Show outline", "Ctrl+Shift+O", Action::Outline),
        ],
    ),
    (
        "View",
        &[
            mi("Source", "Ctrl+1", Action::FocusSource),
            mi("Console", "Ctrl+2", Action::FocusConsole),
            mi("Files", "Ctrl+5", Action::FocusFiles),
            mi("Plots", "Ctrl+6", Action::FocusPlots),
            mi("Environment", "Ctrl+8", Action::FocusEnv),
            mi("Help", "Ctrl+3", Action::FocusHelp),
            mi("Viewer", "Ctrl+9", Action::FocusViewer),
            mi("Zoom pane", "Ctrl+Shift+1..9", Action::Zoom),
            mi("Show all panes", "Ctrl+Shift+0", Action::ShowAll),
        ],
    ),
    (
        "Plots",
        &[
            mi("Previous plot", "Left", Action::PlotPrev),
            mi("Next plot", "Right", Action::PlotNext),
            mi("Cycle display mode", "m", Action::PlotMode),
            mi("Image view on/off", "i", Action::PlotImage),
            mi("Redraw at pane size", "r", Action::PlotRedraw),
            mi("Save plot as PNG...", "s", Action::PlotSave),
        ],
    ),
    (
        "Session",
        &[
            mi("Interrupt R", "Ctrl+C", Action::Interrupt),
            mi("Restart R", "Ctrl+Shift+F10", Action::Restart),
            mi("Clear console", "Ctrl+L", Action::ClearConsole),
            mi("Set wd to file location", "", Action::SetWd),
            mi("Clear workspace", "", Action::ClearWorkspace),
        ],
    ),
    ("Help", &[mi("Keyboard shortcuts", "F1", Action::Help), mi("Show help pane", "Ctrl+3", Action::FocusHelp)]),
];

pub const HELP: &[&str] = &[
    "tRm - a lightweight RStudio-like IDE for the terminal",
    "",
    "PANES                                 (Alt+digit works where Ctrl+digit doesn't)",
    "  Ctrl+1  Source        Ctrl+2  Console      Ctrl+5  Files",
    "  Ctrl+6  Plots         Ctrl+8  Environment  Ctrl+9  Viewer   Ctrl+3  Help",
    "  Ctrl+Shift+1/2/5/6/8/9  zoom that pane     Ctrl+Shift+0  show all",
    "  Alt+Z   zoom / unzoom current pane        mouse: click panes, drag borders",
    "",
    "RUNNING CODE",
    "  Ctrl+Enter         run line / selection / whole statement (also Ctrl+J, Alt+Enter)",
    "  Ctrl+A Ctrl+Enter  run the whole file",
    "  Ctrl+Shift+S       source file          Ctrl+Shift+Enter  source with echo",
    "  Ctrl+Shift+M       insert |>  (Alt+M)   Alt+-             insert <-",
    "  Ctrl+L             clear console        Ctrl+Shift+F10    restart R",
    "",
    "EDITING",
    "  Ctrl+Z / Ctrl+Y undo/redo   Ctrl+X/C/V cut/copy/paste   Ctrl+A select all",
    "  Ctrl+F find (Enter next, Shift+Enter/Up prev)   F3 find next   Ctrl+H replace",
    "  Ctrl+G go to line   Ctrl+D delete line   Alt+Up/Down move line",
    "  Ctrl+Shift+C or Alt+C or Ctrl+/ comment    Tab/Shift+Tab indent",
    "  Ctrl+Shift+O (Alt+O) document outline     F7 lint now",
    "  Ctrl+N new   Ctrl+O open   Ctrl+S save   Ctrl+W close   Ctrl+PgUp/PgDn tabs",
    "",
    "ENVIRONMENT  Up/Down select  Enter/Right expand  Left collapse",
    "             v View()  Del remove object  s sort by size/name",
    "FILES        Enter open  Backspace up  . hidden  w setwd here  g go to R wd",
    "PLOTS        Left/Right history  m mode (ascii/braille/blocks/image)",
    "             i image view  r redraw at pane size  s save PNG",
    "VIEWER       arrows / PgUp / PgDn / Home / End to scroll",
    "HELP         type to search all installed packages, Enter open, Esc clear",
    "             Backspace back, Alt+Left/Right history, ?topic opens here directly",
    "",
    "Console: Shift+PgUp/PgDn or mouse wheel scroll back. Hold Shift to select",
    "text with the mouse using your terminal's own selection.",
    "Config: ~/.config/trm/config  (plot_mode, image_protocol, lint, r_command ...)",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Drag {
    None,
    VSep,
    LSplit,
    RSplit,
    Select,
}

#[derive(Default, Clone)]
pub struct Rects {
    pub menubar: Rect,
    pub status: Rect,
    pub body: Rect,
    pub source: Rect,
    pub console: Rect,
    pub env: Rect,
    pub right: Rect,
    pub vsep: Rect,
    pub source_text: Rect,
    pub gutter_w: u16,
    pub outline: Rect,
    pub console_body: Rect,
    pub env_list: Rect,
    pub env_preview: Rect,
    pub right_body: Rect,
    pub source_tabs: Vec<(Rect, usize)>,
    pub right_tabs: Vec<(Rect, RightTab)>,
    pub menu_titles: Vec<(Rect, usize)>,
    pub menu_popup: Rect,
    pub open_list: Rect,
}

pub struct App {
    pub cfg: Config,
    pub tx: Sender<AppEvent>,
    pub docs: Vec<Doc>,
    pub active: usize,
    pub console: Option<Console>,
    pub console_gen: u64,
    pub console_error: Option<String>,
    r_queue: VecDeque<Vec<u8>>,
    queue_since: Instant,
    pub session: Session,
    pub env: EnvView,
    pub files: FilesView,
    pub plots: PlotView,
    pub viewer: ViewerView,
    pub help: HelpView,
    help_render_n: u64,
    pub focus: Pane,
    pub right_tab: RightTab,
    pub zoom: Option<Pane>,
    pub vsplit: f32,
    pub lsplit: f32,
    pub rsplit: f32,
    pub rects: Rects,
    pub modal: Modal,
    pub status: Option<(String, Instant, bool)>,
    pub clipboard: String,
    pub osc52: Option<String>,
    pub show_outline: bool,
    pub drag: Drag,
    last_click: (Instant, u16, u16, u8),
    pub quit: bool,
    pub dirty: bool,
    pub last_search: String,
    find_origin: Pos,
    pub r_wd: Option<PathBuf>,
    pub cell_px: (u16, u16),
    last_plot_size: (u32, u32),
    pub image_protocol: ImageProtocol,
    pub image_req: Option<(PathBuf, Rect)>,
    pub help_cursor: Option<(u16, u16)>,
    pub need_full_redraw: bool,
    last_autosave: Instant,
    pub kbd_enhanced: bool,
    pub start_cwd: PathBuf,
    pub panics: u32,
}

impl App {
    pub fn new(cfg: Config, tx: Sender<AppEvent>, args: Vec<String>) -> Result<App, String> {
        let session = Session::new()?;
        let cwd = std::env::current_dir().unwrap_or_else(|_| files::home());
        let protocol = cfg.image_protocol;
        let mode = cfg.plot_mode;
        let mut app = App {
            tx,
            docs: Vec::new(),
            active: 0,
            console: None,
            console_gen: 0,
            console_error: None,
            r_queue: VecDeque::new(),
            queue_since: Instant::now(),
            session,
            env: EnvView { snap: EnvSnapshot::default(), sel: 0, scroll: 0, expanded: HashSet::new(), sort_size: false, preview_scroll: 0 },
            files: FilesView::new(cwd.clone()),
            plots: PlotView::new(mode),
            viewer: ViewerView::default(),
            help: HelpView::new(),
            help_render_n: 0,
            focus: Pane::Source,
            right_tab: RightTab::Files,
            zoom: None,
            vsplit: 0.56,
            lsplit: 0.55,
            rsplit: 0.45,
            rects: Rects::default(),
            modal: Modal::None,
            status: None,
            clipboard: String::new(),
            osc52: None,
            show_outline: false,
            drag: Drag::None,
            last_click: (Instant::now() - Duration::from_secs(10), 0, 0, 0),
            quit: false,
            dirty: true,
            last_search: String::new(),
            find_origin: Pos::default(),
            r_wd: None,
            cell_px: (8, 16),
            last_plot_size: (0, 0),
            image_protocol: protocol,
            image_req: None,
            help_cursor: None,
            need_full_redraw: false,
            last_autosave: Instant::now(),
            kbd_enhanced: false,
            start_cwd: cwd,
            panics: 0,
            cfg,
        };
        for w in app.cfg.warnings.clone() {
            app.msg_err(&w);
        }
        // files from the command line, else the previous session
        let mut opened = false;
        for a in &args {
            let p = files::expand(a, &app.start_cwd);
            if p.is_dir() {
                app.files.cd(p);
                continue;
            }
            app.open_path(&p);
            opened = true;
        }
        if !opened && args.is_empty() && app.cfg.restore_session {
            app.restore_session();
        }
        if app.docs.is_empty() {
            app.docs.push(Doc::new());
            app.active = 0;
        }
        app.check_recovery();
        Ok(app)
    }

    // ---------- small helpers ----------

    pub fn doc(&self) -> &Doc {
        &self.docs[self.active.min(self.docs.len() - 1)]
    }

    pub fn doc_mut(&mut self) -> &mut Doc {
        let i = self.active.min(self.docs.len() - 1);
        &mut self.docs[i]
    }

    pub fn msg(&mut self, s: &str) {
        self.status = Some((s.to_string(), Instant::now(), false));
    }

    pub fn msg_err(&mut self, s: &str) {
        self.status = Some((s.to_string(), Instant::now(), true));
    }

    fn source_view_size(&self) -> (usize, usize) {
        let r = self.rects.source_text;
        ((r.height as usize).max(1), (r.width.saturating_sub(self.rects.gutter_w) as usize).max(1))
    }

    fn scroll_cursor_into_view(&mut self) {
        let (h, w) = self.source_view_size();
        self.doc_mut().ensure_visible(h, w);
    }

    pub fn r_running(&self) -> bool {
        self.console.as_ref().map(|c| c.alive).unwrap_or(false)
    }

    pub fn r_busy(&mut self) -> bool {
        match self.console.as_mut() {
            Some(c) if c.alive => !c.at_prompt(),
            _ => false,
        }
    }

    // ---------- R process ----------

    pub fn start_r(&mut self, rows: u16, cols: u16) {
        self.console_gen += 1;
        self.session.reset();
        let mut env = vec![
            ("TRM_DIR".to_string(), self.session.dir.to_string_lossy().to_string()),
            ("R_PROFILE_USER".to_string(), self.session.init.to_string_lossy().to_string()),
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("TRM_COLS".to_string(), cols.to_string()),
        ];
        if let Some(p) = crate::rsession::user_profile(&self.start_cwd) {
            env.push(("TRM_USER_PROFILE".to_string(), p.to_string_lossy().to_string()));
        }
        let spec = SpawnSpec {
            program: self.cfg.r_command.clone(),
            args: self.cfg.r_args.clone(),
            env,
            cwd: Some(self.start_cwd.clone()),
        };
        match Console::spawn(&spec, rows, cols, self.tx.clone(), self.console_gen) {
            Ok(c) => {
                self.console = Some(c);
                self.console_error = None;
            }
            Err(e) => {
                self.console = None;
                self.console_error = Some(e.clone());
                self.msg_err(&e);
            }
        }
        self.r_queue.clear();
    }

    fn restart_r(&mut self) {
        if let Some(mut c) = self.console.take() {
            c.kill();
        }
        self.env.snap = EnvSnapshot::default();
        self.env.sel = 0;
        let r = self.rects.console_body;
        self.start_r(r.height.max(5), r.width.max(20));
        self.msg("R restarted");
    }

    fn console_write(&mut self, data: &[u8]) {
        if let Some(c) = self.console.as_mut() {
            c.write(data);
        }
    }

    /// Send code to R as if typed at the console (the RStudio REPL workflow).
    pub fn send_code(&mut self, code: &str) {
        if !self.r_running() {
            self.msg_err("R is not running - Session > Restart R");
            return;
        }
        let code = code.replace("\r\n", "\n").replace('\t', "  ");
        let mut lines: Vec<&str> = code.split('\n').collect();
        while lines.len() > 1 && lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
            lines.pop();
        }
        let bytes = if lines.len() > 300 {
            let f = self.session.dir.join("run.R");
            if let Err(e) = std::fs::write(&f, lines.join("\n")) {
                self.msg_err(&format!("cannot write temp file: {e}"));
                return;
            }
            format!("source({}, echo = TRUE, max.deparse.length = Inf)\r", r_string(&f.to_string_lossy())).into_bytes()
        } else {
            let mut s = lines.join("\r");
            s.push('\r');
            s.into_bytes()
        };
        let idle = self.console.as_mut().map(|c| c.at_prompt()).unwrap_or(false);
        if idle && self.r_queue.is_empty() {
            self.console_write(&bytes);
        } else {
            if self.r_queue.is_empty() {
                self.queue_since = Instant::now();
            }
            self.r_queue.push_back(bytes);
            self.msg(&format!("R is busy - {} command(s) queued", self.r_queue.len()));
        }
    }

    fn flush_queue(&mut self) {
        if self.r_queue.is_empty() {
            return;
        }
        let Some(c) = self.console.as_mut() else {
            self.r_queue.clear();
            return;
        };
        let stale = c.last_output.elapsed() > Duration::from_secs(4) && self.queue_since.elapsed() > Duration::from_secs(4);
        if c.at_prompt() || stale {
            if let Some(b) = self.r_queue.pop_front() {
                c.write(&b);
                self.queue_since = Instant::now();
            }
        }
    }

    /// Ctrl+Enter: run the selection, or the statement under the cursor.
    pub fn run_current(&mut self) {
        let doc = self.doc_mut();
        if let Some(sel) = doc.selected_text() {
            self.send_code(&sel);
            return;
        }
        let l = doc.cur.line;
        let blank = doc.lines.get(l).map(|x| x.iter().all(|c| c.is_whitespace())).unwrap_or(true);
        if blank {
            // skip forward to the next non-empty line
            let n = doc.lines.len();
            let mut k = l + 1;
            while k < n && doc.lines[k].iter().all(|c| c.is_whitespace()) {
                k += 1;
            }
            if k < n {
                doc.move_to(Pos::new(k, 0), false);
            }
            self.scroll_cursor_into_view();
            return;
        }
        let (s, e) = doc.statement_range(l);
        let code: Vec<String> = (s..=e).map(|i| doc.lines[i].iter().collect()).collect();
        // advance to the next non-blank line like RStudio
        let n = doc.lines.len();
        let mut k = e + 1;
        while k < n && doc.lines[k].iter().all(|c| c.is_whitespace()) {
            k += 1;
        }
        if k < n {
            doc.move_to(Pos::new(k, 0), false);
        } else {
            let end = doc.end_pos();
            doc.move_to(Pos::new(e, doc.line_len(e)), false);
            let _ = end;
        }
        self.scroll_cursor_into_view();
        self.send_code(&code.join("\n"));
    }

    fn source_file(&mut self, echo: bool) {
        let path = if self.doc().path.is_some() {
            if self.doc().modified {
                self.save_doc(self.active);
            }
            self.doc().path.clone()
        } else {
            let f = self.session.dir.join("untitled.R");
            let text = self.doc().text();
            match std::fs::write(&f, text) {
                Ok(_) => Some(f),
                Err(e) => {
                    self.msg_err(&format!("cannot write temp file: {e}"));
                    None
                }
            }
        };
        if let Some(p) = path {
            let cmd = if echo {
                format!("source({}, echo = TRUE)", r_string(&p.to_string_lossy()))
            } else {
                format!("source({})", r_string(&p.to_string_lossy()))
            };
            self.send_code(&cmd);
        }
    }

    fn insert_snippet(&mut self, s: &str) {
        match self.focus {
            Pane::Console => {
                let b = s.as_bytes().to_vec();
                self.console_write(&b);
            }
            _ => {
                self.focus = Pane::Source;
                let doc = self.doc_mut();
                // avoid doubled spaces
                let prev_space = doc.cur.col > 0 && doc.lines[doc.cur.line].get(doc.cur.col - 1) == Some(&' ');
                let t = if prev_space { s.trim_start() } else { s };
                doc.insert_text(t);
                self.scroll_cursor_into_view();
            }
        }
    }

    // ---------- files ----------

    pub fn open_path(&mut self, p: &Path) {
        let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        if let Some(i) = self.docs.iter().position(|d| d.path.as_deref() == Some(&p)) {
            self.active = i;
            self.focus = Pane::Source;
            return;
        }
        if files::is_image_ext(&p) {
            self.plots.push(p.clone());
            self.right_tab = RightTab::Plots;
            return;
        }
        let doc = if p.exists() {
            match Doc::load(&p) {
                Ok(d) => d,
                Err(e) => {
                    self.msg_err(&format!("cannot open {}: {e}", p.display()));
                    return;
                }
            }
        } else {
            let mut d = Doc::new();
            d.path = Some(p.clone());
            self.msg(&format!("new file {}", p.display()));
            d
        };
        // replace a pristine untitled tab
        if self.docs.len() == 1 && self.docs[0].path.is_none() && !self.docs[0].modified && self.docs[0].lines.len() <= 1 {
            self.docs.clear();
        }
        self.docs.push(doc);
        self.active = self.docs.len() - 1;
        self.focus = Pane::Source;
        if p.exists() {
            self.lint_doc(self.active);
        }
    }

    pub fn save_doc(&mut self, i: usize) -> bool {
        if self.docs.get(i).map(|d| d.path.is_none()).unwrap_or(true) {
            self.active = i.min(self.docs.len().saturating_sub(1));
            self.open_prompt(PromptKind::SaveAs, "Save as", &self.default_dir_string());
            return false;
        }
        let res = self.docs[i].save();
        match res {
            Ok(()) => {
                let name = self.docs[i].title();
                self.msg(&format!("saved {name}"));
                self.remove_recovery(i);
                self.lint_doc(i);
                true
            }
            Err(e) => {
                self.msg_err(&format!("save failed: {e}"));
                false
            }
        }
    }

    fn default_dir_string(&self) -> String {
        let d = self.doc().path.as_ref().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_else(|| self.files.dir.clone());
        let mut s = d.to_string_lossy().to_string();
        if !s.ends_with('/') {
            s.push('/');
        }
        s
    }

    fn close_tab(&mut self, force: bool) {
        if !force && self.doc().modified {
            let id = self.doc().id;
            let t = format!("'{}' has unsaved changes. Save? (y)es / (n)o / Esc", self.doc().title());
            self.modal = Modal::Confirm(ConfirmKind::CloseTab(id), t);
            return;
        }
        self.remove_recovery(self.active);
        self.docs.remove(self.active.min(self.docs.len() - 1));
        if self.docs.is_empty() {
            self.docs.push(Doc::new());
        }
        self.active = self.active.min(self.docs.len() - 1);
    }

    pub fn request_quit(&mut self) {
        let unsaved: Vec<String> = self.docs.iter().filter(|d| d.modified).map(|d| d.title()).collect();
        if unsaved.is_empty() {
            self.do_quit();
        } else {
            let t = format!("Unsaved: {}. Save all? (y)es / (n)o, quit anyway / Esc", unsaved.join(", "));
            self.modal = Modal::Confirm(ConfirmKind::Quit, t);
        }
    }

    fn do_quit(&mut self) {
        self.save_session();
        for i in 0..self.docs.len() {
            if !self.docs[i].modified {
                self.remove_recovery(i);
            }
        }
        self.quit = true;
    }

    fn lint_doc(&mut self, i: usize) {
        let Some(d) = self.docs.get(i) else { return };
        let is_r = d.path.as_ref().map(|p| {
            let e = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            e == "r"
        }).unwrap_or(true);
        if !is_r || self.cfg.lint == lint::LintMode::Off {
            return;
        }
        let f = self.session.dir.join(format!("lint-{}.R", d.id));
        if std::fs::write(&f, d.text()).is_err() {
            return;
        }
        lint::spawn(self.cfg.rscript.clone(), f, self.cfg.lint, d.id, d.version, self.tx.clone());
    }

    fn session_file() -> PathBuf {
        config_dir().join("session")
    }

    fn save_session(&self) {
        let mut s = String::new();
        for (i, d) in self.docs.iter().enumerate() {
            if let Some(p) = &d.path {
                if p.exists() {
                    s.push_str(if i == self.active { "*" } else { " " });
                    s.push_str(&p.to_string_lossy());
                    s.push('\n');
                }
            }
        }
        let _ = std::fs::create_dir_all(config_dir());
        let _ = std::fs::write(Self::session_file(), s);
    }

    fn restore_session(&mut self) {
        let Ok(text) = std::fs::read_to_string(Self::session_file()) else { return };
        let mut active = None;
        for l in text.lines() {
            if l.len() < 2 {
                continue;
            }
            let (mark, path) = l.split_at(1);
            let p = PathBuf::from(path);
            if p.is_file() {
                self.open_path(&p);
                if mark == "*" {
                    active = Some(self.docs.len() - 1);
                }
            }
        }
        if let Some(a) = active {
            self.active = a.min(self.docs.len().saturating_sub(1));
        }
    }

    fn recovery_path(d: &Doc) -> PathBuf {
        let name = match &d.path {
            Some(p) => p.to_string_lossy().replace('/', "%"),
            None => format!("untitled-{}.R", d.id),
        };
        data_dir().join("recovery").join(name)
    }

    fn autosave(&mut self) {
        for d in &self.docs {
            if d.modified {
                let p = Self::recovery_path(d);
                let _ = std::fs::create_dir_all(p.parent().unwrap_or(Path::new("/tmp")));
                let _ = std::fs::write(&p, d.text());
            }
        }
    }

    fn remove_recovery(&self, i: usize) {
        if let Some(d) = self.docs.get(i) {
            let _ = std::fs::remove_file(Self::recovery_path(d));
        }
    }

    fn check_recovery(&mut self) {
        let dir = data_dir().join("recovery");
        let n = std::fs::read_dir(&dir).map(|r| r.count()).unwrap_or(0);
        if n > 0 {
            self.msg(&format!("{n} unsaved file(s) recovered in {} - use File > Open", dir.display()));
        }
    }

    /// Called after a caught panic: write recovery copies of everything.
    pub fn emergency_save(&mut self) {
        self.autosave();
    }

    // ---------- periodic work ----------

    pub fn tick(&mut self) {
        let updates = self.session.poll();
        for u in updates {
            self.dirty = true;
            match u {
                Update::Env(snap) => {
                    let sel_name = self.env.selected_obj().and_then(|i| self.env.snap.objects.get(i)).map(|o| o.name.clone());
                    self.env.snap = snap;
                    let names: HashSet<String> = self.env.snap.objects.iter().map(|o| o.name.clone()).collect();
                    self.env.expanded.retain(|n| names.contains(n));
                    if let Some(n) = sel_name {
                        let rows = self.env.rows();
                        if let Some(pos) = rows.iter().position(|r| matches!(r, EnvRow::Obj(i) if self.env.snap.objects[*i].name == n)) {
                            self.env.sel = pos;
                        }
                    }
                    let len = self.env.rows().len();
                    if self.env.sel >= len {
                        self.env.sel = len.saturating_sub(1);
                    }
                }
                Update::Plot(p) => {
                    self.plots.push(p);
                    if self.right_tab != RightTab::Plots && self.zoom.is_none() {
                        self.right_tab = RightTab::Plots;
                    }
                }
                Update::Wd(wd) => {
                    let changed = self.r_wd.as_ref() != Some(&wd);
                    self.r_wd = Some(wd.clone());
                    if changed && self.files.follow_r {
                        self.files.cd(wd);
                    }
                }
                Update::View(v) => {
                    self.viewer.data = Some(v);
                    self.viewer.row = 0;
                    self.viewer.col = 0;
                    self.show_right_tab(RightTab::Viewer);
                }
                Update::Help(h) => {
                    self.help.show(h);
                    self.show_right_tab(RightTab::Help);
                }
                Update::Libs(libs) => {
                    if libs != self.help.index_libs {
                        self.help.index_libs = libs.clone();
                        help::build_in_background(libs, self.tx.clone());
                    }
                }
                Update::Packages(p) => self.help.attached = p,
            }
        }
        self.flush_queue();
        if let Some((_, t, _)) = &self.status {
            if t.elapsed() > Duration::from_secs(6) {
                self.status = None;
                self.dirty = true;
            }
        }
        if self.last_autosave.elapsed() > Duration::from_secs(30) {
            self.last_autosave = Instant::now();
            self.autosave();
        }
    }

    /// Keep R's plot size in sync with the pane size.
    pub fn update_plot_size(&mut self, area: Rect) {
        let (cw, ch) = (self.cell_px.0.max(1) as u32, self.cell_px.1.max(1) as u32);
        let mut w = area.width as u32 * cw;
        let mut h = area.height as u32 * ch;
        // scale up small panes so text in the plot stays legible, keep aspect
        let scale = (640.0 / w.max(1) as f64).max(1.0);
        w = ((w as f64 * scale) as u32).clamp(320, 2400);
        h = ((h as f64 * scale) as u32).clamp(240, 2400);
        if (w, h) != self.last_plot_size {
            self.last_plot_size = (w, h);
            self.session.write_plot_size(w, h);
        }
    }

    // ---------- event dispatch ----------

    pub fn handle(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Term(e) => self.handle_term(e),
            AppEvent::Pty(gen, data) => {
                if let Some(c) = self.console.as_mut() {
                    if c.gen == gen {
                        c.process(&data);
                        self.dirty = true;
                    }
                }
                self.flush_queue();
            }
            AppEvent::PtyExit(gen) => {
                if let Some(c) = self.console.as_mut() {
                    if c.gen == gen {
                        c.alive = false;
                        c.reap();
                        c.process(b"\r\n\x1b[1;31m[R exited - press Enter here or Ctrl+Shift+F10 to restart]\x1b[0m\r\n");
                        self.dirty = true;
                    }
                }
            }
            AppEvent::HelpIndex(idx) => {
                self.help.index = Some(idx);
                if !self.help.query.is_empty() {
                    self.help.update_results();
                }
                self.dirty = true;
            }
            AppEvent::HelpPage(res) => {
                match res {
                    Ok(p) => {
                        self.help.show(p);
                        self.show_right_tab(RightTab::Help);
                    }
                    Err(e) => {
                        self.help.loading = None;
                        self.msg_err(&e);
                    }
                }
                self.dirty = true;
            }
            AppEvent::Lint(id, version, res) => {
                match res {
                    Ok(lints) => {
                        if let Some(d) = self.docs.iter_mut().find(|d| d.id == id) {
                            if d.version == version {
                                let errs = lints.iter().filter(|l| l.kind == LintKind::Error).count();
                                let n = lints.len();
                                d.lints = lints;
                                if errs > 0 {
                                    self.msg_err(&format!("{errs} syntax error(s) - see markers in the gutter"));
                                } else if n > 0 {
                                    self.msg(&format!("lint: {n} note(s)"));
                                }
                            }
                        }
                    }
                    Err(e) => self.msg_err(&e),
                }
                self.dirty = true;
            }
        }
    }

    fn handle_term(&mut self, e: Event) {
        self.dirty = true;
        match e {
            Event::Key(k) => {
                if k.kind == KeyEventKind::Release {
                    return;
                }
                self.handle_key(k);
            }
            Event::Mouse(m) => self.handle_mouse(m),
            Event::Paste(s) => self.handle_paste(&s),
            Event::Resize(_, _) => {
                self.need_full_redraw = true;
                if let Ok(ws) = ratatui::crossterm::terminal::window_size() {
                    if ws.width > 0 && ws.columns > 0 && ws.height > 0 && ws.rows > 0 {
                        self.cell_px = ((ws.width / ws.columns).max(1), (ws.height / ws.rows).max(1));
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_paste(&mut self, s: &str) {
        match &mut self.modal {
            Modal::Prompt(p) => {
                for c in s.chars().filter(|c| *c != '\n' && *c != '\r') {
                    p.input.insert(p.cursor, c);
                    p.cursor += 1;
                }
                return;
            }
            Modal::None => {}
            _ => return,
        }
        match self.focus {
            Pane::Console => {
                let bracketed = self.console.as_ref().map(|c| c.bracketed_paste()).unwrap_or(false);
                let body = s.replace("\r\n", "\n").replace('\n', "\r");
                let data = if bracketed { format!("\x1b[200~{body}\x1b[201~") } else { body };
                self.console_write(data.as_bytes());
            }
            Pane::Right if self.right_tab == RightTab::Help => {
                let line = s.lines().next().unwrap_or("").trim();
                self.help.query.push_str(line);
                self.help.update_results();
            }
            Pane::Source => {
                let t = s.replace("\r\n", "\n").replace('\r', "\n");
                self.doc_mut().insert_text(&t);
                self.doc_mut().close_group();
                self.scroll_cursor_into_view();
            }
            _ => {}
        }
    }

    pub fn do_action(&mut self, a: Action) {
        match a {
            Action::NewFile => {
                self.docs.push(Doc::new());
                self.active = self.docs.len() - 1;
                self.focus = Pane::Source;
            }
            Action::OpenFile => self.open_dialog(),
            Action::Save => {
                self.save_doc(self.active);
            }
            Action::SaveAs => {
                let init = match &self.doc().path {
                    Some(p) => p.to_string_lossy().to_string(),
                    None => self.default_dir_string(),
                };
                self.open_prompt(PromptKind::SaveAs, "Save as", &init);
            }
            Action::CloseTab => self.close_tab(false),
            Action::Quit => self.request_quit(),
            Action::Undo => {
                self.doc_mut().undo();
                self.scroll_cursor_into_view();
            }
            Action::Redo => {
                self.doc_mut().redo();
                self.scroll_cursor_into_view();
            }
            Action::Cut => {
                let d = self.doc_mut();
                if d.selection().is_none() {
                    let l = d.cur.line;
                    d.anchor = Some(Pos::new(l, 0));
                    d.cur = if l + 1 < d.lines.len() { Pos::new(l + 1, 0) } else { Pos::new(l, d.line_len(l)) };
                }
                if let Some(t) = d.selected_text() {
                    d.delete_selection(crate::editor::EditKind::Other);
                    self.set_clipboard(t);
                }
                self.scroll_cursor_into_view();
            }
            Action::Copy => {
                let d = self.doc();
                let t = match d.selected_text() {
                    Some(t) => t,
                    None => {
                        let mut l: String = d.lines[d.cur.line].iter().collect();
                        l.push('\n');
                        l
                    }
                };
                self.set_clipboard(t);
                self.msg("copied");
            }
            Action::Paste => {
                let t = self.clipboard.clone();
                if !t.is_empty() {
                    self.doc_mut().insert_text(&t);
                    self.doc_mut().close_group();
                    self.scroll_cursor_into_view();
                }
            }
            Action::SelectAll => self.doc_mut().select_all(),
            Action::Find => {
                self.find_origin = self.doc().cur;
                let init = self.doc().selected_text().filter(|s| !s.contains('\n')).unwrap_or_else(|| self.last_search.clone());
                self.open_prompt(PromptKind::Find, "Find", &init);
            }
            Action::Replace => {
                let init = self.doc().selected_text().filter(|s| !s.contains('\n')).unwrap_or_else(|| self.last_search.clone());
                self.open_prompt(PromptKind::ReplaceFind, "Replace all - find", &init);
            }
            Action::Goto => self.open_prompt(PromptKind::Goto, "Go to line", ""),
            Action::RunLine => self.run_current(),
            Action::SourceFile => self.source_file(false),
            Action::SourceEcho => self.source_file(true),
            Action::InsertPipe => self.insert_snippet(" |> "),
            Action::InsertAssign => self.insert_snippet(" <- "),
            Action::Comment => self.doc_mut().toggle_comment(),
            Action::MoveUp => self.doc_mut().move_lines(true),
            Action::MoveDown => self.doc_mut().move_lines(false),
            Action::DeleteLine => self.doc_mut().delete_lines(),
            Action::Lint => {
                if self.cfg.lint == lint::LintMode::Off {
                    self.cfg.lint = lint::LintMode::Lintr;
                }
                self.lint_doc(self.active);
                self.msg("linting...");
            }
            Action::Outline => self.show_outline = !self.show_outline,
            Action::FocusSource => self.focus_pane(Pane::Source, None),
            Action::FocusConsole => self.focus_pane(Pane::Console, None),
            Action::FocusEnv => self.focus_pane(Pane::Env, None),
            Action::FocusFiles => self.focus_pane(Pane::Right, Some(RightTab::Files)),
            Action::FocusPlots => self.focus_pane(Pane::Right, Some(RightTab::Plots)),
            Action::FocusViewer => self.focus_pane(Pane::Right, Some(RightTab::Viewer)),
            Action::Zoom => {
                self.zoom = if self.zoom == Some(self.focus) { None } else { Some(self.focus) };
                self.need_full_redraw = true;
            }
            Action::ShowAll => {
                self.zoom = None;
                self.need_full_redraw = true;
            }
            Action::PlotPrev => {
                if self.plots.idx > 0 {
                    self.plots.idx -= 1;
                }
                self.right_tab = RightTab::Plots;
            }
            Action::PlotNext => {
                if self.plots.idx + 1 < self.plots.history.len() {
                    self.plots.idx += 1;
                }
                self.right_tab = RightTab::Plots;
            }
            Action::PlotMode => {
                self.plots.mode = self.plots.mode.next();
                if self.plots.mode == PlotMode::Image && self.image_protocol == ImageProtocol::None {
                    self.plots.mode = PlotMode::Ascii;
                }
                self.right_tab = RightTab::Plots;
                self.need_full_redraw = true;
                self.msg(&format!("plot display: {}", self.plots.mode.name()));
            }
            Action::PlotImage => {
                if self.plots.mode == PlotMode::Image {
                    self.plots.mode = self.cfg.plot_mode;
                    if self.plots.mode == PlotMode::Image {
                        self.plots.mode = PlotMode::Ascii;
                    }
                } else if self.image_protocol == ImageProtocol::None {
                    self.msg_err("this terminal doesn't seem to support images; set image_protocol = kitty or sixel in the config to force it");
                } else {
                    self.plots.mode = PlotMode::Image;
                }
                self.right_tab = RightTab::Plots;
                self.need_full_redraw = true;
            }
            Action::PlotRedraw => {
                self.right_tab = RightTab::Plots;
                self.send_code(".trm_redraw()");
            }
            Action::PlotSave => {
                if self.plots.current().is_some() {
                    let init = format!("{}plot.png", self.default_dir_string());
                    self.open_prompt(PromptKind::SavePlot, "Save plot as", &init);
                } else {
                    self.msg_err("no plot yet");
                }
            }
            Action::Interrupt => {
                if let Some(c) = &self.console {
                    c.interrupt();
                }
                self.r_queue.clear();
            }
            Action::Restart => self.restart_r(),
            Action::ClearConsole => {
                if self.r_busy() {
                    self.msg_err("R is busy");
                } else {
                    self.console_write(b"\x0c");
                }
            }
            Action::SetWd => {
                let dir = self.doc().path.as_ref().and_then(|p| p.parent().map(|p| p.to_path_buf()));
                match dir {
                    Some(d) => self.send_code(&format!("setwd({})", r_string(&d.to_string_lossy()))),
                    None => self.msg_err("save the file first"),
                }
            }
            Action::ClearWorkspace => self.send_code("rm(list = ls())"),
            Action::Help => {
                self.help.show(HelpPage::from_text("tRm keyboard shortcuts", HELP));
                self.focus_pane(Pane::Right, Some(RightTab::Help));
            }
            Action::FocusHelp => self.focus_pane(Pane::Right, Some(RightTab::Help)),
            Action::AutoPairs => {
                self.cfg.auto_pairs = !self.cfg.auto_pairs;
                let s = if self.cfg.auto_pairs { "on" } else { "off" };
                self.msg(&format!("auto-close brackets {s}"));
            }
        }
    }

    fn set_clipboard(&mut self, t: String) {
        self.osc52 = Some(t.clone());
        self.clipboard = t;
    }

    /// Bring a right-pane tab to the front without stealing keyboard focus.
    fn show_right_tab(&mut self, t: RightTab) {
        if self.right_tab != t {
            self.right_tab = t;
            self.need_full_redraw = true;
        }
    }

    fn focus_pane(&mut self, p: Pane, tab: Option<RightTab>) {
        self.focus = p;
        if let Some(t) = tab {
            if self.right_tab != t {
                self.need_full_redraw = true;
            }
            self.right_tab = t;
        }
        if self.zoom.is_some() && self.zoom != Some(p) {
            self.zoom = Some(p);
            self.need_full_redraw = true;
        }
    }

    fn zoom_pane(&mut self, p: Pane, tab: Option<RightTab>) {
        self.focus_pane(p, tab);
        self.zoom = if self.zoom == Some(p) { None } else { Some(p) };
        self.need_full_redraw = true;
    }

    fn open_prompt(&mut self, kind: PromptKind, title: &str, init: &str) {
        let input: Vec<char> = init.chars().collect();
        let cursor = input.len();
        self.modal = Modal::Prompt(Prompt { kind, title: title.to_string(), input, cursor });
    }

    fn open_dialog(&mut self) {
        let dir = self.doc().path.as_ref().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_else(|| self.files.dir.clone());
        let mut d = OpenDlg { dir: dir.clone(), entries: Vec::new(), filter: String::new(), sel: 0, scroll: 0, error: None };
        Self::load_open_dir(&mut d, dir);
        self.modal = Modal::Open(d);
    }

    fn load_open_dir(d: &mut OpenDlg, dir: PathBuf) {
        match files::list_dir(&dir, false) {
            Ok(e) => {
                d.entries = e;
                d.error = None;
            }
            Err(e) => {
                d.entries.clear();
                d.error = Some(e);
            }
        }
        d.dir = dir;
        d.filter.clear();
        d.sel = 0;
        d.scroll = 0;
    }

    // ---------- keyboard ----------

    pub fn handle_key(&mut self, ev: KeyEvent) {
        let k = norm(&ev);
        if !matches!(self.modal, Modal::None) {
            self.key_modal(ev, k);
            return;
        }
        if self.global_key(&k) {
            return;
        }
        match self.focus {
            Pane::Source => self.key_source(ev, k),
            Pane::Console => self.key_console(ev, k),
            Pane::Env => self.key_env(k),
            Pane::Right => match self.right_tab {
                RightTab::Files => self.key_files(k),
                RightTab::Plots => self.key_plots(k),
                RightTab::Viewer => self.key_viewer(k),
                RightTab::Help => self.key_help(ev, k),
            },
        }
    }

    /// Hotkeys that work from any pane. Returns true when handled.
    fn global_key(&mut self, k: &Key) -> bool {
        use KeyCode::*;
        let digit = match k.code {
            Char(c) if c.is_ascii_digit() && (k.ctrl || k.alt) => c.to_digit(10),
            _ => None,
        };
        if let Some(d) = digit {
            let target = match d {
                1 => Some((Pane::Source, None)),
                2 => Some((Pane::Console, None)),
                3 => {
                    self.do_action(Action::FocusHelp);
                    if k.shift {
                        self.zoom = Some(Pane::Right);
                        self.need_full_redraw = true;
                    }
                    return true;
                }
                5 => Some((Pane::Right, Some(RightTab::Files))),
                6 => Some((Pane::Right, Some(RightTab::Plots))),
                8 => Some((Pane::Env, None)),
                9 => Some((Pane::Right, Some(RightTab::Viewer))),
                0 => {
                    self.do_action(Action::ShowAll);
                    return true;
                }
                _ => None,
            };
            if let Some((p, t)) = target {
                if k.shift {
                    self.zoom_pane(p, t);
                } else {
                    self.focus_pane(p, t);
                }
            }
            return true;
        }
        let a = match (k.ctrl, k.alt, k.shift, k.code) {
            (true, false, false, Char('q')) => Some(Action::Quit),
            (true, false, false, Char('s')) => Some(Action::Save),
            (true, false, true, Char('s')) | (false, true, false, Char('s')) => Some(Action::SourceFile),
            (true, false, false, Char('o')) => Some(Action::OpenFile),
            (true, _, true, Enter) | (false, true, true, Enter) => Some(Action::SourceEcho),
            (true, false, true, Char('m')) | (false, true, false, Char('m')) => Some(Action::InsertPipe),
            (false, true, false, Char('-')) => Some(Action::InsertAssign),
            (true, false, true, Char('o')) | (false, true, false, Char('o')) => Some(Action::Outline),
            (true, false, true, F(10)) => Some(Action::Restart),
            (true, false, false, Char('l')) => Some(Action::ClearConsole),
            (false, true, false, Char('z')) => Some(Action::Zoom),
            (_, _, _, F(1)) => Some(Action::Help),
            (_, _, _, F(7)) => Some(Action::Lint),
            (true, false, false, PageUp) | (false, true, false, Char(',')) => {
                if !self.docs.is_empty() {
                    self.active = (self.active + self.docs.len() - 1) % self.docs.len();
                    self.focus = Pane::Source;
                }
                return true;
            }
            (true, false, false, PageDown) | (false, true, false, Char('.')) => {
                if !self.docs.is_empty() {
                    self.active = (self.active + 1) % self.docs.len();
                    self.focus = Pane::Source;
                }
                return true;
            }
            (_, _, _, F(10)) => {
                self.modal = Modal::Menu { menu: 0, item: 0 };
                return true;
            }
            _ => None,
        };
        match a {
            Some(a) => {
                self.do_action(a);
                true
            }
            None => false,
        }
    }

    fn key_source(&mut self, ev: KeyEvent, k: Key) {
        use KeyCode::*;
        let auto = self.cfg.auto_pairs;
        let (h, _) = self.source_view_size();
        let sel = k.shift;
        match (k.ctrl, k.alt, k.code) {
            (true, false, Enter) | (true, false, Char('j')) | (false, true, Enter) => {
                self.run_current();
                return;
            }
            (true, false, Char('a')) => self.do_action(Action::SelectAll),
            (true, false, Char('c')) if k.shift => self.do_action(Action::Comment),
            (false, true, Char('c')) | (true, false, Char('/')) | (true, false, Char('7')) => self.do_action(Action::Comment),
            (true, false, Char('c')) => self.do_action(Action::Copy),
            (true, false, Char('x')) => self.do_action(Action::Cut),
            (true, false, Char('v')) => self.do_action(Action::Paste),
            (true, false, Char('z')) if k.shift => self.do_action(Action::Redo),
            (true, false, Char('z')) => self.do_action(Action::Undo),
            (true, false, Char('y')) => self.do_action(Action::Redo),
            (true, false, Char('f')) => self.do_action(Action::Find),
            (true, false, Char('h')) => self.do_action(Action::Replace),
            (true, false, Char('g')) | (false, true, Char('g')) => self.do_action(Action::Goto),
            (true, false, Char('d')) => self.do_action(Action::DeleteLine),
            (true, false, Char('n')) => self.do_action(Action::NewFile),
            (true, false, Char('w')) => self.do_action(Action::CloseTab),
            (false, false, F(3)) => self.find_next(k.shift),
            (false, true, Up) => self.do_action(Action::MoveUp),
            (false, true, Down) => self.do_action(Action::MoveDown),
            (false, true, Backspace) | (true, false, Backspace) => self.doc_mut().delete_word_back(),
            (_, false, Left) => {
                let d = self.doc_mut();
                if !sel && !k.ctrl {
                    if let Some((a, _)) = d.selection() {
                        d.move_to(a, false);
                        self.scroll_cursor_into_view();
                        return;
                    }
                }
                let p = if k.ctrl {
                    d.word_left(d.cur)
                } else if d.cur.col > 0 {
                    Pos::new(d.cur.line, d.cur.col - 1)
                } else if d.cur.line > 0 {
                    Pos::new(d.cur.line - 1, d.line_len(d.cur.line - 1))
                } else {
                    d.cur
                };
                d.move_to(p, sel);
            }
            (_, false, Right) => {
                let d = self.doc_mut();
                if !sel && !k.ctrl {
                    if let Some((_, b)) = d.selection() {
                        d.move_to(b, false);
                        self.scroll_cursor_into_view();
                        return;
                    }
                }
                let p = if k.ctrl {
                    d.word_right(d.cur)
                } else if d.cur.col < d.line_len(d.cur.line) {
                    Pos::new(d.cur.line, d.cur.col + 1)
                } else if d.cur.line + 1 < d.lines.len() {
                    Pos::new(d.cur.line + 1, 0)
                } else {
                    d.cur
                };
                d.move_to(p, sel);
            }
            (true, false, Up) => {
                let d = self.doc_mut();
                d.top = d.top.saturating_sub(1);
                if d.cur.line >= d.top + h {
                    d.vertical(-1, false);
                }
                return;
            }
            (true, false, Down) => {
                let d = self.doc_mut();
                if d.top + 1 < d.lines.len() {
                    d.top += 1;
                }
                if d.cur.line < d.top {
                    d.vertical(1, false);
                }
                return;
            }
            (false, false, Up) => self.doc_mut().vertical(-1, sel),
            (false, false, Down) => self.doc_mut().vertical(1, sel),
            (_, false, PageUp) => self.doc_mut().vertical(-(h as isize - 1).max(1), sel),
            (_, false, PageDown) => self.doc_mut().vertical((h as isize - 1).max(1), sel),
            (true, false, Home) => self.doc_mut().move_to(Pos::new(0, 0), sel),
            (true, false, End) => {
                let e = self.doc().end_pos();
                self.doc_mut().move_to(e, sel);
            }
            (false, false, Home) => self.doc_mut().home(sel),
            (false, false, End) => {
                let d = self.doc_mut();
                let p = Pos::new(d.cur.line, d.line_len(d.cur.line));
                d.move_to(p, sel);
            }
            (false, false, Enter) => self.doc_mut().newline(),
            (false, false, Tab) => self.doc_mut().indent(false),
            (_, false, BackTab) => self.doc_mut().indent(true),
            (false, false, Backspace) => self.doc_mut().backspace(auto),
            (false, false, Delete) => self.doc_mut().delete_forward(),
            (false, false, Esc) => {
                let d = self.doc_mut();
                d.anchor = None;
            }
            (false, false, Char(_)) => {
                if let KeyCode::Char(c) = ev.code {
                    self.doc_mut().type_char(c, auto);
                }
            }
            _ => return,
        }
        self.scroll_cursor_into_view();
    }

    fn find_next(&mut self, backwards: bool) {
        let needle = self.last_search.clone();
        if needle.is_empty() {
            self.do_action(Action::Find);
            return;
        }
        let d = self.doc_mut();
        let from = if backwards {
            d.selection().map(|(a, _)| a).unwrap_or(d.cur)
        } else {
            d.cur
        };
        match d.find_from(&needle, from, backwards, true) {
            Some(p) => {
                let len = needle.chars().count();
                d.anchor = Some(p);
                d.cur = Pos::new(p.line, p.col + len);
                d.close_group();
                if backwards {
                    // keep cursor at the match end but remember start for the next search
                }
                self.scroll_cursor_into_view();
            }
            None => self.msg_err(&format!("'{needle}' not found")),
        }
    }

    fn key_console(&mut self, ev: KeyEvent, k: Key) {
        use KeyCode::*;
        let Some(c) = self.console.as_mut() else {
            if k.code == Enter {
                self.restart_r();
            }
            return;
        };
        if !c.alive {
            if k.code == Enter {
                self.restart_r();
            }
            return;
        }
        let page = (self.rects.console_body.height as isize - 1).max(1);
        match (k.ctrl, k.alt, k.shift, k.code) {
            (false, false, true, PageUp) => c.scroll_by(page),
            (false, false, true, PageDown) => c.scroll_by(-page),
            (true, false, false, Char('c')) => {
                c.write(b"\x03");
                if !self.r_queue.is_empty() {
                    self.r_queue.clear();
                    self.msg("interrupted - queued commands dropped");
                }
            }
            _ => {
                let app_cursor = c.application_cursor();
                let mut e = ev;
                // send the unshifted key for ctrl+letter
                if let Char(ch) = e.code {
                    if k.ctrl && ch.is_ascii_uppercase() {
                        e.code = Char(ch.to_ascii_lowercase());
                    }
                }
                let b = key_bytes(&e, app_cursor);
                c.write(&b);
            }
        }
    }

    fn key_env(&mut self, k: Key) {
        use KeyCode::*;
        let rows = self.env.rows();
        let n = rows.len();
        let page = (self.rects.env_list.height as usize).max(1);
        match k.code {
            Up => self.env.sel = self.env.sel.saturating_sub(1),
            Down => self.env.sel = (self.env.sel + 1).min(n.saturating_sub(1)),
            PageUp => self.env.sel = self.env.sel.saturating_sub(page),
            PageDown => self.env.sel = (self.env.sel + page).min(n.saturating_sub(1)),
            Home => self.env.sel = 0,
            End => self.env.sel = n.saturating_sub(1),
            Enter | Right | Char(' ') => {
                if let Some(EnvRow::Obj(i)) = rows.get(self.env.sel) {
                    let name = self.env.snap.objects[*i].name.clone();
                    if k.code == Right {
                        self.env.expanded.insert(name);
                    } else if !self.env.expanded.remove(&name) {
                        self.env.expanded.insert(name);
                    }
                }
            }
            Left => {
                if let Some(r) = rows.get(self.env.sel) {
                    let i = match r {
                        EnvRow::Obj(i) | EnvRow::Child(i, _) => *i,
                    };
                    let name = self.env.snap.objects[i].name.clone();
                    self.env.expanded.remove(&name);
                    let new_rows = self.env.rows();
                    if let Some(p) = new_rows.iter().position(|r| *r == EnvRow::Obj(i)) {
                        self.env.sel = p;
                    }
                }
            }
            Char('v') => {
                if let Some(expr) = self.env_expr(&rows) {
                    self.send_code(&format!("View({expr})"));
                }
            }
            Char('p') => {
                if let Some(expr) = self.env_expr(&rows) {
                    self.send_code(&format!("str({expr})"));
                }
            }
            Delete | Char('x') => {
                if let Some(EnvRow::Obj(i)) = rows.get(self.env.sel) {
                    let name = r_name(&self.env.snap.objects[*i].name);
                    self.send_code(&format!("rm({name})"));
                }
            }
            Char('s') => {
                self.env.sort_size = !self.env.sort_size;
                self.msg(if self.env.sort_size { "sorted by size" } else { "sorted by name" });
            }
            _ => {}
        }
        self.env.preview_scroll = 0;
    }

    fn env_expr(&self, rows: &[EnvRow]) -> Option<String> {
        match rows.get(self.env.sel)? {
            EnvRow::Obj(i) => Some(r_name(&self.env.snap.objects[*i].name)),
            EnvRow::Child(i, c) => {
                let o = &self.env.snap.objects[*i];
                let ch = o.children.get(*c)?;
                if ch.name == "..." {
                    return None;
                }
                if let Some(idx) = ch.name.strip_prefix("[[").and_then(|s| s.strip_suffix("]]")) {
                    Some(format!("{}[[{}]]", r_name(&o.name), idx))
                } else {
                    Some(format!("{}[[{}]]", r_name(&o.name), r_string(&ch.name)))
                }
            }
        }
    }

    fn key_files(&mut self, k: Key) {
        use KeyCode::*;
        let n = self.files.len();
        let page = (self.rects.right_body.height as usize).saturating_sub(1).max(1);
        match k.code {
            Up => self.files.sel = self.files.sel.saturating_sub(1),
            Down => self.files.sel = (self.files.sel + 1).min(n.saturating_sub(1)),
            PageUp => self.files.sel = self.files.sel.saturating_sub(page),
            PageDown => self.files.sel = (self.files.sel + page).min(n.saturating_sub(1)),
            Home => self.files.sel = 0,
            End => self.files.sel = n.saturating_sub(1),
            Enter | Right => self.files_open_selected(),
            Backspace | Left => {
                self.files.follow_r = false;
                self.files.up();
            }
            Char('.') => {
                self.files.show_hidden = !self.files.show_hidden;
                self.files.refresh();
            }
            Char('r') => self.files.refresh(),
            Char('~') => {
                self.files.follow_r = false;
                self.files.cd(files::home());
            }
            Char('g') => {
                if let Some(wd) = self.r_wd.clone() {
                    self.files.follow_r = true;
                    self.files.cd(wd);
                }
            }
            Char('w') => {
                let d = self.files.dir.clone();
                self.files.follow_r = true;
                self.send_code(&format!("setwd({})", r_string(&d.to_string_lossy())));
            }
            Char('n') => {
                let init = {
                    let mut s = self.files.dir.to_string_lossy().to_string();
                    if !s.ends_with('/') {
                        s.push('/');
                    }
                    s
                };
                self.open_prompt(PromptKind::NewFile, "New file", &init);
            }
            _ => {}
        }
    }

    fn files_open_selected(&mut self) {
        if self.files.sel == 0 {
            self.files.follow_r = false;
            self.files.up();
            return;
        }
        let Some(e) = self.files.selected().cloned() else { return };
        if e.is_dir {
            self.files.follow_r = false;
            self.files.cd(e.path);
        } else if files::is_image_ext(&e.path) {
            self.plots.push(e.path);
            self.right_tab = RightTab::Plots;
        } else if files::is_text_ext(&e.path) || e.size < 2_000_000 {
            self.open_path(&e.path);
        } else {
            self.msg_err("file too large to open in the editor");
        }
    }

    fn key_plots(&mut self, k: Key) {
        use KeyCode::*;
        match k.code {
            Left | Char('p') => self.do_action(Action::PlotPrev),
            Right | Char('n') => self.do_action(Action::PlotNext),
            Home => self.plots.idx = 0,
            End => self.plots.idx = self.plots.history.len().saturating_sub(1),
            Char('m') => self.do_action(Action::PlotMode),
            Char('i') => self.do_action(Action::PlotImage),
            Char('r') => self.do_action(Action::PlotRedraw),
            Char('s') => self.do_action(Action::PlotSave),
            _ => {}
        }
        self.need_full_redraw = true;
    }

    fn key_viewer(&mut self, k: Key) {
        use KeyCode::*;
        let Some(d) = &self.viewer.data else { return };
        let nr = d.rows.len();
        let nc = d.headers.len().saturating_sub(1);
        let page = (self.rects.right_body.height as usize).saturating_sub(3).max(1);
        match k.code {
            Up => self.viewer.row = self.viewer.row.saturating_sub(1),
            Down => self.viewer.row = (self.viewer.row + 1).min(nr.saturating_sub(1)),
            PageUp => self.viewer.row = self.viewer.row.saturating_sub(page),
            PageDown => self.viewer.row = (self.viewer.row + page).min(nr.saturating_sub(1)),
            Home => {
                self.viewer.row = 0;
                self.viewer.col = 0;
            }
            End => self.viewer.row = nr.saturating_sub(1),
            Left => self.viewer.col = self.viewer.col.saturating_sub(1),
            Right => self.viewer.col = (self.viewer.col + 1).min(nc.saturating_sub(1)),
            _ => {}
        }
    }

    fn key_help(&mut self, ev: KeyEvent, k: Key) {
        use KeyCode::*;
        let searching = !self.help.query.is_empty();
        let page_h = (self.rects.right_body.height as usize).saturating_sub(2).max(1);
        let max_row = self.help.page.as_ref().map(|p| p.lines.len()).unwrap_or(0).saturating_sub(page_h);
        let nres = self.help.results.len();
        match (k.ctrl, k.alt, k.code) {
            (false, true, Left) => {
                self.help.go_back();
            }
            (false, true, Right) => {
                self.help.go_forward();
            }
            (_, _, Esc) => {
                self.help.query.clear();
                self.help.results.clear();
            }
            (_, _, Enter) if searching => self.help_open_selected(),
            (_, _, Backspace) => {
                if searching {
                    self.help.query.pop();
                    self.help.update_results();
                } else {
                    self.help.go_back();
                }
            }
            (true, false, Char('u')) => {
                self.help.query.clear();
                self.help.results.clear();
            }
            (_, _, Up) if searching => self.help.sel = self.help.sel.saturating_sub(1),
            (_, _, Down) if searching => self.help.sel = (self.help.sel + 1).min(nres.saturating_sub(1)),
            (_, _, PageUp) if searching => self.help.sel = self.help.sel.saturating_sub(page_h),
            (_, _, PageDown) if searching => self.help.sel = (self.help.sel + page_h).min(nres.saturating_sub(1)),
            (_, _, Up) => self.help.row = self.help.row.saturating_sub(1),
            (_, _, Down) => self.help.row = (self.help.row + 1).min(max_row),
            (_, _, PageUp) => self.help.row = self.help.row.saturating_sub(page_h.saturating_sub(1).max(1)),
            (_, _, PageDown) => self.help.row = (self.help.row + page_h.saturating_sub(1).max(1)).min(max_row),
            (_, _, Char(' ')) if !searching => self.help.row = (self.help.row + page_h.saturating_sub(1).max(1)).min(max_row),
            (_, _, Home) if !searching => {
                self.help.row = 0;
                self.help.col = 0;
            }
            (_, _, End) if !searching => self.help.row = max_row,
            (_, _, Left) if !searching => self.help.col = self.help.col.saturating_sub(8),
            (_, _, Right) if !searching => self.help.col = (self.help.col + 8).min(200),
            (false, false, Char(_)) => {
                if let KeyCode::Char(c) = ev.code {
                    self.help.query.push(c);
                    self.help.update_results();
                    if self.help.index.is_none() {
                        self.msg("help index is still loading (it is built once R has started)");
                    }
                }
            }
            _ => {}
        }
    }

    fn help_open_selected(&mut self) {
        let Some(t) = self.help.selected_topic() else {
            let q = self.help.query.clone();
            self.msg_err(&format!("no help topic matches '{q}'"));
            return;
        };
        self.help.query.clear();
        self.help.results.clear();
        self.help.loading = Some(format!("{}::{}", t.pkg, t.alias));
        self.help_render_n += 1;
        let out = self.session.dir.join(format!("help-render-{}.txt", self.help_render_n));
        let width = self.rects.right_body.width.saturating_sub(2);
        help::render(self.cfg.rscript.clone(), t, width, out, self.tx.clone());
    }

    fn key_modal(&mut self, ev: KeyEvent, k: Key) {
        use KeyCode::*;
        let modal = std::mem::replace(&mut self.modal, Modal::None);
        match modal {
            Modal::None => {}
            Modal::Menu { mut menu, mut item } => {
                let items = MENUS[menu].1;
                match k.code {
                    Esc | F(10) => {}
                    Left => {
                        menu = (menu + MENUS.len() - 1) % MENUS.len();
                        self.modal = Modal::Menu { menu, item: 0 };
                    }
                    Right => {
                        menu = (menu + 1) % MENUS.len();
                        self.modal = Modal::Menu { menu, item: 0 };
                    }
                    Up => {
                        item = (item + items.len() - 1) % items.len();
                        self.modal = Modal::Menu { menu, item };
                    }
                    Down => {
                        item = (item + 1) % items.len();
                        self.modal = Modal::Menu { menu, item };
                    }
                    Enter => {
                        let a = items[item.min(items.len() - 1)].action;
                        self.do_action(a);
                    }
                    _ => self.modal = Modal::Menu { menu, item },
                }
            }
            Modal::Confirm(kind, text) => match (kind, k.code) {
                (_, Esc) => {}
                (ConfirmKind::Quit, Char('y')) => {
                    for i in 0..self.docs.len() {
                        if self.docs[i].modified && self.docs[i].path.is_some() {
                            self.save_doc(i);
                        }
                    }
                    if self.docs.iter().any(|d| d.modified) {
                        self.msg_err("some files could not be saved (untitled?) - save them or press n");
                    } else {
                        self.do_quit();
                    }
                }
                (ConfirmKind::Quit, Char('n')) => {
                    self.autosave();
                    self.save_session();
                    self.quit = true;
                }
                (ConfirmKind::CloseTab(id), Char('y')) => {
                    if let Some(i) = self.docs.iter().position(|d| d.id == id) {
                        self.active = i;
                        if self.save_doc(i) {
                            self.close_tab(true);
                        }
                    }
                }
                (ConfirmKind::CloseTab(id), Char('n')) => {
                    if let Some(i) = self.docs.iter().position(|d| d.id == id) {
                        self.active = i;
                        self.close_tab(true);
                    }
                }
                _ => self.modal = Modal::Confirm(kind, text),
            },
            Modal::Prompt(mut p) => {
                match (k.ctrl, k.code) {
                    (_, Esc) => {
                        if p.kind == PromptKind::Find {
                            self.last_search = p.input.iter().collect();
                        }
                        return;
                    }
                    (_, Enter) => {
                        self.submit_prompt(p, k.shift);
                        return;
                    }
                    (false, Up) if p.kind == PromptKind::Find => {
                        self.last_search = p.input.iter().collect();
                        self.find_next(true);
                    }
                    (false, Down) if p.kind == PromptKind::Find => {
                        self.last_search = p.input.iter().collect();
                        self.find_next(false);
                    }
                    (_, Left) => p.cursor = p.cursor.saturating_sub(1),
                    (_, Right) => p.cursor = (p.cursor + 1).min(p.input.len()),
                    (_, Home) => p.cursor = 0,
                    (_, End) => p.cursor = p.input.len(),
                    (true, Char('a')) => p.cursor = 0,
                    (true, Char('e')) => p.cursor = p.input.len(),
                    (true, Char('u')) => {
                        p.input.drain(..p.cursor);
                        p.cursor = 0;
                    }
                    (_, Backspace) => {
                        if p.cursor > 0 {
                            p.cursor -= 1;
                            p.input.remove(p.cursor);
                        }
                    }
                    (_, Delete) => {
                        if p.cursor < p.input.len() {
                            p.input.remove(p.cursor);
                        }
                    }
                    (_, Tab) if matches!(p.kind, PromptKind::SaveAs | PromptKind::SavePlot | PromptKind::NewFile) => {
                        complete_path(&mut p, &self.files.dir);
                    }
                    (false, Char(_)) => {
                        if let KeyCode::Char(c) = ev.code {
                            p.input.insert(p.cursor, c);
                            p.cursor += 1;
                        }
                    }
                    _ => {}
                }
                // incremental search
                if p.kind == PromptKind::Find && !matches!(k.code, Up | Down) {
                    let needle: String = p.input.iter().collect();
                    let origin = self.find_origin;
                    let d = self.doc_mut();
                    if needle.is_empty() {
                        d.anchor = None;
                    } else if let Some(hit) = d.find_from(&needle, origin, false, true) {
                        d.anchor = Some(hit);
                        d.cur = Pos::new(hit.line, hit.col + needle.chars().count());
                    }
                    self.scroll_cursor_into_view();
                }
                self.modal = Modal::Prompt(p);
            }
            Modal::Open(mut d) => {
                let vis_len = d.visible().len();
                match k.code {
                    Esc => return,
                    Up => d.sel = d.sel.saturating_sub(1),
                    Down => d.sel = (d.sel + 1).min(vis_len.saturating_sub(1)),
                    PageUp => d.sel = d.sel.saturating_sub(10),
                    PageDown => d.sel = (d.sel + 10).min(vis_len.saturating_sub(1)),
                    Enter => {
                        let chosen = d.visible().get(d.sel).map(|e| (*e).clone());
                        match chosen {
                            Some(e) if e.is_dir => Self::load_open_dir(&mut d, e.path),
                            Some(e) => {
                                self.open_path(&e.path);
                                return;
                            }
                            None => {
                                if !d.filter.is_empty() {
                                    let p = files::expand(&d.filter, &d.dir);
                                    if p.is_dir() {
                                        Self::load_open_dir(&mut d, p);
                                    } else {
                                        self.open_path(&p);
                                        return;
                                    }
                                }
                            }
                        }
                    }
                    Backspace => {
                        if d.filter.is_empty() {
                            if let Some(parent) = d.dir.parent().map(|p| p.to_path_buf()) {
                                let old = d.dir.clone();
                                Self::load_open_dir(&mut d, parent);
                                if let Some(i) = d.visible().iter().position(|e| e.path == old) {
                                    d.sel = i;
                                }
                            }
                        } else {
                            d.filter.pop();
                            d.sel = 0;
                        }
                    }
                    Char(_) => {
                        if let KeyCode::Char(c) = ev.code {
                            if c == '/' && d.filter.is_empty() {
                                Self::load_open_dir(&mut d, PathBuf::from("/"));
                            } else if c == '~' && d.filter.is_empty() {
                                Self::load_open_dir(&mut d, files::home());
                            } else if c == '/' {
                                let p = files::expand(&d.filter, &d.dir);
                                if p.is_dir() {
                                    Self::load_open_dir(&mut d, p);
                                } else {
                                    d.filter.push(c);
                                }
                            } else {
                                d.filter.push(c);
                                d.sel = 0;
                            }
                        }
                    }
                    _ => {}
                }
                self.modal = Modal::Open(d);
            }
        }
    }

    fn submit_prompt(&mut self, p: Prompt, shift: bool) {
        let input: String = p.input.iter().collect();
        match p.kind {
            PromptKind::SaveAs => {
                if input.trim().is_empty() || input.ends_with('/') {
                    self.msg_err("enter a file name");
                    self.modal = Modal::Prompt(p);
                    return;
                }
                let path = files::expand(&input, &self.files.dir);
                if let Some(parent) = path.parent() {
                    if !parent.exists() {
                        self.msg_err(&format!("folder {} does not exist", parent.display()));
                        self.modal = Modal::Prompt(p);
                        return;
                    }
                }
                self.remove_recovery(self.active);
                self.doc_mut().path = Some(path);
                self.save_doc(self.active);
                self.files.refresh();
            }
            PromptKind::NewFile => {
                let path = files::expand(&input, &self.files.dir);
                if path.exists() {
                    self.open_path(&path);
                } else {
                    match std::fs::write(&path, "") {
                        Ok(_) => {
                            self.files.refresh();
                            self.open_path(&path);
                        }
                        Err(e) => self.msg_err(&format!("cannot create file: {e}")),
                    }
                }
            }
            PromptKind::Goto => {
                if let Ok(n) = input.trim().parse::<usize>() {
                    let d = self.doc_mut();
                    d.move_to(Pos::new(n.saturating_sub(1), 0), false);
                    let (h, _) = self.source_view_size();
                    let d = self.doc_mut();
                    d.top = d.cur.line.saturating_sub(h / 3);
                    self.scroll_cursor_into_view();
                }
            }
            PromptKind::Find => {
                self.last_search = input;
                self.find_next(shift);
                self.modal = Modal::Prompt(p);
            }
            PromptKind::ReplaceFind => {
                if !input.is_empty() {
                    self.last_search = input.clone();
                    self.open_prompt(PromptKind::ReplaceWith(input.clone()), &format!("Replace all '{input}' with"), "");
                }
            }
            PromptKind::ReplaceWith(find) => {
                let n = self.doc_mut().replace_all(&find, &input);
                self.msg(&format!("replaced {n} occurrence(s)"));
            }
            PromptKind::SavePlot => {
                let dest = files::expand(&input, &self.files.dir);
                if let Some(src) = self.plots.current().cloned() {
                    match std::fs::copy(&src, &dest) {
                        Ok(_) => {
                            self.msg(&format!("plot saved to {}", dest.display()));
                            self.files.refresh();
                        }
                        Err(e) => self.msg_err(&format!("cannot save plot: {e}")),
                    }
                }
            }
        }
    }

    // ---------- mouse ----------

    fn handle_mouse(&mut self, m: MouseEvent) {
        let (x, y) = (m.column, m.row);
        let hit = |r: Rect| x >= r.x && x < r.right() && y >= r.y && y < r.bottom();
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // click counting for double/triple clicks
                let now = Instant::now();
                let (t, lx, ly, n) = self.last_click;
                let count = if now.duration_since(t) < Duration::from_millis(400) && lx == x && ly == y { (n % 3) + 1 } else { 1 };
                self.last_click = (now, x, y, count);
                self.mouse_down(x, y, count, hit, m.modifiers);
            }
            MouseEventKind::Drag(MouseButton::Left) => self.mouse_drag(x, y),
            MouseEventKind::Up(_) => {
                if self.drag != Drag::None && self.drag != Drag::Select {
                    self.need_full_redraw = true;
                }
                self.drag = Drag::None;
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = matches!(m.kind, MouseEventKind::ScrollUp);
                self.mouse_scroll(x, y, up);
            }
            _ => {}
        }
    }

    fn mouse_scroll(&mut self, x: u16, y: u16, up: bool) {
        let hit = |r: Rect| x >= r.x && x < r.right() && y >= r.y && y < r.bottom();
        let d: isize = if up { -3 } else { 3 };
        if let Modal::Open(dlg) = &mut self.modal {
            let n = dlg.visible().len();
            dlg.sel = (dlg.sel as isize + d).clamp(0, n.saturating_sub(1) as isize) as usize;
            return;
        }
        if !matches!(self.modal, Modal::None) {
            return;
        }
        let r = self.rects.clone();
        if hit(r.source) {
            if hit(r.outline) {
                return;
            }
            let doc = self.doc_mut();
            let max = doc.lines.len().saturating_sub(1) as isize;
            doc.top = (doc.top as isize + d).clamp(0, max) as usize;
        } else if hit(r.console) {
            if let Some(c) = self.console.as_mut() {
                if c.alternate_screen() {
                    let key: &[u8] = if up { b"\x1b[A\x1b[A\x1b[A" } else { b"\x1b[B\x1b[B\x1b[B" };
                    c.write(key);
                } else {
                    c.scroll_by(-d);
                }
            }
        } else if hit(r.env) {
            if hit(r.env_preview) {
                self.env.preview_scroll = (self.env.preview_scroll as isize + d).max(0) as usize;
            } else {
                let n = self.env.rows().len();
                let max = n.saturating_sub(r.env_list.height as usize);
                self.env.scroll = ((self.env.scroll as isize + d).max(0) as usize).min(max);
            }
        } else if hit(r.right) {
            match self.right_tab {
                RightTab::Files => {
                    let n = self.files.len();
                    let max = n.saturating_sub(r.right_body.height as usize);
                    self.files.scroll = ((self.files.scroll as isize + d).max(0) as usize).min(max);
                }
                RightTab::Viewer => {
                    if let Some(v) = &self.viewer.data {
                        let n = v.rows.len();
                        self.viewer.row = ((self.viewer.row as isize + d).max(0) as usize).min(n.saturating_sub(1));
                    }
                }
                RightTab::Help => {
                    if !self.help.query.is_empty() {
                        let n = self.help.results.len();
                        self.help.sel = ((self.help.sel as isize + d).max(0) as usize).min(n.saturating_sub(1));
                    } else {
                        let n = self.help.page.as_ref().map(|h| h.lines.len()).unwrap_or(0);
                        let max = n.saturating_sub(r.right_body.height.saturating_sub(2) as usize);
                        self.help.row = ((self.help.row as isize + d).max(0) as usize).min(max);
                    }
                }
                RightTab::Plots => {}
            }
        }
    }

    fn mouse_down(&mut self, x: u16, y: u16, count: u8, hit: impl Fn(Rect) -> bool, mods: KeyModifiers) {
        let r = self.rects.clone();
        // menus
        if let Modal::Menu { menu, .. } = self.modal {
            if hit(r.menu_popup) {
                let idx = (y - r.menu_popup.y) as isize - 1;
                let items = MENUS[menu].1;
                if idx >= 0 && (idx as usize) < items.len() {
                    self.modal = Modal::None;
                    self.do_action(items[idx as usize].action);
                }
                return;
            }
            for (rect, i) in &r.menu_titles {
                if hit(*rect) {
                    self.modal = if *i == menu { Modal::None } else { Modal::Menu { menu: *i, item: 0 } };
                    return;
                }
            }
            self.modal = Modal::None;
            return;
        }
        if let Modal::Open(dlg) = &mut self.modal {
            if hit(r.open_list) {
                let idx = dlg.scroll + (y - r.open_list.y) as usize;
                if idx < dlg.visible().len() {
                    dlg.sel = idx;
                    if count >= 2 {
                        self.key_modal(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), Key { code: KeyCode::Enter, ctrl: false, alt: false, shift: false });
                    }
                }
            }
            return;
        }
        if !matches!(self.modal, Modal::None) {
            return;
        }
        if hit(r.menubar) {
            for (rect, i) in &r.menu_titles {
                if hit(*rect) {
                    self.modal = Modal::Menu { menu: *i, item: 0 };
                    return;
                }
            }
            return;
        }
        // splitters
        if hit(r.vsep) {
            self.drag = Drag::VSep;
            return;
        }
        if r.console.height > 0 && y == r.console.y && x >= r.console.x && x < r.console.right() && self.zoom.is_none() {
            if !self.title_click(x, y) {
                self.drag = Drag::LSplit;
            }
            self.focus = Pane::Console;
            return;
        }
        if r.right.height > 0 && y == r.right.y && x >= r.right.x && x < r.right.right() && self.zoom.is_none() {
            if !self.title_click(x, y) {
                self.drag = Drag::RSplit;
            }
            self.focus = Pane::Right;
            return;
        }
        if hit(r.source) {
            self.focus = Pane::Source;
            if y == r.source.y {
                self.title_click(x, y);
                return;
            }
            if hit(r.outline) {
                let items = self.doc_mut().outline();
                let idx = (y - r.outline.y) as usize;
                let off = self.outline_offset(items.len(), r.outline.height as usize);
                if let Some(it) = items.get(idx + off) {
                    let line = it.line;
                    let (h, _) = self.source_view_size();
                    let d = self.doc_mut();
                    d.move_to(Pos::new(line, 0), false);
                    d.top = line.saturating_sub(h / 4);
                }
                return;
            }
            if hit(r.source_text) {
                let p = self.editor_pos_at(x, y);
                let d = self.doc_mut();
                match count {
                    2 => {
                        let (a, b) = d.word_at(p);
                        d.anchor = Some(a);
                        d.cur = b;
                    }
                    3 => {
                        d.anchor = Some(Pos::new(p.line, 0));
                        d.cur = if p.line + 1 < d.lines.len() { Pos::new(p.line + 1, 0) } else { Pos::new(p.line, d.line_len(p.line)) };
                    }
                    _ => {
                        d.move_to(p, mods.contains(KeyModifiers::SHIFT));
                        self.drag = Drag::Select;
                    }
                }
            }
            return;
        }
        if hit(r.console) {
            self.focus = Pane::Console;
            return;
        }
        if hit(r.env) {
            self.focus = Pane::Env;
            if hit(r.env_list) {
                let idx = self.env.scroll + (y - r.env_list.y) as usize;
                let rows = self.env.rows();
                if idx < rows.len() {
                    let toggle = self.env.sel == idx || x <= r.env_list.x + 1 || count >= 2;
                    self.env.sel = idx;
                    self.env.preview_scroll = 0;
                    if toggle {
                        if let EnvRow::Obj(i) = rows[idx] {
                            let name = self.env.snap.objects[i].name.clone();
                            if !self.env.expanded.remove(&name) {
                                self.env.expanded.insert(name);
                            }
                        }
                    }
                }
            }
            return;
        }
        if hit(r.right) {
            self.focus = Pane::Right;
            if hit(r.right_body) && self.right_tab == RightTab::Help && !self.help.query.is_empty() {
                // row 0 is the search box, results follow
                if y > r.right_body.y {
                    let idx = self.help.results_scroll + (y - r.right_body.y - 1) as usize;
                    if idx < self.help.results.len() {
                        self.help.sel = idx;
                        self.help_open_selected();
                    }
                }
                return;
            }
            if hit(r.right_body) && self.right_tab == RightTab::Files {
                // row 0 of the body is the path header
                if y > r.right_body.y {
                    let idx = self.files.scroll + (y - r.right_body.y - 1) as usize;
                    if idx < self.files.len() {
                        let again = self.files.sel == idx;
                        self.files.sel = idx;
                        if count >= 2 || again {
                            self.files_open_selected();
                        }
                    }
                }
            }
        }
    }

    pub fn outline_offset(&mut self, n: usize, h: usize) -> usize {
        // keep the section containing the cursor visible
        let cur = self.doc().cur.line;
        let items = self.doc_mut().outline();
        let idx = items.iter().rposition(|it| it.line <= cur).unwrap_or(0);
        if n <= h || h == 0 {
            0
        } else {
            idx.saturating_sub(h / 2).min(n - h)
        }
    }

    /// Clicks on a pane title row (tabs). Returns true when a tab was hit.
    fn title_click(&mut self, x: u16, y: u16) -> bool {
        let hit = |r: &Rect| x >= r.x && x < r.right() && y >= r.y && y < r.bottom();
        for (r, i) in self.rects.source_tabs.clone() {
            if hit(&r) {
                self.active = i;
                self.focus = Pane::Source;
                return true;
            }
        }
        for (r, t) in self.rects.right_tabs.clone() {
            if hit(&r) {
                self.focus_pane(Pane::Right, Some(t));
                return true;
            }
        }
        false
    }

    fn editor_pos_at(&mut self, x: u16, y: u16) -> Pos {
        let r = self.rects.source_text;
        let gw = self.rects.gutter_w;
        let d = self.doc();
        let line = (d.top + y.saturating_sub(r.y) as usize).min(d.lines.len().saturating_sub(1));
        let dx = x.saturating_sub(r.x + gw) as usize + d.left;
        Pos::new(line, d.col_at_disp(line, dx))
    }

    fn mouse_drag(&mut self, x: u16, y: u16) {
        let b = self.rects.body;
        match self.drag {
            Drag::VSep => {
                if b.width > 20 {
                    self.vsplit = ((x.saturating_sub(b.x)) as f32 / b.width as f32).clamp(0.15, 0.85);
                }
            }
            Drag::LSplit => {
                if b.height > 6 {
                    self.lsplit = ((y.saturating_sub(b.y)) as f32 / b.height as f32).clamp(0.1, 0.9);
                }
            }
            Drag::RSplit => {
                if b.height > 6 {
                    self.rsplit = ((y.saturating_sub(b.y)) as f32 / b.height as f32).clamp(0.1, 0.9);
                }
            }
            Drag::Select => {
                let r = self.rects.source_text;
                let (h, _) = self.source_view_size();
                {
                    let d = self.doc_mut();
                    if y < r.y {
                        d.top = d.top.saturating_sub(1);
                    } else if y >= r.bottom() && d.top + h < d.lines.len() {
                        d.top += 1;
                    }
                }
                let yy = y.clamp(r.y, r.bottom().saturating_sub(1));
                let p = self.editor_pos_at(x, yy);
                let d = self.doc_mut();
                if d.anchor.is_none() {
                    d.anchor = Some(d.cur);
                }
                d.cur = p;
            }
            Drag::None => {}
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Key {
    pub code: KeyCode,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// Normalize a key event for shortcut matching: lowercase letters, shifted
/// digits (US layout) mapped back to digits, shift tracked as a flag.
pub fn norm(k: &KeyEvent) -> Key {
    let m = k.modifiers;
    let ctrl = m.contains(KeyModifiers::CONTROL);
    let alt = m.contains(KeyModifiers::ALT);
    let mut shift = m.contains(KeyModifiers::SHIFT);
    let code = match k.code {
        KeyCode::Char(c) => {
            if c.is_ascii_uppercase() {
                shift = true;
                KeyCode::Char(c.to_ascii_lowercase())
            } else if ctrl || alt {
                let base = match c {
                    '!' => Some('1'),
                    '@' => Some('2'),
                    '#' => Some('3'),
                    '$' => Some('4'),
                    '%' => Some('5'),
                    '^' => Some('6'),
                    '&' => Some('7'),
                    '*' => Some('8'),
                    '(' => Some('9'),
                    ')' => Some('0'),
                    _ => None,
                };
                match base {
                    Some(b) => {
                        shift = true;
                        KeyCode::Char(b)
                    }
                    None => KeyCode::Char(c),
                }
            } else {
                KeyCode::Char(c)
            }
        }
        other => other,
    };
    Key { code, ctrl, alt, shift }
}

fn complete_path(p: &mut Prompt, base: &Path) {
    let input: String = p.input.iter().collect();
    let (dir_part, prefix) = match input.rfind('/') {
        Some(i) => (&input[..=i], &input[i + 1..]),
        None => ("", input.as_str()),
    };
    let dir = if dir_part.is_empty() { base.to_path_buf() } else { files::expand(dir_part, base) };
    let Ok(entries) = files::list_dir(&dir, prefix.starts_with('.')) else { return };
    let matches: Vec<&Entry> = entries.iter().filter(|e| e.name.starts_with(prefix)).collect();
    if matches.is_empty() {
        return;
    }
    // longest common prefix
    let mut common = matches[0].name.clone();
    for m in &matches[1..] {
        let n = common.chars().zip(m.name.chars()).take_while(|(a, b)| a == b).count();
        common = common.chars().take(n).collect();
    }
    let mut out = format!("{dir_part}{common}");
    if matches.len() == 1 && matches[0].is_dir {
        out.push('/');
    }
    p.input = out.chars().collect();
    p.cursor = p.input.len();
}
