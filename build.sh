#!/usr/bin/env bash
# Üç Windows build-i, hamısı MSVC + statik CRT (heç bir runtime DLL lazım deyil):
#   Win10/11 x64  — stable
#   Win7 x64/x86  — nightly + build-std (win7 target-ləri tier-3-dür)
# Tələb: rustup (stable + nightly/rust-src), cargo-xwin, clang/lld.
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"
export XWIN_ARCH=x86,x86_64
export RUSTFLAGS="-C target-feature=+crt-static"
mkdir -p dist

cargo +stable xwin build --release --target x86_64-pc-windows-msvc
for t in x86_64-win7-windows-msvc i686-win7-windows-msvc; do
  cargo +nightly xwin build --release -Zbuild-std=std,panic_abort --target "$t"
done

cp target/x86_64-pc-windows-msvc/release/wintool.exe   dist/wintool-win10-11-x64.exe
cp target/x86_64-win7-windows-msvc/release/wintool.exe dist/wintool-win7-x64.exe
cp target/i686-win7-windows-msvc/release/wintool.exe   dist/wintool-win7-x86.exe

ls -lh dist
