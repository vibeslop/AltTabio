# /// script
# requires-python = ">=3.10"
# dependencies = ["resvg-py==0.5.0"]
# ///
"""Regenerate the embedded GitHub marks: uv run scripts/windows/build-github-icons.py."""

import struct
from pathlib import Path

import resvg_py


def main() -> None:
    folder = Path(__file__).resolve().parents[2] / "assets" / "icons" / "github"
    source = (folder / "mark-github.svg").read_text(encoding="utf-8")
    sizes = (16, 18, 20, 24, 28, 32, 36, 40, 48, 64)
    for name, color in (("dark", "#f0f0f0"), ("light", "#1c1c1c")):
        svg = source.replace("<svg ", f'<svg fill="{color}" ', 1)
        images = [resvg_py.svg_to_bytes(svg_string=svg, width=s, height=s) for s in sizes]
        offset = 6 + 16 * len(sizes)
        entries = []
        for size, image in zip(sizes, images, strict=True):
            entries.append(struct.pack("<BBBBHHII", size, size, 0, 0, 1, 32, len(image), offset))
            offset += len(image)
        (folder / f"github-{name}.ico").write_bytes(
            struct.pack("<HHH", 0, 1, len(sizes)) + b"".join(entries) + b"".join(images)
        )


if __name__ == "__main__":
    main()
