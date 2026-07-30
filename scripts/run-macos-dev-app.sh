#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)

sh "$repo_root/scripts/build-macos-dev-app.sh"

# LaunchServices can leave an ad-hoc-signed development bundle suspended on
# newer macOS releases. Running the bundle executable directly exercises the
# exact same binary and resources without that development-only gate.
exec "$repo_root/target/debug/Suaegi.app/Contents/MacOS/suaegi-app"
