//! Background linting with a separate Rscript process (never blocks the UI).

use crate::editor::{Lint, LintKind};
use crate::AppEvent;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LintMode {
    Off,
    Syntax,
    Lintr,
}

impl LintMode {
    pub fn parse(s: &str) -> Option<LintMode> {
        match s.trim() {
            "off" | "none" | "false" => Some(LintMode::Off),
            "syntax" | "parse" => Some(LintMode::Syntax),
            "lintr" | "on" | "true" => Some(LintMode::Lintr),
            _ => None,
        }
    }
}

const SCRIPT: &str = r#"
args <- commandArgs(TRUE)
f <- args[1]; mode <- args[2]
clean <- function(x) gsub("[\t\r\n]+", " ", x)
err <- tryCatch({ parse(f, keep.source = FALSE); NULL }, error = function(e) conditionMessage(e))
if (!is.null(err)) {
  m <- regmatches(err, regexec("([0-9]+):([0-9]+):", err))[[1]]
  ln <- if (length(m) >= 3) m[2] else "1"; cl <- if (length(m) >= 3) m[3] else "1"
  msg <- sub("^.*?[0-9]+:[0-9]+: *", "", err)
  cat("error", ln, cl, clean(msg), sep = "\t"); cat("\n")
} else if (mode == "lintr" && requireNamespace("lintr", quietly = TRUE)) {
  res <- tryCatch(lintr::lint(f, parse_settings = FALSE), error = function(e) list())
  for (x in res) {
    cat(x$type, x$line_number, x$column_number, clean(x$message), sep = "\t"); cat("\n")
  }
}
"#;

pub fn spawn(rscript: String, file: PathBuf, mode: LintMode, doc_id: u64, version: u64, tx: Sender<AppEvent>) {
    if mode == LintMode::Off {
        return;
    }
    std::thread::spawn(move || {
        let mode_s = if mode == LintMode::Lintr { "lintr" } else { "syntax" };
        let out = Command::new(&rscript)
            .arg("--vanilla")
            .arg("-e")
            .arg(SCRIPT)
            .arg(&file)
            .arg(mode_s)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        let _ = std::fs::remove_file(&file);
        let res = match out {
            Ok(o) => Ok(parse(&String::from_utf8_lossy(&o.stdout))),
            Err(e) => Err(format!("lint: cannot run {rscript}: {e}")),
        };
        let _ = tx.send(AppEvent::Lint(doc_id, version, res));
    });
}

fn parse(text: &str) -> Vec<Lint> {
    let mut v = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 4 {
            continue;
        }
        let kind = match f[0].trim() {
            "error" => LintKind::Error,
            "warning" => LintKind::Warning,
            _ => LintKind::Style,
        };
        let ln: usize = f[1].trim().parse().unwrap_or(1);
        let col: usize = f[2].trim().parse().unwrap_or(1);
        v.push(Lint { line: ln.saturating_sub(1), col: col.saturating_sub(1), kind, msg: f[3].trim().to_string() });
    }
    v
}
