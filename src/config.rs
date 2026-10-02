//! Tiny `key = value` config file at ~/.config/trm/config.

use crate::lint::LintMode;
use crate::plot::{ColorDepth, ImageProtocol, PlotMode};
use std::path::PathBuf;

pub struct Config {
    pub plot_mode: PlotMode,
    pub image_protocol: ImageProtocol,
    pub color_depth: ColorDepth,
    pub r_command: String,
    pub r_args: Vec<String>,
    pub rscript: String,
    pub lint: LintMode,
    pub auto_pairs: bool,
    pub restore_session: bool,
    pub warnings: Vec<String>,
}

pub const EXAMPLE: &str = "# tRm configuration (~/.config/trm/config)
# plot_mode = ascii        # ascii | braille | blocks | image
# image_protocol = auto    # auto | kitty | sixel | none
# colors = auto            # auto | truecolor | 256 | 16 | mono
# r_command = R
# r_args = --no-save --quiet
# rscript = Rscript
# lint = lintr             # lintr | syntax | off   (runs on open/save)
# auto_pairs = true
# restore_session = true
";

pub fn config_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join("trm");
        }
    }
    crate::files::home().join(".config").join("trm")
}

pub fn data_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_DATA_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join("trm");
        }
    }
    crate::files::home().join(".local").join("share").join("trm")
}

impl Config {
    pub fn load() -> Config {
        let mut c = Config {
            plot_mode: PlotMode::Ascii,
            image_protocol: ImageProtocol::detect(),
            color_depth: ColorDepth::detect(),
            r_command: "R".into(),
            r_args: vec!["--no-save".into(), "--quiet".into()],
            rscript: "Rscript".into(),
            lint: LintMode::Lintr,
            auto_pairs: true,
            restore_session: true,
            warnings: Vec::new(),
        };
        let path = config_dir().join("config");
        let Ok(text) = std::fs::read_to_string(&path) else { return c };
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                c.warnings.push(format!("config line {}: expected key = value", n + 1));
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            let ok = match k {
                "plot_mode" => PlotMode::parse(v).map(|m| c.plot_mode = m).is_some(),
                "image_protocol" => ImageProtocol::parse(v).map(|p| {
                    if let Some(p) = p {
                        c.image_protocol = p
                    }
                }).is_some(),
                "colors" => ColorDepth::parse(v).map(|d| {
                    if let Some(d) = d {
                        c.color_depth = d
                    }
                }).is_some(),
                "r_command" => {
                    c.r_command = v.to_string();
                    true
                }
                "r_args" => {
                    c.r_args = v.split_whitespace().map(|s| s.to_string()).collect();
                    true
                }
                "rscript" => {
                    c.rscript = v.to_string();
                    true
                }
                "lint" => LintMode::parse(v).map(|m| c.lint = m).is_some(),
                "auto_pairs" => parse_bool(v).map(|b| c.auto_pairs = b).is_some(),
                "restore_session" => parse_bool(v).map(|b| c.restore_session = b).is_some(),
                _ => false,
            };
            if !ok {
                c.warnings.push(format!("config line {}: unknown setting '{}'", n + 1, raw.trim()));
            }
        }
        c
    }
}

fn parse_bool(v: &str) -> Option<bool> {
    match v {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}
