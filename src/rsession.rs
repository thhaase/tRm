//! Side channel between tRm and the R process: a private temp directory
//! where the R helper writes environment snapshots, plots and View() data.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const INIT_R: &str = include_str!("trm_init.R");

#[derive(Clone, Debug, Default)]
pub struct EnvChild {
    pub name: String,
    pub class: String,
    pub size: f64,
    pub dims: String,
    pub summary: String,
}

#[derive(Clone, Debug, Default)]
pub struct EnvObj {
    pub name: String,
    pub class: String,
    pub size: f64,
    pub dims: String,
    pub summary: String,
    pub children: Vec<EnvChild>,
    pub preview: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct EnvSnapshot {
    pub objects: Vec<EnvObj>,
    pub total: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ViewData {
    pub title: String,
    pub nrow: usize,
    pub ncol: usize,
    pub headers: Vec<String>,
    pub types: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Clone, Debug, Default)]
pub struct HelpPage {
    pub title: String,
    /// text and whether the line is a heading (bold in R's text help)
    pub lines: Vec<(String, bool)>,
}

pub fn is_heading(l: &str) -> bool {
    let t = l.trim_end();
    !t.is_empty() && !t.starts_with(' ') && (t.ends_with(':') || t.chars().all(|c| !c.is_lowercase()))
}

impl HelpPage {
    pub fn from_text(title: &str, lines: &[&str]) -> HelpPage {
        HelpPage {
            title: title.to_string(),
            lines: lines.iter().enumerate().map(|(i, l)| (l.to_string(), i == 0 || is_heading(l))).collect(),
        }
    }
}

pub enum Update {
    Env(EnvSnapshot),
    Plot(PathBuf),
    Wd(PathBuf),
    Libs(Vec<PathBuf>),
    Packages(Vec<String>),
    View(ViewData),
    Help(HelpPage),
}

pub struct Session {
    pub dir: PathBuf,
    pub init: PathBuf,
    mtimes: [Option<SystemTime>; 5],
    view_width: u16,
    last_plot: u32,
    text_size: (u16, u16),
}

const FILES: [&str; 5] = ["env", "plot", "state", "view", "help"];

impl Session {
    pub fn new() -> Result<Session, String> {
        let base = std::env::temp_dir();
        cleanup_stale(&base);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let dir = base.join(format!("trm-{}-{:x}", std::process::id(), nanos));
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        let init = dir.join("init.R");
        std::fs::write(&init, INIT_R).map_err(|e| e.to_string())?;
        Ok(Session { dir, init, mtimes: [None; 5], view_width: 0, last_plot: 0, text_size: (0, 0) })
    }

    /// Clear old outputs (used when R restarts).
    pub fn reset(&mut self) {
        for f in FILES {
            let _ = std::fs::remove_file(self.dir.join(f));
        }
        self.mtimes = [None; 5];
        self.view_width = 0;
        self.text_size = (0, 0);
    }

    /// plot_NNNN.png files newer than the last one reported, in order.
    fn new_plots(&self) -> Vec<(u32, PathBuf)> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut v: Vec<(u32, PathBuf)> = rd
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                let n = name.strip_prefix("plot_")?.strip_suffix(".png")?.parse::<u32>().ok()?;
                (n > self.last_plot).then(|| (n, e.path()))
            })
            .collect();
        v.sort();
        v
    }

    pub fn write_view_width_if_changed(&mut self, w: u16) {
        if w >= 30 && self.view_width != w {
            self.view_width = w;
            let _ = std::fs::write(self.dir.join("viewwidth"), format!("{w}\n"));
        }
    }

    pub fn write_text_size_if_changed(&mut self, cols: u16, rows: u16) {
        if (cols, rows) != self.text_size && cols >= 10 && rows >= 5 {
            self.text_size = (cols, rows);
            let _ = std::fs::write(self.dir.join("textsize"), format!("{cols} {rows}\n"));
        }
    }

    pub fn write_plot_size(&self, w: u32, h: u32) {
        let _ = std::fs::write(self.dir.join("plotsize"), format!("{w} {h}\n"));
    }

    pub fn poll(&mut self) -> Vec<Update> {
        let mut out = Vec::new();
        for (i, name) in FILES.iter().enumerate() {
            let path = self.dir.join(name);
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            let Ok(mt) = meta.modified() else { continue };
            if self.mtimes[i] == Some(mt) {
                continue;
            }
            self.mtimes[i] = Some(mt);
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let text = String::from_utf8_lossy(&bytes);
            match *name {
                "env" => out.push(Update::Env(parse_env(&text))),
                "plot" => {
                    // several plots can appear between two polls: pick up all new ones
                    for (n, p) in self.new_plots() {
                        self.last_plot = n;
                        out.push(Update::Plot(p));
                    }
                }
                "state" => {
                    let mut libs = Vec::new();
                    let mut pkgs = Vec::new();
                    for l in text.lines() {
                        if let Some(wd) = l.strip_prefix("wd\t") {
                            out.push(Update::Wd(PathBuf::from(wd)));
                        } else if let Some(lib) = l.strip_prefix("lib\t") {
                            libs.push(PathBuf::from(lib));
                        } else if let Some(p) = l.strip_prefix("pkg\t") {
                            pkgs.push(p.to_string());
                        }
                    }
                    if !libs.is_empty() {
                        out.push(Update::Libs(libs));
                    }
                    out.push(Update::Packages(pkgs));
                }
                "view" => out.push(Update::View(parse_view(&text))),
                "help" => out.push(Update::Help(parse_help(&text))),
                _ => {}
            }
        }
        out
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.dir.starts_with(std::env::temp_dir()) && self.dir.file_name().map(|n| n.to_string_lossy().starts_with("trm-")).unwrap_or(false) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// Remove session dirs left behind by tRm processes that were killed.
fn cleanup_stale(base: &Path) {
    let Ok(rd) = std::fs::read_dir(base) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(rest) = name.strip_prefix("trm-") else { continue };
        let Some(pid) = rest.split('-').next().and_then(|p| p.parse::<u32>().ok()) else { continue };
        if Path::new(&format!("/proc/{pid}")).exists() || !Path::new("/proc/self").exists() {
            continue;
        }
        let path = e.path();
        // only touch our own directories
        if path.join("init.R").exists() {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

fn num(s: &str) -> f64 {
    s.trim().parse::<f64>().unwrap_or(-1.0)
}

pub fn parse_env(text: &str) -> EnvSnapshot {
    let mut snap = EnvSnapshot::default();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        match f.first().copied() {
            Some("TOTAL") => snap.total = f.get(1).and_then(|s| s.trim().parse().ok()).unwrap_or(0),
            Some("OBJ") => snap.objects.push(EnvObj {
                name: f.get(1).unwrap_or(&"").to_string(),
                class: f.get(2).unwrap_or(&"").to_string(),
                size: num(f.get(3).unwrap_or(&"")),
                dims: f.get(4).unwrap_or(&"").to_string(),
                summary: f.get(5).unwrap_or(&"").to_string(),
                children: Vec::new(),
                preview: Vec::new(),
            }),
            Some("CHILD") => {
                if let Some(o) = snap.objects.last_mut() {
                    o.children.push(EnvChild {
                        name: f.get(1).unwrap_or(&"").to_string(),
                        class: f.get(2).unwrap_or(&"").to_string(),
                        size: num(f.get(3).unwrap_or(&"")),
                        dims: f.get(4).unwrap_or(&"").to_string(),
                        summary: f.get(5).unwrap_or(&"").to_string(),
                    });
                }
            }
            Some("PREV") => {
                if let Some(o) = snap.objects.last_mut() {
                    o.preview.push(line.get(5..).unwrap_or("").to_string());
                }
            }
            _ => {}
        }
    }
    snap
}

pub fn parse_view(text: &str) -> ViewData {
    let mut v = ViewData::default();
    for line in text.lines() {
        let mut f: Vec<String> = line.split('\t').map(|s| s.to_string()).collect();
        if f.is_empty() {
            continue;
        }
        let tag = f.remove(0);
        match tag.as_str() {
            "TITLE" => {
                v.title = f.first().cloned().unwrap_or_default();
                v.nrow = f.get(1).and_then(|s| s.trim().parse().ok()).unwrap_or(0);
                v.ncol = f.get(2).and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            }
            "HEAD" => v.headers = f,
            "TYPE" => v.types = f,
            "ROW" => v.rows.push(f),
            _ => {}
        }
    }
    v
}

/// Parse the pager output. R's text help uses backspace overstrikes
/// (`_\x08x` underline, `x\x08x` bold); we strip them and mark the line.
pub fn parse_help(text: &str) -> HelpPage {
    let mut page = HelpPage::default();
    for line in text.lines() {
        if let Some(t) = line.strip_prefix("TITLE\t") {
            page.title = t.to_string();
        } else if let Some(l) = line.strip_prefix("L\t") {
            let mut out: Vec<char> = Vec::with_capacity(l.len());
            let mut bold = false;
            let mut chars = l.chars();
            while let Some(c) = chars.next() {
                if c == '\x08' {
                    if let Some(next) = chars.next() {
                        out.pop();
                        out.push(next);
                        bold = true;
                    }
                } else {
                    out.push(c);
                }
            }
            let text: String = out.into_iter().collect();
            let first = page.lines.is_empty();
            let heading = bold || first || is_heading(&text);
            page.lines.push((text, heading));
        }
    }
    page
}

/// R's own lookup order for the user profile, so we can chain to it.
pub fn user_profile(cwd: &Path) -> Option<PathBuf> {
    if let Ok(p) = std::env::var("R_PROFILE_USER") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    let local = cwd.join(".Rprofile");
    if local.exists() {
        return Some(local);
    }
    let home = std::env::var("HOME").ok().map(PathBuf::from)?;
    let h = home.join(".Rprofile");
    if h.exists() {
        Some(h)
    } else {
        None
    }
}

/// Quote an R name with backticks when it is not syntactic.
pub fn r_name(name: &str) -> String {
    let syntactic = !name.is_empty()
        && name.chars().next().map(|c| c.is_alphabetic() || c == '.').unwrap_or(false)
        && name.chars().all(|c| c.is_alphanumeric() || c == '.' || c == '_')
        && !(name.starts_with('.') && name.chars().nth(1).map(|c| c.is_ascii_digit()).unwrap_or(false));
    if syntactic {
        name.to_string()
    } else {
        format!("`{}`", name.replace('`', "\\`"))
    }
}

pub fn r_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_parse() {
        let t = "TOTAL\t1\nOBJ\tdf\tdata.frame\t1208\t5 obs. of 2 vars\tx\nCHILD\ta\tinteger\t80\t[5]\t1 2\nPREV\t  a b\n";
        let s = parse_env(t);
        assert_eq!(s.objects.len(), 1);
        assert_eq!(s.objects[0].children.len(), 1);
        assert_eq!(s.objects[0].preview, vec!["  a b".to_string()]);
        assert_eq!(s.objects[0].size, 1208.0);
    }

    #[test]
    fn help_overstrike() {
        let p = parse_help("TITLE\tR Help on 'mean'\nL\t_\x08D_\x08e_\x08s:\nL\t  plain\n");
        assert_eq!(p.title, "R Help on 'mean'");
        assert_eq!(p.lines[0], ("Des:".to_string(), true));
        assert_eq!(p.lines[1], ("  plain".to_string(), false));
    }

    #[test]
    fn names() {
        assert_eq!(r_name("abc.d"), "abc.d");
        assert_eq!(r_name("my var"), "`my var`");
        assert_eq!(r_name("[[1]]"), "`[[1]]`");
    }
}
