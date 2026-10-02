#!/bin/sh
# Install tRm for the current user. Picks the 32-bit or 64-bit binary
# automatically. Run from the source folder (after ./build.sh) or from dist/.
#   sh install.sh               installs to ~/.local/bin
#   PREFIX=/usr/local sh install.sh   (as root) installs to /usr/local/bin
set -e
here=$(cd "$(dirname "$0")" && pwd)

case "$(uname -m)" in
  x86_64|amd64) arch=x64 ;;
  i386|i486|i586|i686|x86) arch=x32 ;;
  *) echo "Unsupported CPU: $(uname -m) (only 32/64-bit x86 builds exist)" >&2; exit 1 ;;
esac

src=""
for d in "$here/$arch" "$here/dist/$arch"; do
  [ -x "$d/trm" ] && { src="$d/trm"; break; }
done
if [ -z "$src" ]; then
  echo "No $arch binary found. Run ./build.sh first (or copy dist/ from a build machine)." >&2
  exit 1
fi

bindir="${PREFIX:-$HOME/.local}/bin"
mkdir -p "$bindir"
cp "$src" "$bindir/trm.new" && mv "$bindir/trm.new" "$bindir/trm"
chmod +x "$bindir/trm"
echo "Installed $arch build to $bindir/trm"

# example config (never overwrite an existing one)
cfgdir="${XDG_CONFIG_HOME:-$HOME/.config}/trm"
for c in "$here/config.example" "$here/../config.example"; do
  if [ -f "$c" ] && [ ! -f "$cfgdir/config" ]; then
    mkdir -p "$cfgdir"
    cp "$c" "$cfgdir/config"
    echo "Wrote default config to $cfgdir/config"
    break
  fi
done

case ":$PATH:" in
  *":$bindir:"*) ;;
  *) echo
     echo "NOTE: $bindir is not on your PATH. Add this line to ~/.bashrc (or ~/.profile):"
     echo "  export PATH=\"$bindir:\$PATH\""
     ;;
esac

command -v R >/dev/null 2>&1 || echo "WARNING: R not found - install R (e.g. sudo apt install r-base)"
command -v Rscript >/dev/null 2>&1 || echo "WARNING: Rscript not found - help pages and lint need it"
echo
echo "Start with: trm   (or: trm myscript.R)   - press F1 inside for shortcuts"
