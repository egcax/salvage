#!/bin/sh
# Builds .deb, .rpm, pacman and AppImage packages from a release build.
# Needs fpm and rsvg-convert.
set -e
VER="${1:-2.1.0}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BUNDLE="$ROOT/app/build/linux/x64/release/bundle"
STAGE="$ROOT/dist/stage"
rm -rf "$ROOT/dist"
mkdir -p "$STAGE/opt/salvage" "$STAGE/usr/bin" "$STAGE/usr/share/polkit-1/actions" \
         "$STAGE/usr/share/applications" "$STAGE/usr/share/icons/hicolor/scalable/apps" \
         "$STAGE/usr/share/licenses/salvage"
cp -a "$BUNDLE/." "$STAGE/opt/salvage/"
install -m755 "$ROOT/engine/target/release/salvage-engine" "$STAGE/opt/salvage/salvage-engine"
install -m755 "$ROOT/packaging/salvage.sh" "$STAGE/usr/bin/salvage"
install -m644 "$ROOT/packaging/dev.salvage.Salvage.policy" "$STAGE/usr/share/polkit-1/actions/"
install -m644 "$ROOT/packaging/dev.salvage.Salvage.desktop" "$STAGE/usr/share/applications/"
install -m644 "$ROOT/packaging/dev.salvage.Salvage.svg" "$STAGE/usr/share/icons/hicolor/scalable/apps/"
install -m644 "$ROOT/LICENSE" "$STAGE/usr/share/licenses/salvage/"

pack() {
    fpm -s dir -n salvage -v "$VER" --license MIT --url https://github.com/egcax/salvage \
        --maintainer egcax --architecture x86_64 \
        --description 'Find and recover deleted files from any drive' -C "$STAGE" "$@" .
}
cd "$ROOT/dist"
pack -t deb -d libgtk-3-0 -d 'pkexec | policykit-1' \
    --deb-recommends smartmontools --deb-recommends udisks2 \
    --deb-recommends ffmpegthumbnailer --deb-recommends poppler-utils
pack -t rpm -d gtk3 -d polkit -d /usr/bin/pkexec \
    --rpm-tag 'Recommends: smartmontools' --rpm-tag 'Recommends: udisks2'
pack -t pacman -d gtk3 -d polkit -d util-linux \
    --pacman-optional-depends 'smartmontools: disk health' \
    --pacman-optional-depends 'udisks2: unmount drives' \
    --pacman-optional-depends 'ffmpegthumbnailer: video thumbnails' \
    --pacman-optional-depends 'ffmpeg: HEIC and video previews' \
    --pacman-optional-depends 'poppler: PDF thumbnails'

# Polkit rules depend on the installed path, so scans from the AppImage ask for a password.
APP="$ROOT/dist/Salvage.AppDir"
mkdir -p "$APP/usr/lib/salvage"
cp -a "$STAGE/opt/salvage/." "$APP/usr/lib/salvage/"
cp "$ROOT/packaging/dev.salvage.Salvage.desktop" "$APP/"
rsvg-convert -w 256 -h 256 "$ROOT/packaging/dev.salvage.Salvage.svg" -o "$APP/dev.salvage.Salvage.png"
cat > "$APP/AppRun" <<'RUN'
#!/bin/sh
# Root cannot read a FUSE mount, so pkexec gets a private copy of the engine.
HERE="$(dirname "$(readlink -f "$0")")"
TMP="$(mktemp -d "${XDG_RUNTIME_DIR:-/tmp}/salvage.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT
cp "$HERE/usr/lib/salvage/salvage-engine" "$TMP/"
SALVAGE_ENGINE="$TMP/salvage-engine" "$HERE/usr/lib/salvage/salvage" "$@"
RUN
chmod +x "$APP/AppRun"
curl -fsSL -o appimagetool https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
chmod +x appimagetool
ARCH=x86_64 ./appimagetool --appimage-extract-and-run "$APP" "Salvage-$VER-x86_64.AppImage"
rm -rf appimagetool "$APP" "$STAGE"
sha256sum ./* > SHA256SUMS
