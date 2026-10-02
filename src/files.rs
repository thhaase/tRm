//! Directory listing used by the Files pane and the Open dialog.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
}

pub fn list_dir(dir: &Path, show_hidden: bool) -> Result<Vec<Entry>, String> {
    let rd = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for e in rd.flatten().take(20_000) {
        let name = e.file_name().to_string_lossy().to_string();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let path = e.path();
        // follow symlinks for the dir check, fall back gracefully
        let meta = std::fs::metadata(&path).ok();
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        out.push(Entry { name, path, is_dir, size });
    }
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(out)
}

pub fn home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"))
}

/// Expand a leading `~` and make relative paths absolute against `base`.
pub fn expand(input: &str, base: &Path) -> PathBuf {
    let s = input.trim();
    let p = if s == "~" {
        home()
    } else if let Some(rest) = s.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(s)
    };
    if p.is_absolute() {
        p
    } else {
        base.join(p)
    }
}

pub fn is_text_ext(p: &Path) -> bool {
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(
        ext.as_str(),
        "r" | "rmd" | "qmd" | "rprofile" | "renviron" | "txt" | "md" | "csv" | "tsv" | "json" | "yaml" | "yml"
            | "toml" | "sh" | "py" | "sql" | "stan" | "c" | "cpp" | "h" | "tex" | "bib" | "html" | "css" | "js"
            | "log" | "ini" | "cfg" | "conf" | "xml" | "rs" | "lintr" | "dcf" | ""
    ) || p.file_name().map(|n| {
        let n = n.to_string_lossy();
        n == "DESCRIPTION" || n == "NAMESPACE" || n.starts_with('.')
    }).unwrap_or(false)
}

pub fn is_image_ext(p: &Path) -> bool {
    p.extension().map(|e| e.to_string_lossy().eq_ignore_ascii_case("png")).unwrap_or(false)
}

pub struct FilesView {
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    pub sel: usize,
    pub scroll: usize,
    pub show_hidden: bool,
    pub error: Option<String>,
    pub follow_r: bool,
}

impl FilesView {
    pub fn new(dir: PathBuf) -> FilesView {
        let mut f = FilesView { dir, entries: Vec::new(), sel: 0, scroll: 0, show_hidden: false, error: None, follow_r: true };
        f.refresh();
        f
    }

    pub fn refresh(&mut self) {
        match list_dir(&self.dir, self.show_hidden) {
            Ok(e) => {
                self.entries = e;
                self.error = None;
            }
            Err(e) => {
                self.entries.clear();
                self.error = Some(e);
            }
        }
        if self.sel >= self.entries.len() + 1 {
            self.sel = 0;
        }
    }

    pub fn cd(&mut self, dir: PathBuf) {
        let old = self.dir.clone();
        self.dir = dir;
        self.sel = 0;
        self.scroll = 0;
        self.refresh();
        // select the directory we came from when going up
        if let Some(i) = self.entries.iter().position(|e| e.path == old) {
            self.sel = i + 1;
        }
    }

    pub fn up(&mut self) {
        if let Some(p) = self.dir.parent() {
            let p = p.to_path_buf();
            self.cd(p);
        }
    }

    /// Row 0 is the ".." entry.
    pub fn len(&self) -> usize {
        self.entries.len() + 1
    }

    pub fn selected(&self) -> Option<&Entry> {
        if self.sel == 0 {
            None
        } else {
            self.entries.get(self.sel - 1)
        }
    }
}
