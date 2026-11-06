#!/bin/sh
# Run against the release bundle. No TCC grants or running switcher are required.
set -eu

cd "$(dirname "$0")/../.."
binary=target/mac/AltTabio.app/Contents/MacOS/AltTabio
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT HUP INT TERM

rejects() {
    status=0
    "$binary" "$@" >"$work/stdout" 2>"$work/stderr" || status=$?
    if [ "$status" -ne 2 ] || [ -s "$work/stdout" ] ||
        [ "$(cat "$work/stderr")" != "--list and --activate are available only in debug builds of AltTabio" ]; then
        printf '%s\n' "Release diagnostic rejection failed for $* (exit $status)" >&2
        exit 1
    fi
}

rejects --list
rejects --activate 42
rejects --activate
rejects --activate invalid
rejects --settings --list
rejects --preview --activate 42
rejects --activate 42 --list
