#!/bin/sh
# Prints true when a platform's release archive has to be built again, false when nothing it ships
# changed between two refs: release-changes.sh windows|macos <base> <head>. The paths that decide
# go to stderr. A path counts for both platforms unless it is known to belong to one, so a missing
# rule costs an extra build, never a missed one. Version bumps alone do not count.
set -eu

platform=$1
base=$2
head=$3
case $platform in
    windows) other=macos ;;
    macos) other=windows ;;
    *) echo "Unknown platform: $platform" >&2; exit 2 ;;
esac
for ref in "$base" "$head"; do
    git rev-parse --quiet --verify "$ref^{commit}" > /dev/null || { echo "Unknown ref: $ref" >&2; exit 2; }
done

# Modules src/main.rs declares under #[cfg(windows)], at either ref.
windows_modules=$(for ref in "$base" "$head"; do
    git show "$ref:src/main.rs" | awk '
        /^#\[cfg\(windows\)\]$/ { windows = 1; next }
        windows && /^mod [a-z0-9_]+;$/ { sub(/^mod /, ""); sub(/;$/, ""); print }
        { windows = 0 }'
done | sort -u)

owner() {
    case $1 in
        THIRD_PARTY_LICENSES.md) echo both; return ;;
        docs/* | assets/site/* | tests/* | *.md | .gitignore | .github/workflows/ci.yml | scripts/build-icons.py)
            echo none; return ;;
        src/macos/* | scripts/mac/* | assets/mac/*) echo macos; return ;;
        app.rc | app.manifest | app.dev.manifest | assets/icons/* | scripts/windows/* | tools/windows-update-signing/*)
            echo windows; return ;;
    esac
    for module in $windows_modules; do
        case $1 in src/"$module".rs | src/"$module"/*) echo windows; return ;; esac
    done
    echo both
}

# The file at a ref with the app's own version blanked, so a bump compares equal.
without_version() {
    git show "$1:$2" 2>/dev/null | case $2 in
        Cargo.toml) awk '/^\[/ { package = ($0 == "[package]") }
            package && /^version = / { $0 = "version = \"\"" } { print }' ;;
        *Cargo.lock) awk 'previous == "name = \"alttabio\"" && /^version = / { $0 = "version = \"\"" }
            { print; previous = $0 }' ;;
        app.rc) sed -E -e 's/^([[:space:]]*(FILEVERSION|PRODUCTVERSION)[[:space:]]).*/\1/' \
            -e 's/("(FileVersion|ProductVersion)", ").*/\1"/' ;;
        *) cat ;;
    esac
}

changed=false
for path in $(git diff --name-only --no-renames "$base" "$head"); do
    case $(owner "$path") in none | "$other") continue ;; esac
    case $path in
        Cargo.toml | *Cargo.lock | app.rc)
            if [ "$(without_version "$base" "$path")" = "$(without_version "$head" "$path")" ]; then
                continue
            fi
            ;;
    esac
    echo "$platform: $path changed since $base" >&2
    changed=true
done
echo "$changed"
