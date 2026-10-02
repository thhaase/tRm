#!/bin/sh
# Build static tRm binaries for 32-bit and 64-bit Linux PCs:
#   dist/x32/trm   32-bit (i686)   - old netbooks / 32-bit Linux installs
#   dist/x64/trm   64-bit (x86_64) - normal PCs
# Both are fully static (musl): no library dependencies, run on any distro.
# Usage: ./build.sh            build both
#        ./build.sh x32|x64    build only one
set -e
cd "$(dirname "$0")"

want="${1:-all}"
targets=""
case "$want" in
  all) targets="x32 x64" ;;
  x32|x64) targets="$want" ;;
  *) echo "usage: $0 [x32|x64]" >&2; exit 1 ;;
esac

triple() {
  case "$1" in
    x32) echo i686-unknown-linux-musl ;;
    x64) echo x86_64-unknown-linux-musl ;;
  esac
}

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found - install Rust from https://rustup.rs" >&2
  exit 1
fi

for t in $targets; do
  tr=$(triple "$t")
  if command -v rustup >/dev/null 2>&1; then
    rustup target list --installed | grep -qx "$tr" || rustup target add "$tr"
  fi
  echo "==> building $t ($tr)"
  cargo build --release --target "$tr"
  mkdir -p "dist/$t"
  cp "target/$tr/release/trm" "dist/$t/trm"
done

cp install.sh config.example dist/
chmod +x dist/install.sh dist/*/trm
echo
echo "Done:"
ls -l dist/*/trm
echo
echo "Copy the whole dist/ folder to any machine and run: sh dist/install.sh"
