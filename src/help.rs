//! The Help pane: a topic index built from every installed package's
//! `help/AnIndex`, instant search, pages rendered by a separate Rscript
//! (so the console stays clean and help works while R is busy), and history.

use crate::rsession::HelpPage;
use crate::AppEvent;
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

#[derive(Clone, Debug)]
pub struct Topic {
    pub alias: String,
    pub pkg: String,
    /// path without extension, as utils:::.getHelpFile wants it
    pub file: PathBuf,
}

/// Compact index: all strings live in one arena (93k topics from ~950
/// packages take a few MB instead of tens of MB).
#[derive(Default)]
pub struct HelpIndex {
    arena: String,
    pkgs: Vec<(String, PathBuf)>, // name, help dir
    entries: Vec<Entry>,
}

#[derive(Clone, Copy)]
struct Entry {
    alias: (u32, u16),
    file: (u32, u16),
    pkg: u16,
}

/// Packages shipped with R, ranked first like RStudio does.
const BASE_PKGS: &[&str] = &[
    "base", "utils", "stats", "graphics", "grDevices", "methods", "datasets", "tools", "grid", "parallel",
    "splines", "stats4", "tcltk", "compiler",
];

fn contains_ci(hay: &str, needle_lower: &str) -> bool {
    if needle_lower.len() > hay.len() {
        return false;
    }
    hay.as_bytes().windows(needle_lower.len()).any(|w| w.eq_ignore_ascii_case(needle_lower.as_bytes()))
}

impl HelpIndex {
    fn push_str(&mut self, s: &str) -> (u32, u16) {
        let s = &s[..s.len().min(u16::MAX as usize)];
        let start = self.arena.len() as u32;
        self.arena.push_str(s);
        (start, s.len() as u16)
    }

    fn add(&mut self, pkg: u16, alias: &str, file: &str) {
        let alias = self.push_str(alias);
        let file = self.push_str(file);
        self.entries.push(Entry { alias, file, pkg });
    }

    fn get(&self, r: (u32, u16)) -> &str {
        self.arena.get(r.0 as usize..r.0 as usize + r.1 as usize).unwrap_or("")
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn alias(&self, i: usize) -> &str {
        self.entries.get(i).map(|e| self.get(e.alias)).unwrap_or("")
    }

    pub fn pkg(&self, i: usize) -> &str {
        self.entries.get(i).and_then(|e| self.pkgs.get(e.pkg as usize)).map(|p| p.0.as_str()).unwrap_or("")
    }

    pub fn topic(&self, i: usize) -> Option<Topic> {
        let e = self.entries.get(i)?;
        let (pkg, dir) = self.pkgs.get(e.pkg as usize)?;
        Some(Topic { alias: self.get(e.alias).to_string(), pkg: pkg.clone(), file: dir.join(self.get(e.file)) })
    }

    pub fn build(libs: &[PathBuf]) -> HelpIndex {
        let mut idx = HelpIndex::default();
        let mut seen = HashSet::new();
        for lib in libs {
            let Ok(rd) = std::fs::read_dir(lib) else { continue };
            for e in rd.flatten() {
                let pkg = e.file_name().to_string_lossy().to_string();
                if seen.contains(&pkg) || idx.pkgs.len() >= u16::MAX as usize {
                    continue;
                }
                let help_dir = e.path().join("help");
                let Ok(text) = std::fs::read_to_string(help_dir.join("AnIndex")) else { continue };
                seen.insert(pkg.clone());
                let pi = idx.pkgs.len() as u16;
                idx.pkgs.push((pkg, help_dir));
                for line in text.lines() {
                    let mut it = line.splitn(2, '\t');
                    if let (Some(alias), Some(file)) = (it.next(), it.next()) {
                        idx.add(pi, alias, file.trim());
                    }
                }
            }
        }
        idx.arena.shrink_to_fit();
        idx.entries.shrink_to_fit();
        idx
    }

    /// Ranked matches: exact, then prefix, then substring. `pkg::topic` narrows
    /// to one package. Attached packages rank before other installed ones.
    pub fn search(&self, query: &str, attached: &[String], limit: usize) -> Vec<usize> {
        let q = query.trim();
        if q.is_empty() {
            return Vec::new();
        }
        let (pkg_filter, q) = match q.split_once("::") {
            Some((p, t)) => (Some(p), t.trim_start_matches(':')),
            None => (None, q),
        };
        let ql = q.to_ascii_lowercase();
        let pkg_rank: Vec<usize> = self
            .pkgs
            .iter()
            .map(|(name, _)| {
                if let Some(i) = BASE_PKGS.iter().position(|p| p == name) {
                    i
                } else if attached.iter().any(|a| a == name) {
                    50
                } else {
                    100
                }
            })
            .collect();
        let mut hits: Vec<(u8, usize, usize, usize)> = Vec::new(); // (class, pkg rank, len, idx)
        for (i, e) in self.entries.iter().enumerate() {
            let pkg = &self.pkgs[e.pkg as usize].0;
            if let Some(p) = pkg_filter {
                if pkg != p {
                    continue;
                }
            }
            let alias = self.get(e.alias);
            let class = if alias == q {
                0
            } else if alias.eq_ignore_ascii_case(q) {
                1
            } else if alias.starts_with(q) {
                2
            } else if alias.len() >= ql.len() && alias.as_bytes()[..ql.len()].eq_ignore_ascii_case(ql.as_bytes()) {
                3
            } else if ql.is_empty() {
                5
            } else if contains_ci(alias, &ql) {
                4
            } else {
                continue;
            };
            hits.push((class, pkg_rank[e.pkg as usize], alias.len(), i));
        }
        hits.sort_unstable();
        hits.into_iter().take(limit).map(|h| h.3).collect()
    }
}

const RENDER: &str = r#"
a <- commandArgs(TRUE)
tools::Rd2txt(utils:::.getHelpFile(a[1]), out = a[3],
              options = list(width = as.integer(a[2]), underline_titles = FALSE))
"#;

/// Render a topic with a separate Rscript in the background.
pub fn render(rscript: String, topic: Topic, width: u16, out: PathBuf, tx: Sender<AppEvent>) {
    std::thread::spawn(move || {
        let title = format!("{}::{}", topic.pkg, topic.alias);
        let res = Command::new(&rscript)
            .arg("--vanilla")
            .arg("-e")
            .arg(RENDER)
            .arg(&topic.file)
            .arg(width.clamp(30, 100).to_string())
            .arg(&out)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output();
        let page = match res {
            Ok(o) => match std::fs::read_to_string(&out) {
                Ok(text) => {
                    let lines: Vec<&str> = text.lines().collect();
                    Ok(HelpPage::from_text(&title, &lines))
                }
                Err(_) => Err(format!("cannot render help for {title}: {}", String::from_utf8_lossy(&o.stderr).trim())),
            },
            Err(e) => Err(format!("cannot run {rscript}: {e}")),
        };
        let _ = std::fs::remove_file(&out);
        let _ = tx.send(AppEvent::HelpPage(page));
    });
}

pub fn build_in_background(libs: Vec<PathBuf>, tx: Sender<AppEvent>) {
    std::thread::spawn(move || {
        let idx = HelpIndex::build(&libs);
        let _ = tx.send(AppEvent::HelpIndex(idx));
    });
}

pub struct HelpView {
    pub index: Option<HelpIndex>,
    pub index_libs: Vec<PathBuf>,
    pub attached: Vec<String>,
    pub query: String,
    pub results: Vec<usize>,
    pub sel: usize,
    pub results_scroll: usize,
    pub page: Option<HelpPage>,
    pub back: Vec<(HelpPage, usize)>,
    pub forward: Vec<(HelpPage, usize)>,
    pub row: usize,
    pub col: usize,
    pub loading: Option<String>,
}

impl HelpView {
    pub fn new() -> HelpView {
        HelpView {
            index: None,
            index_libs: Vec::new(),
            attached: Vec::new(),
            query: String::new(),
            results: Vec::new(),
            sel: 0,
            results_scroll: 0,
            page: None,
            back: Vec::new(),
            forward: Vec::new(),
            row: 0,
            col: 0,
            loading: None,
        }
    }

    pub fn update_results(&mut self) {
        self.results = match &self.index {
            Some(idx) => idx.search(&self.query, &self.attached, 200),
            None => Vec::new(),
        };
        self.sel = 0;
        self.results_scroll = 0;
    }

    pub fn show(&mut self, page: HelpPage) {
        if let Some(old) = self.page.take() {
            if old.title != page.title {
                self.back.push((old, self.row));
                if self.back.len() > 50 {
                    self.back.remove(0);
                }
            }
        }
        self.forward.clear();
        self.page = Some(page);
        self.row = 0;
        self.col = 0;
        self.loading = None;
    }

    pub fn go_back(&mut self) -> bool {
        let Some((p, row)) = self.back.pop() else { return false };
        if let Some(cur) = self.page.take() {
            self.forward.push((cur, self.row));
        }
        self.page = Some(p);
        self.row = row;
        self.col = 0;
        true
    }

    pub fn go_forward(&mut self) -> bool {
        let Some((p, row)) = self.forward.pop() else { return false };
        if let Some(cur) = self.page.take() {
            self.back.push((cur, self.row));
        }
        self.page = Some(p);
        self.row = row;
        self.col = 0;
        true
    }

    pub fn selected_topic(&self) -> Option<Topic> {
        let i = *self.results.get(self.sel)?;
        self.index.as_ref()?.topic(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx() -> HelpIndex {
        let mut i = HelpIndex::default();
        for (p, list) in [("graphics", vec!["plot.default", "plot"]), ("base", vec!["plot"]), ("ggplot2", vec!["ggplot"]), ("zz", vec!["Plot3"])] {
            let pi = i.pkgs.len() as u16;
            i.pkgs.push((p.to_string(), PathBuf::from("x")));
            for a in list {
                i.add(pi, a, a);
            }
        }
        i
    }

    #[test]
    fn ranking() {
        let i = idx();
        let r = i.search("plot", &[], 10);
        let names: Vec<String> = r.iter().map(|k| format!("{}::{}", i.pkg(*k), i.alias(*k))).collect();
        assert_eq!(names[0], "base::plot");
        assert_eq!(names[1], "graphics::plot");
        assert!(names.contains(&"ggplot2::ggplot".to_string()));
        let r = i.search("graphics::plot", &[], 10);
        assert_eq!(i.pkg(r[0]), "graphics");
        assert_eq!(i.alias(r[0]), "plot");
    }
}
