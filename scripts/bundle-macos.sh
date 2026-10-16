#!/bin/sh
# Builds dist/Ferrender.app for this Mac. It is a local build: ad-hoc
# signed, not notarized for distribution.
set -eu
cd "$(dirname "$0")/.."

# Verify the official archive before cadrum can consume native libraries.
OCCT_ROOT=$(python3 scripts/prepare-occt.py)
export OCCT_ROOT
cargo build --locked --release -p ferrender

app=dist/Ferrender.app
version=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/ferrender/Cargo.toml | head -1)
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/ferrender "$app/Contents/MacOS/ferrender"
# The embedded font is redistributed with the app; include its license and provenance.
mkdir -p "$app/Contents/Resources/licenses"
cp crates/fr-core/assets/fonts/OFL.txt "$app/Contents/Resources/licenses/NotoSans-OFL.txt"
cp crates/fr-core/assets/fonts/PROVENANCE.txt "$app/Contents/Resources/licenses/NotoSans-PROVENANCE.txt"
python3 scripts/bundle-dependency-notices.py --output "$app/Contents/Resources/licenses/rust-dependencies"

icon=""
if [ -f assets/icon.png ]; then
    set=$(mktemp -d)/Ferrender.iconset
    mkdir "$set"
    for s in 16 32 128 256 512; do
        sips -z $s $s assets/icon.png --out "$set/icon_${s}x${s}.png" >/dev/null
        sips -z $((s * 2)) $((s * 2)) assets/icon.png --out "$set/icon_${s}x${s}@2x.png" >/dev/null
    done
    iconutil -c icns "$set" -o "$app/Contents/Resources/Ferrender.icns"
    icon="<key>CFBundleIconFile</key><string>Ferrender</string>"
fi

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Ferrender</string>
    <key>CFBundleDisplayName</key><string>Ferrender</string>
    <key>CFBundleIdentifier</key><string>org.ferrender.Ferrender</string>
    <key>CFBundleExecutable</key><string>ferrender</string>
    $icon
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.graphics-design</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key><string>Ferrender Design</string>
            <key>CFBundleTypeExtensions</key><array><string>ferr</string><string>stl</string></array>
            <key>CFBundleTypeRole</key><string>Editor</string>
        </dict>
    </array>
</dict>
</plist>
PLIST

codesign --force --sign - "$app"
echo "built: $PWD/$app"
