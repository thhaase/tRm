# tRm

An RStudio-like IDE for R in the terminal. One static binary (~1.2 MB) for
32-bit and 64-bit Linux. Uses under 10 MB of RAM besides R itself. Runs on
old netbooks, over SSH, and on the bare Linux console.

## Features

- **Four panes:** Source, Console, Environment, and Files / Plots / Viewer / Help.
- **RStudio workflow:** Ctrl+Enter sends the line, the full multi-line
  statement, or the selection to the console.
- **Real R console:** readline, history, tab completion and `browser()` work
  as usual.
- **Editor:** R syntax highlighting, bracket matching, auto-indent, auto-closing
  brackets, undo/redo, find/replace, comments, tabs, and an outline of
  `# Section ----` headers and functions.
- **Lint:** syntax errors and lintr notes in the gutter, run in the background.
- **Environment:** name, type, size and value of every object. Data frames
  and lists expand to columns and elements, with a preview.
- **Plots:** readable ASCII by default, with real text for titles, labels,
  ticks and legends. Optional braille, color blocks, or real images (kitty /
  sixel). Plot history and PNG export.
- **Viewer:** `View(df)` shows a scrollable table.
- **Help:** a searchable index of all installed packages. `?topic` opens
  immediately, with no package selection menu.
- **Robust:** errors are caught and logged instead of crashing, saves are
  atomic, and unsaved files are backed up every 30 s.

## Manual

### Start

```sh
trm                 # reopen the files from the last session
trm analysis.R      # open files
trm ~/project       # start in a folder (R's working directory)
```

F1 lists all shortcuts. F10 opens the menu bar (also clickable).

### Panes

| Pane | Key | Fallback |
| --- | --- | --- |
| Source | Ctrl+1 | Alt+1 |
| Console | Ctrl+2 | Alt+2 |
| Help | Ctrl+3 | Alt+3 |
| Files | Ctrl+5 | Alt+5 |
| Plots | Ctrl+6 | Alt+6 |
| Environment | Ctrl+8 | Alt+8 |
| Viewer | Ctrl+9 | Alt+9 |

- **Zoom a pane:** Ctrl+Shift+digit, or Alt+Z for the current pane.
  Ctrl+Shift+0 shows all panes again.
- **Mouse:** click to focus, drag the borders to resize, scroll with the wheel.

### Running code

| Key | Action |
| --- | --- |
| Ctrl+Enter | Run the selection, or the statement under the cursor, then move to the next line |
| Ctrl+A, Ctrl+Enter | Run the whole file |
| Ctrl+Shift+S | `source()` the file |
| Ctrl+Shift+Enter | `source()` with echo |
| Ctrl+Shift+M | Insert ` \|> ` |
| Alt+- | Insert ` <- ` |
| Ctrl+C (in console) | Interrupt R |
| Ctrl+L | Clear the console |
| Ctrl+Shift+F10 | Restart R |

"Statement" means the complete expression: a whole function definition, pipe
chain, or call spread over several lines. Code sent while R is busy is queued
and runs when R is ready.

### Editing

| Key | Action |
| --- | --- |
| Ctrl+Z / Ctrl+Y | Undo / redo |
| Ctrl+X / C / V | Cut / copy / paste (copy also sets the system clipboard via OSC 52) |
| Ctrl+F, F3 | Find, find next |
| Ctrl+H | Replace all |
| Ctrl+G | Go to line |
| Ctrl+Shift+C | Comment / uncomment |
| Tab / Shift+Tab | Indent / outdent |
| Alt+Up / Alt+Down | Move line(s) |
| Ctrl+D | Delete line(s) |
| Ctrl+Shift+O | Toggle outline |
| F7 | Lint now (also runs on open and save) |
| Ctrl+N / O / S / W | New / open / save / close tab |
| Ctrl+PgUp / Ctrl+PgDn | Previous / next tab |
| Ctrl+Q | Quit |

### Environment

| Key | Action |
| --- | --- |
| Up / Down | Select |
| Enter or Right / Left | Expand / collapse |
| v | `View()` the object |
| p | `str()` the object |
| Del | `rm()` the object |
| s | Sort by name or by size |

### Plots

| Key | Action |
| --- | --- |
| Left / Right | Plot history |
| m | Cycle display mode |
| i | Toggle image view |
| r | Redraw at the current pane size |
| s | Save as PNG |

Display modes:

- **ascii** (default): lines `- | / \`, points `*` `o`, and bars with one
  character per color (`#` `=` `%` …). Text is real text. Works in every
  terminal.
- **braille:** like ascii, with smoother lines. Needs a font with braille
  characters.
- **blocks:** colored half blocks, a picture of the PNG.
- **image:** the real PNG, via the kitty graphics protocol or sixel.

### Help

- Type in the search box to search every installed package's topics.
- `pkg::topic` limits the search to one package.
- Enter opens the selected topic. Esc clears the search.
- Backspace on an empty search goes back. Alt+Left / Alt+Right move through
  history.
- `?topic` and `help(topic)` in the console open here. If several packages
  document the topic, the first one opens and the others are listed at the
  top of the page.

### Files

| Key | Action |
| --- | --- |
| Enter | Open the file or folder (PNGs open in Plots) |
| Backspace | Go to the parent folder |
| w | `setwd()` to the current folder |
| g | Go to R's working directory |
| . | Show / hide hidden files |
| n | New file |

## Install and setup

### 1. Install R

```sh
sudo apt install r-base      # Debian, Ubuntu, Mint
sudo dnf install R           # Fedora
sudo pacman -S r             # Arch
```

`lintr` is optional; without it, only syntax errors are reported.

For 32-bit machines, use a distribution that still ships 32-bit R, for
example Debian 12.

### 2. Build

Requires Rust (https://rustup.rs). Build on any 64-bit Linux machine:

```sh
git clone https://github.com/thhaase/tRm
cd tRm
./build.sh            # both architectures; or ./build.sh x32 / ./build.sh x64
```

Output:

```
dist/
├── x32/trm         32-bit (i686, needs SSE2: Atom, Pentium M/4 or newer)
├── x64/trm         64-bit (x86_64)
├── install.sh
└── config.example
```

Both binaries are statically linked (musl) and run on any Linux distribution.

### 3. Install

Copy `dist/` to the target machine, then:

```sh
sh dist/install.sh
```

The installer:

- picks the binary that matches `uname -m`
- installs it to `~/.local/bin/trm` (`PREFIX=/usr/local` installs system-wide)
- creates `~/.config/trm/config` if it does not exist
- warns if `~/.local/bin` is not in `PATH`, or if R or Rscript is missing

If `~/.local/bin` is not in `PATH`:

```sh
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
```

To update: rebuild, copy `dist/` again, and rerun `install.sh`.

### 4. Terminal setup

Everything works in any terminal through the fallbacks below. Setup is only
needed for the exact RStudio shortcuts and real plot images.

**Works everywhere:**

- Alt+digit switches panes, Alt+M inserts `|>`, Alt+C comments, Alt+O shows
  the outline.
- Ctrl+Enter arrives as Ctrl+J in most terminals, and tRm accepts it.
  Alt+Enter also runs code.
- Hold Shift while dragging to select text with the terminal's own selection.

**RStudio shortcuts** (Ctrl+Shift+…, Ctrl+digit, Ctrl+Enter) need the kitty
keyboard protocol. tRm turns it on automatically in terminals that support it:
Ghostty, kitty, WezTerm, foot, Alacritty ≥ 0.13, recent Konsole and iTerm2,
and Rio.

Some terminals reserve keys for themselves. Unbind them or use the fallback:

| Terminal | Reserved key | Fix |
| --- | --- | --- |
| Ghostty | Ctrl+Enter (fullscreen) | `keybind = ctrl+enter=unbind` in `~/.config/ghostty/config` |
| kitty | Ctrl+Shift+Enter | `map ctrl+shift+enter no_op` in `kitty.conf` |
| most | Ctrl+Shift+C (copy) | Use Alt+C to comment |

**tmux:** for Ctrl+Enter and Ctrl+Shift shortcuts, add to `~/.tmux.conf`:

```
set -g extended-keys on
set -as terminal-features 'xterm*:extkeys'
```

Image plots do not pass through tmux or screen.

**Plot images:** kitty protocol (Ghostty, kitty, WezTerm, Konsole) or sixel
(foot, WezTerm, mlterm, `xterm -ti vt340`). Support is detected automatically.
To override, set `image_protocol` in the config.

### 5. Configuration

`~/.config/trm/config` (see `config.example`):

| Setting | Values | Default |
| --- | --- | --- |
| `plot_mode` | `ascii` `braille` `blocks` `image` | `ascii` |
| `image_protocol` | `auto` `kitty` `sixel` `none` | `auto` |
| `colors` | `auto` `truecolor` `256` `16` `mono` | `auto` |
| `r_command` | R executable | `R` |
| `r_args` | R arguments | `--no-save --quiet` |
| `rscript` | Rscript (help pages, lint) | `Rscript` |
| `lint` | `lintr` `syntax` `off` | `lintr` |
| `auto_pairs` | `true` `false` | `true` |
| `restore_session` | `true` `false` | `true` |

On slow machines set `lint = syntax`. Your `~/.Rprofile` (or a project's
`.Rprofile`) is still loaded.

### Troubleshooting

| Problem | Cause / fix |
| --- | --- |
| `trm: command not found` | `~/.local/bin` is not in `PATH` (step 3) |
| "could not start R" | R is not installed or not in `PATH`; set `r_command = /path/to/R` |
| A shortcut does nothing | The terminal does not send it; use the Alt fallback (step 4) |
| Error message in the status bar | Logged to `~/.local/share/trm/crash.log`; unsaved files are in `~/.local/share/trm/recovery` |

## How it works

R runs in a pseudo-terminal, rendered by a built-in VT100 emulator. A helper
script, loaded through `R_PROFILE_USER` (it then loads your own `.Rprofile`),
registers a task callback. After each top-level command, the callback writes
an environment snapshot and any new plot (PNG plus an uncompressed vector PDF)
to a private temp directory, which tRm reads. ASCII plots are drawn from the
PDF. tRm redraws only when something changes, so it uses no CPU when idle.
