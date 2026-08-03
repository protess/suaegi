#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
dist_dir="$repo_root/dist"
target=${SUEGI_TARGET:-$(rustc -vV | awk '/^host:/ { print $2 }')}
version=${SUEGI_VERSION:-$(awk '
    /^\[workspace.package\]$/ { workspace_package = 1; next }
    /^\[/ { workspace_package = 0 }
    workspace_package && /^version = / {
        gsub(/version = |"/, "")
        print
        exit
    }
' "$repo_root/Cargo.toml")}
build_number=${SUEGI_BUILD_NUMBER:-1}
signing_identity=${SUEGI_SIGN_IDENTITY:-}
require_notarization=${SUEGI_REQUIRE_NOTARIZATION:-0}
notary_key=${SUEGI_NOTARY_KEY_PATH:-}
notary_key_id=${SUEGI_NOTARY_KEY_ID:-}
notary_issuer=${SUEGI_NOTARY_ISSUER_ID:-}

case "$target" in
    aarch64-apple-darwin) release_arch=arm64 ;;
    x86_64-apple-darwin) release_arch=x86_64 ;;
    *)
        printf 'unsupported macOS release target: %s\n' "$target" >&2
        exit 2
        ;;
esac

case "$version" in
    ''|*[!0-9A-Za-z.+-]*)
        printf 'invalid release version: %s\n' "$version" >&2
        exit 2
        ;;
esac
case "$build_number" in
    ''|*[!0-9]*)
        printf 'SUEGI_BUILD_NUMBER must contain only digits\n' >&2
        exit 2
        ;;
esac
if [ "$(uname -s)" != Darwin ]; then
    printf 'macOS release packaging must run on macOS\n' >&2
    exit 2
fi

work_dir="$dist_dir/.macos-release-$release_arch"
case "$work_dir" in
    "$repo_root"/dist/.macos-release-*) ;;
    *)
        printf 'refusing unsafe work directory: %s\n' "$work_dir" >&2
        exit 2
        ;;
esac

notary_values=0
for value in "$notary_key" "$notary_key_id" "$notary_issuer"; do
    if [ -n "$value" ]; then
        notary_values=$((notary_values + 1))
    fi
done
if [ "$notary_values" -ne 0 ] && [ "$notary_values" -ne 3 ]; then
    printf 'SUEGI_NOTARY_KEY_PATH, SUEGI_NOTARY_KEY_ID, and SUEGI_NOTARY_ISSUER_ID must be provided together\n' >&2
    exit 2
fi
if [ "$require_notarization" = 1 ] && { [ -z "$signing_identity" ] || [ "$notary_values" -ne 3 ]; }; then
    printf 'production packaging requires a Developer ID identity and complete notary credentials\n' >&2
    exit 2
fi

rm -rf "$work_dir"
mkdir -p "$work_dir/Suaegi.app/Contents/MacOS" \
    "$work_dir/Suaegi.app/Contents/Resources/notification-sounds" \
    "$dist_dir"

cd "$repo_root"
cargo build --locked --release -p suaegi-app --target "$target"

app_dir="$work_dir/Suaegi.app"
binary="$repo_root/target/$target/release/suaegi-app"
cp "$binary" "$app_dir/Contents/MacOS/suaegi-app"
cp "$repo_root/packaging/macos/Info.plist" "$app_dir/Contents/Info.plist"
cp "$repo_root/packaging/macos/AppIcon.icns" "$app_dir/Contents/Resources/AppIcon.icns"
cp "$repo_root/packaging/macos/AppIcon-watercolor.png" "$app_dir/Contents/Resources/AppIcon-watercolor.png"
cp "$repo_root/packaging/macos/AppIcon-blue.png" "$app_dir/Contents/Resources/AppIcon-blue.png"
cp "$repo_root"/packaging/macos/notification-sounds/*.mp3 \
    "$app_dir/Contents/Resources/notification-sounds/"
chmod +x "$app_dir/Contents/MacOS/suaegi-app"

/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$app_dir/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $build_number" "$app_dir/Contents/Info.plist"
plutil -lint "$app_dir/Contents/Info.plist" >/dev/null
plutil -lint "$repo_root/packaging/macos/Suaegi.entitlements" >/dev/null

if [ -n "$signing_identity" ]; then
    codesign --force --timestamp --options runtime \
        --entitlements "$repo_root/packaging/macos/Suaegi.entitlements" \
        --sign "$signing_identity" "$app_dir"
else
    codesign --force --sign - "$app_dir"
fi
codesign --verify --deep --strict --verbose=2 "$app_dir"

notary_submit() {
    submission_path=$1
    result_path=$2
    xcrun notarytool submit "$submission_path" \
        --key "$notary_key" \
        --key-id "$notary_key_id" \
        --issuer "$notary_issuer" \
        --wait --timeout 45m --output-format json >"$result_path"
    status=$(plutil -extract status raw -o - "$result_path")
    if [ "$status" != Accepted ]; then
        submission_id=$(plutil -extract id raw -o - "$result_path")
        xcrun notarytool log "$submission_id" "$result_path.log.json" \
            --key "$notary_key" \
            --key-id "$notary_key_id" \
            --issuer "$notary_issuer" || true
        printf 'notarization failed for %s with status %s\n' "$submission_path" "$status" >&2
        exit 1
    fi
}

preflight_zip="$work_dir/Suaegi-notary.zip"
if [ "$notary_values" -eq 3 ]; then
    ditto -c -k --sequesterRsrc --keepParent "$app_dir" "$preflight_zip"
    notary_submit "$preflight_zip" "$work_dir/notary-app.json"
    xcrun stapler staple -v "$app_dir"
    xcrun stapler validate -v "$app_dir"
fi

artifact_base="Suaegi-$version-macos-$release_arch"
zip_path="$dist_dir/$artifact_base.zip"
dmg_path="$dist_dir/$artifact_base.dmg"
checksum_path="$dist_dir/$artifact_base.sha256"
rm -f "$zip_path" "$dmg_path" "$checksum_path"
ditto -c -k --sequesterRsrc --keepParent "$app_dir" "$zip_path"

dmg_source="$work_dir/dmg"
mkdir -p "$dmg_source"
ditto "$app_dir" "$dmg_source/Suaegi.app"
ln -s /Applications "$dmg_source/Applications"
hdiutil create -quiet -volname Suaegi -srcfolder "$dmg_source" -ov -format UDZO "$dmg_path"
if [ -n "$signing_identity" ]; then
    codesign --force --timestamp --sign "$signing_identity" "$dmg_path"
    codesign --verify --strict --verbose=2 "$dmg_path"
fi
if [ "$notary_values" -eq 3 ]; then
    notary_submit "$dmg_path" "$work_dir/notary-dmg.json"
    xcrun stapler staple -v "$dmg_path"
    xcrun stapler validate -v "$dmg_path"
fi
hdiutil verify "$dmg_path" >/dev/null

(
    cd "$dist_dir"
    shasum -a 256 "$(basename "$zip_path")" "$(basename "$dmg_path")" >"$(basename "$checksum_path")"
)

printf '%s\n%s\n%s\n' "$zip_path" "$dmg_path" "$checksum_path"
