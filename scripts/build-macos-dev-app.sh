#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
app_dir="$repo_root/target/debug/Suaegi.app"

cd "$repo_root"
cargo build -p suaegi-app
mkdir -p "$app_dir/Contents/MacOS" "$app_dir/Contents/Resources"
cp "$repo_root/target/debug/suaegi-app" "$app_dir/Contents/MacOS/suaegi-app"
cp "$repo_root/packaging/macos/Info.plist" "$app_dir/Contents/Info.plist"
cp "$repo_root/packaging/macos/AppIcon.icns" "$app_dir/Contents/Resources/AppIcon.icns"
cp "$repo_root/packaging/macos/AppIcon-watercolor.png" "$app_dir/Contents/Resources/AppIcon-watercolor.png"
cp "$repo_root/packaging/macos/AppIcon-blue.png" "$app_dir/Contents/Resources/AppIcon-blue.png"
mkdir -p "$app_dir/Contents/Resources/notification-sounds"
cp "$repo_root"/packaging/macos/notification-sounds/*.mp3 "$app_dir/Contents/Resources/notification-sounds/"
chmod +x "$app_dir/Contents/MacOS/suaegi-app"

# Copying the linker-signed binary into an app bundle changes the signed
# container. Re-sign the assembled bundle so LaunchServices/RBS does not leave
# a failed process suspended in dyld before the first application frame.
if command -v codesign >/dev/null 2>&1; then
  codesign --force --deep --sign - "$app_dir"
fi

printf '%s\n' "$app_dir"
