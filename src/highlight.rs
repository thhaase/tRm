//! A small hand-written R lexer used for syntax highlighting, bracket
//! matching and finding the statement under the cursor.

use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LexState {
    #[default]
    Normal,
    Str(char),
    Raw { close: char, dashes: u8, quote: char },
    Backtick,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tok {
    Ident,
    Comment,
    Section,
    Roxygen,
    Str,
    Num,
    Keyword,
    Constant,
    Op,
    Assign,
    Pipe,
    Call,
    Bracket,
    Namespace,
}

#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub tok: Tok,
}

pub fn style(tok: Tok) -> Style {
    let s = Style::default();
    match tok {
        Tok::Ident => s,
        Tok::Comment => s.fg(Color::DarkGray),
        Tok::Roxygen => s.fg(Color::Green),
        Tok::Section => s.fg(Color::Cyan).add_modifier(Modifier::BOLD),
        Tok::Str => s.fg(Color::Green),
        Tok::Num => s.fg(Color::Magenta),
        Tok::Keyword => s.fg(Color::Blue).add_modifier(Modifier::BOLD),
        Tok::Constant => s.fg(Color::Magenta).add_modifier(Modifier::BOLD),
        Tok::Op => s.fg(Color::Yellow),
        Tok::Assign => s.fg(Color::Yellow).add_modifier(Modifier::BOLD),
        Tok::Pipe => s.fg(Color::Cyan).add_modifier(Modifier::BOLD),
        Tok::Call => s.fg(Color::LightBlue),
        Tok::Bracket => s,
        Tok::Namespace => s.fg(Color::LightRed),
    }
}

const KEYWORDS: &[&str] = &[
    "if", "else", "repeat", "while", "function", "for", "in", "next", "break", "return",
];
const CONSTANTS: &[&str] = &[
    "TRUE", "FALSE", "NULL", "NA", "NA_integer_", "NA_real_", "NA_character_",
    "NA_complex_", "Inf", "NaN",
];

pub fn is_section_comment(line: &[char]) -> bool {
    let s: String = line.iter().collect();
    let t = s.trim();
    if !t.starts_with('#') || t.starts_with("#'") {
        return false;
    }
    let body = t.trim_end();
    ["----", "====", "####"].iter().any(|m| body.ends_with(m)) && body.trim_matches(|c| c == '#' || c == '-' || c == '=' || c == ' ').len() > 0
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '.' || c == '_'
}
fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '.' || c == '_'
}

fn closing_for(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        _ => None,
    }
}

/// Scan a quoted string body starting at `i` (just after the opening quote).
/// Returns the index after the closing quote, or None when unterminated.
fn scan_string(line: &[char], mut i: usize, quote: char) -> Option<usize> {
    while i < line.len() {
        let c = line[i];
        if c == '\\' {
            i += 2;
            continue;
        }
        if c == quote {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

fn scan_raw(line: &[char], mut i: usize, close: char, dashes: u8, quote: char) -> Option<usize> {
    while i < line.len() {
        if line[i] == close {
            let mut j = i + 1;
            let mut d = 0u8;
            while j < line.len() && line[j] == '-' && d < dashes {
                j += 1;
                d += 1;
            }
            if d == dashes && j < line.len() && line[j] == quote {
                return Some(j + 1);
            }
        }
        i += 1;
    }
    None
}

/// Lex one line, appending spans to `out`. Returns the state at line end.
pub fn lex_line(line: &[char], st: LexState, out: &mut Vec<Span>) -> LexState {
    let n = line.len();
    let mut i = 0;
    // continue a multi-line construct
    match st {
        LexState::Normal => {}
        LexState::Str(q) => match scan_string(line, 0, q) {
            Some(e) => {
                out.push(Span { start: 0, end: e, tok: Tok::Str });
                i = e;
            }
            None => {
                out.push(Span { start: 0, end: n, tok: Tok::Str });
                return st;
            }
        },
        LexState::Raw { close, dashes, quote } => match scan_raw(line, 0, close, dashes, quote) {
            Some(e) => {
                out.push(Span { start: 0, end: e, tok: Tok::Str });
                i = e;
            }
            None => {
                out.push(Span { start: 0, end: n, tok: Tok::Str });
                return st;
            }
        },
        LexState::Backtick => match scan_string(line, 0, '`') {
            Some(e) => {
                out.push(Span { start: 0, end: e, tok: Tok::Ident });
                i = e;
            }
            None => {
                out.push(Span { start: 0, end: n, tok: Tok::Ident });
                return st;
            }
        },
    }

    while i < n {
        let c = line[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        // comments
        if c == '#' {
            let tok = if i + 1 < n && line[i + 1] == '\'' {
                Tok::Roxygen
            } else if is_section_comment(line) {
                Tok::Section
            } else {
                Tok::Comment
            };
            out.push(Span { start, end: n, tok });
            return LexState::Normal;
        }
        // raw strings r"(...)"
        if (c == 'r' || c == 'R') && i + 1 < n && (line[i + 1] == '"' || line[i + 1] == '\'') {
            let quote = line[i + 1];
            let mut j = i + 2;
            let mut dashes = 0u8;
            while j < n && line[j] == '-' && dashes < 200 {
                j += 1;
                dashes += 1;
            }
            if j < n {
                if let Some(close) = closing_for(line[j]) {
                    match scan_raw(line, j + 1, close, dashes, quote) {
                        Some(e) => {
                            out.push(Span { start, end: e, tok: Tok::Str });
                            i = e;
                            continue;
                        }
                        None => {
                            out.push(Span { start, end: n, tok: Tok::Str });
                            return LexState::Raw { close, dashes, quote };
                        }
                    }
                }
            }
        }
        if c == '"' || c == '\'' {
            match scan_string(line, i + 1, c) {
                Some(e) => {
                    out.push(Span { start, end: e, tok: Tok::Str });
                    i = e;
                    continue;
                }
                None => {
                    out.push(Span { start, end: n, tok: Tok::Str });
                    return LexState::Str(c);
                }
            }
        }
        if c == '`' {
            match scan_string(line, i + 1, '`') {
                Some(e) => {
                    let tok = if next_non_space(line, e) == Some('(') { Tok::Call } else { Tok::Ident };
                    out.push(Span { start, end: e, tok });
                    i = e;
                    continue;
                }
                None => {
                    out.push(Span { start, end: n, tok: Tok::Ident });
                    return LexState::Backtick;
                }
            }
        }
        // numbers
        if c.is_ascii_digit() || (c == '.' && i + 1 < n && line[i + 1].is_ascii_digit()) {
            let mut j = i;
            if c == '0' && i + 1 < n && (line[i + 1] == 'x' || line[i + 1] == 'X') {
                j += 2;
                while j < n && line[j].is_ascii_hexdigit() {
                    j += 1;
                }
            } else {
                while j < n && (line[j].is_ascii_digit() || line[j] == '.') {
                    j += 1;
                }
                if j < n && (line[j] == 'e' || line[j] == 'E') {
                    let mut k = j + 1;
                    if k < n && (line[k] == '+' || line[k] == '-') {
                        k += 1;
                    }
                    if k < n && line[k].is_ascii_digit() {
                        j = k;
                        while j < n && line[j].is_ascii_digit() {
                            j += 1;
                        }
                    }
                }
            }
            if j < n && (line[j] == 'L' || line[j] == 'i') {
                j += 1;
            }
            out.push(Span { start, end: j, tok: Tok::Num });
            i = j;
            continue;
        }
        // identifiers / keywords
        if is_ident_start(c) {
            let mut j = i;
            while j < n && is_ident_char(line[j]) {
                j += 1;
            }
            let word: String = line[i..j].iter().collect();
            let tok = if KEYWORDS.contains(&word.as_str()) {
                Tok::Keyword
            } else if CONSTANTS.contains(&word.as_str()) {
                Tok::Constant
            } else if j + 1 < n && line[j] == ':' && line[j + 1] == ':' {
                Tok::Namespace
            } else if next_non_space(line, j) == Some('(') {
                Tok::Call
            } else {
                Tok::Ident
            };
            out.push(Span { start, end: j, tok });
            i = j;
            continue;
        }
        // lambda shorthand \(x)
        if c == '\\' && next_non_space(line, i + 1) == Some('(') {
            out.push(Span { start, end: i + 1, tok: Tok::Keyword });
            i += 1;
            continue;
        }
        if matches!(c, '(' | ')' | '[' | ']' | '{' | '}') {
            out.push(Span { start, end: i + 1, tok: Tok::Bracket });
            i += 1;
            continue;
        }
        // %op%
        if c == '%' {
            let mut j = i + 1;
            while j < n && line[j] != '%' {
                j += 1;
            }
            let end = (j + 1).min(n);
            let op: String = line[i..end].iter().collect();
            let tok = if op == "%>%" || op == "%<>%" || op == "%T>%" { Tok::Pipe } else { Tok::Op };
            out.push(Span { start, end, tok });
            i = end;
            continue;
        }
        let rest = |k: usize| -> String { line[i..(i + k).min(n)].iter().collect() };
        let (len, tok) = if rest(3) == "<<-" || rest(3) == "->>" {
            (3, Tok::Assign)
        } else if rest(3) == ":::" {
            (3, Tok::Op)
        } else if rest(2) == "|>" {
            (2, Tok::Pipe)
        } else if rest(2) == "<-" || rest(2) == "->" || rest(2) == ":=" {
            (2, Tok::Assign)
        } else if ["==", "!=", "<=", ">=", "&&", "||", "::"].contains(&rest(2).as_str()) {
            (2, Tok::Op)
        } else if c == '=' {
            (1, Tok::Assign)
        } else if "+-*/^!&|~?:$@<>,;".contains(c) {
            (1, Tok::Op)
        } else {
            (1, Tok::Ident)
        };
        out.push(Span { start, end: i + len, tok });
        i += len;
    }
    LexState::Normal
}

fn next_non_space(line: &[char], mut i: usize) -> Option<char> {
    while i < line.len() {
        if !line[i].is_whitespace() {
            return Some(line[i]);
        }
        i += 1;
    }
    None
}

/// Structural information about one line, used to find complete statements.
#[derive(Clone, Copy, Debug, Default)]
pub struct LineInfo {
    pub delta: i32,
    pub min: i32,
    /// line ends with a binary operator / pipe / assignment / comma
    pub cont: bool,
    pub end: LexState,
    pub blank: bool,
    pub first: Option<char>,
    pub last: Option<char>,
}

pub fn line_info(line: &[char], st: LexState) -> LineInfo {
    let mut spans = Vec::new();
    let end = lex_line(line, st, &mut spans);
    let mut info = LineInfo { end, ..Default::default() };
    let mut depth = 0;
    let mut last_sig: Option<Span> = None;
    for sp in &spans {
        if sp.tok == Tok::Bracket {
            match line.get(sp.start) {
                Some('(') | Some('[') | Some('{') => depth += 1,
                Some(_) => {
                    depth -= 1;
                    info.min = info.min.min(depth);
                }
                None => {}
            }
        }
        if !matches!(sp.tok, Tok::Comment | Tok::Roxygen | Tok::Section) {
            last_sig = Some(*sp);
        }
    }
    info.delta = depth;
    let trimmed: Vec<char> = line.iter().copied().filter(|c| !c.is_whitespace()).collect();
    info.blank = trimmed.is_empty();
    info.first = if st == LexState::Normal { trimmed.first().copied() } else { None };
    if let Some(sp) = last_sig {
        info.last = line.get(sp.end.saturating_sub(1)).copied();
        info.cont = match sp.tok {
            Tok::Pipe | Tok::Assign => true,
            Tok::Op => line.get(sp.start) != Some(&';'),
            _ => false,
        };
    }
    info
}
