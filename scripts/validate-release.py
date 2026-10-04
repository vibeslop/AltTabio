"""Authorize a tag build before any publisher key is made available."""

import json
import os
import re
import subprocess
import sys
import tomllib
from pathlib import Path


def git(*arguments):
    return subprocess.run(["git", *arguments], check=True, capture_output=True, text=True).stdout.strip()


def validate(metadata, run_sha):
    fields = {"tag", "version", "sha", "windows_digest", "macos_digest"}
    if set(metadata) != fields or not all(isinstance(value, str) for value in metadata.values()):
        raise ValueError("Invalid release metadata fields")
    tag, sha = metadata["tag"], metadata["sha"]
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag) or metadata["version"] != tag[1:]:
        raise ValueError("Invalid version tag")
    if not re.fullmatch(r"[0-9a-f]{40}", sha) or sha != run_sha:
        raise ValueError("Source does not match the completed build run")
    for field in ("windows_digest", "macos_digest"):
        if not re.fullmatch(r"[0-9a-f]{64}", metadata[field]):
            raise ValueError("Invalid artifact digest")
    if git("rev-parse", f"refs/tags/{tag}^{{commit}}") != sha:
        raise ValueError("Tag moved after the build")
    git("merge-base", "--is-ancestor", sha, "origin/main")
    package = tomllib.loads(git("show", f"{sha}:Cargo.toml"))["package"]
    if package["version"] != metadata["version"]:
        raise ValueError("Tag does not name the source version")
    pin = Path("scripts/mac/release-certificate.sha1").read_text().strip()
    if git("show", f"{sha}:scripts/mac/release-certificate.sha1") != pin:
        raise ValueError("Build source carries a different publisher identity")
    return metadata


def main():
    path = Path(sys.argv[1])
    if path.stat().st_size > 4096:
        raise ValueError("Oversized release metadata")
    metadata = validate(json.loads(path.read_text()), os.environ["BUILD_SHA"])
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        for field, value in metadata.items():
            output.write(f"{field}={value}\n")


if __name__ == "__main__":
    main()
