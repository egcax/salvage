#!/bin/sh
# Builds the engine and the app, then a pacman package.
set -e
cd "$(dirname "$0")"
(cd engine && cargo build --release)
(cd app && flutter build linux --release)
(cd packaging && makepkg -f --noconfirm --skipinteg)
echo "Install: sudo pacman -U packaging/salvage-*.pkg.tar.zst"
