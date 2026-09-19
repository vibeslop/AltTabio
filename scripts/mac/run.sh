#!/bin/zsh
# Builds the app bundle and starts it in the foreground with its log in this terminal.
#
# Usage: scripts/mac/run.sh [--debug] [-- <AltTabio arguments>]
#   scripts/mac/run.sh                       start the switcher
#   scripts/mac/run.sh -- --preview          show the overlay once without hooking Command+Tab
#   scripts/mac/run.sh -- --list             print the windows AltTabio sees and exit
set -euo pipefail

cd "$(dirname "$0")/../.."
build_flags=()
if [[ "${1:-}" == "--debug" ]]; then
    build_flags=(--debug)
    shift
fi
if [[ "${1:-}" == "--" ]]; then
    shift
fi

scripts/mac/build-app.sh "${build_flags[@]}"
pkill -x AltTabio 2>/dev/null || true
exec target/mac/AltTabio.app/Contents/MacOS/AltTabio "$@"
