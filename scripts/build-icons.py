# /// script
# dependencies = ["resvg-py==0.5.0"]
# ///
"""Draws every AltTabio icon: a lit Tab keycap with a ⇥ legend on a graphite tile.

Run `uv run scripts/build-icons.py` from anywhere, then `scripts/mac/compile-icon.sh` for macOS.
The 16, 24, 32 and 48 px icons are drawn on the pixel grid, not scaled from the 1024 master.
"""
import json
import struct
from pathlib import Path

import resvg_py

ROOT = Path(__file__).resolve().parents[1]
COLORS = {  # Settings > icon colour; azure is the default.
    "azure": "#3297e9", "copper": "#c27e49", "ember": "#f47016", "indigo": "#5679f3",
    "orchid": "#dd49af", "rosewood": "#e43d60", "vermilion": "#ec4b22", "violet": "#8c5ff0",
}
TILE = ("#3d4046", "#25272b")

# 1024 master: the key's footprint, its face, and their corner radii.
KW, KH = 770, 524
KX, KY = 512 - KW / 2, 508 - KH / 2
SIDE, TOP, LIP = 14, 10, 84
RAD, FRAD = 104, 90
KEY = (KX, KY, KW, KH)
FACE = (KX + SIDE, KY + TOP, KW - 2 * SIDE, KH - TOP - LIP)
MASK = 'maskUnits="userSpaceOnUse" x="-100" y="-100" width="1300" height="1300"'


def shade(c, f):
    return "#" + "".join(f"{round(int(c[i:i + 2], 16) * f):02x}" for i in (1, 3, 5))


def tint(c, f):
    return "#" + "".join(f"{round(int(c[i:i + 2], 16) + (255 - int(c[i:i + 2], 16)) * f):02x}" for i in (1, 3, 5))


def palette(key):
    """Skirt top, skirt bottom, face top, face bottom, legend."""
    if key == COLORS["azure"]:
        return "#2f6fb8", "#1f4f8c", "#4aa6f0", "#2f84d4", "#f0f8ff"
    return shade(key, .68), shade(key, .5), tint(key, .12), shade(key, .86), tint(key, .94)


def rect(r, rad, extra=""):
    x, y, w, h = r
    return f'<rect x="{x:g}" y="{y:g}" width="{w:g}" height="{h:g}" rx="{rad:g}"{extra}/>'


def legend(color):
    """The ⇥ on the key face: shaft, arrowhead and stop bar, nudged 7 px left for optical balance."""
    x, y, w, h = FACE
    cx, cy, t = x + w / 2, y + h / 2, 75
    left = cx - 7 - (460 + t) / 2 + t / 2
    bar = left + 460
    tip = bar - 81 - t
    return (f'<path d="M{left:g} {cy:g}H{tip:g}M{tip - 114:g} {cy - 114:g}L{tip:g} {cy:g}L{tip - 114:g} {cy + 114:g}'
            f'M{bar:g} {cy - 131:g}V{cy + 131:g}" fill="none" stroke="{color}" stroke-width="{t}" '
            'stroke-linecap="round" stroke-linejoin="round"/>')


def key_svg(key):
    skirt_top, skirt_bottom, face_top, face_bottom, ink = palette(key)
    x, y, w, h = FACE
    return ('<defs>'
            f'<linearGradient id="skirt" x1="0" y1="{KY:g}" x2="0" y2="{KY + KH:g}" gradientUnits="userSpaceOnUse">'
            f'<stop offset="0" stop-color="{skirt_top}"/><stop offset="1" stop-color="{skirt_bottom}"/></linearGradient>'
            f'<linearGradient id="face" x1="0" y1="{y:g}" x2="0" y2="{y + h:g}" gradientUnits="userSpaceOnUse">'
            f'<stop offset="0" stop-color="{face_top}"/><stop offset="1" stop-color="{face_bottom}"/></linearGradient>'
            f'<radialGradient id="dish" cx="{x + w / 2:g}" cy="{y + h / 2:g}" r="{w * .56:g}" gradientUnits="userSpaceOnUse" '
            f'gradientTransform="translate(0 {y + h / 2:g}) scale(1 .62) translate(0 {-(y + h / 2):g})">'
            '<stop offset="0" stop-color="#fff" stop-opacity=".10"/><stop offset=".65" stop-color="#fff" stop-opacity="0"/>'
            '<stop offset="1" stop-color="#000" stop-opacity=".08"/></radialGradient>'
            f'<linearGradient id="sheen" x1="{x:g}" y1="0" x2="{x + w:g}" y2="0" gradientUnits="userSpaceOnUse">'
            '<stop offset="0" stop-color="#fff" stop-opacity="0"/><stop offset=".12" stop-color="#fff" stop-opacity=".6"/>'
            '<stop offset=".88" stop-color="#fff" stop-opacity=".6"/><stop offset="1" stop-color="#fff" stop-opacity="0"/></linearGradient>'
            f'<mask id="top-edge" {MASK}><g fill="#fff">{rect(FACE, FRAD)}</g><g fill="#000">{rect((x, y + 2, w, h), FRAD)}</g></mask>'
            f'<clipPath id="face-clip">{rect(FACE, FRAD)}</clipPath></defs>'
            + rect(KEY, RAD, ' fill="url(#skirt)"')
            + rect(FACE, FRAD, ' fill="url(#face)"')
            + rect(FACE, FRAD, ' fill="url(#dish)"')
            + '<g mask="url(#top-edge)"><rect x="-100" y="-100" width="1300" height="1300" fill="url(#sheen)"/></g>'
            + f'<g clip-path="url(#face-clip)">{legend(ink)}</g>')


def shadow_svg():
    return ('<defs><filter id="seat" x="-30%" y="-50%" width="160%" height="200%"><feGaussianBlur stdDeviation="12"/></filter></defs>'
            f'<g fill="#000" opacity=".42" filter="url(#seat)">{rect((KX + 8, KY + 10, KW - 16, KH), RAD)}</g>')


def master(key):
    return ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024"><defs>'
            f'<linearGradient id="tile" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{TILE[0]}"/>'
            f'<stop offset="1" stop-color="{TILE[1]}"/></linearGradient>'
            '<linearGradient id="rim" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fff" stop-opacity=".25"/>'
            '<stop offset=".5" stop-color="#fff" stop-opacity=".12"/><stop offset="1" stop-color="#fff" stop-opacity=".08"/></linearGradient>'
            '<clipPath id="tile-clip"><rect x="32" y="32" width="960" height="960" rx="216"/></clipPath></defs>'
            '<rect x="32" y="32" width="960" height="960" rx="216" fill="url(#tile)"/>'
            f'<g clip-path="url(#tile-clip)">{shadow_svg()}{key_svg(key)}</g>'
            '<rect x="33" y="33" width="958" height="958" rx="215" fill="none" stroke="url(#rim)" stroke-width="2"/></svg>')


def pixels(cells, w=2, h=2):
    return "".join(f'<rect x="{x}" y="{y}" width="{w}" height="{h}"/>' for x, y in cells)


# Pixel-grid drawings: tile, key, face, corner radius, legend.
SMALL = {
    48: dict(tile=(1, 1, 46, 46, 10), key=(6, 12, 36, 25), face=(7, 12, 34, 21), r=5,
             legend='<path d="M14 22.5H26M21 17.5L26 22.5L21 27.5" fill="none" stroke="{c}" stroke-width="3.5" '
                    'stroke-linecap="round" stroke-linejoin="round"/><rect x="29.5" y="16" width="3.5" height="13" rx="1.75" fill="{c}"/>'),
    32: dict(tile=(1, 1, 30, 30, 7), key=(4, 8, 24, 17), face=(5, 8, 22, 14), r=3.5,
             legend='<g fill="{c}"><rect x="8" y="14" width="9" height="2"/>'
                    + pixels([(13, 11), (14, 12), (15, 13), (16, 14), (15, 15), (14, 16), (13, 17)])
                    + '<rect x="20" y="10" width="2" height="10"/></g>'),
    24: dict(tile=(1, 1, 22, 22, 5), key=(2, 5, 20, 14), face=(3, 5, 18, 12), r=2.5,
             legend='<g fill="{c}"><rect x="5" y="10" width="8" height="2"/>'
                    + pixels([(9, 6), (10, 7), (11, 8), (12, 9), (13, 10), (12, 11), (11, 12), (10, 13), (9, 14)])
                    + '<rect x="17" y="7" width="2" height="8"/></g>'),
    16: dict(tile=(0, 0, 16, 16, 3.5), key=(1, 2, 14, 12), face=(1, 2, 14, 10), r=2,
             legend='<g fill="{c}"><rect x="3" y="6" width="5" height="2"/>'
                    + pixels([(5, 3), (5, 10)], 1, 1)
                    + pixels([(5, 4), (6, 5), (7, 6), (7, 7), (6, 8), (5, 9)], 2, 1)
                    + '<rect x="11" y="3" width="2" height="8"/></g>'),
}


def small(key, n):
    d = SMALL[n]
    skirt_top, skirt_bottom, face_top, face_bottom, ink = palette(key)
    tx, ty, tw, th, tr = d["tile"]
    tile = f'<rect x="{tx:g}" y="{ty:g}" width="{tw:g}" height="{th:g}" rx="{tr:g}"/>'
    edge = (f'<rect x="{tx + .5:g}" y="{ty + .5:g}" width="{tw - 1:g}" height="{th - 1:g}" rx="{tr - .5:g}" '
            'fill="none" stroke="#fff" stroke-opacity=".16"/>' if n >= 24 else "")
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {n} {n}"><defs>'
            f'<linearGradient id="t" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{TILE[0]}"/><stop offset="1" stop-color="{TILE[1]}"/></linearGradient>'
            f'<linearGradient id="s" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{skirt_top}"/><stop offset="1" stop-color="{skirt_bottom}"/></linearGradient>'
            f'<linearGradient id="f" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{face_top}"/><stop offset="1" stop-color="{face_bottom}"/></linearGradient>'
            f'<clipPath id="c">{tile}</clipPath></defs>'
            f'<g fill="url(#t)">{tile}</g><g clip-path="url(#c)">'
            + rect(d["key"], d["r"], ' fill="url(#s)"') + rect(d["face"], d["r"] - .5, ' fill="url(#f)"')
            + d["legend"].format(c=ink) + '</g>' + edge + '</svg>')


def png(svg, size):
    return bytes(resvg_py.svg_to_bytes(svg_string=svg, width=size, height=size))


def ico(images):
    """An .ico of PNG images, given [(size, png bytes)]."""
    header = struct.pack("<HHH", 0, 1, len(images))
    entries, data, offset = b"", b"", 6 + 16 * len(images)
    for size, image in images:
        entries += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(image), offset + len(data))
        data += image
    return header + entries + data


def drawing(key, size):
    if size in SMALL:
        return png(small(key, size), size)
    if size == 20:
        return png(small(key, 24), 20)
    return png(master(key), size)


for name, key in COLORS.items():
    folder = ROOT / "assets" / "icons" / name
    (folder / f"AltTabio-{name}.ico").write_bytes(ico([(s, drawing(key, s)) for s in (16, 20, 24, 32, 48, 256)]))
    (folder / f"AltTabio-{name}-tray.ico").write_bytes(ico([(s, drawing(key, s)) for s in (16, 20, 24, 32)]))

# macOS: the key on a glass layer over its shadow, on a graphite fill. The canvas maps to the
# squircle, so the 960 px tile of the master is scaled up to fill it.
doc = ROOT / "assets" / "mac" / "AltTabio.icon"
canvas = ('<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">'
          f'<g transform="translate(512 512) scale({1024 / 960 * 1.035:g}) translate(-512 -512)">{{}}</g></svg>')
(doc / "Assets").mkdir(parents=True, exist_ok=True)
(doc / "Assets" / "key.png").write_bytes(png(canvas.format(key_svg(COLORS["azure"])), 1024))
(doc / "Assets" / "shadow.png").write_bytes(png(canvas.format(shadow_svg()), 1024))


def srgb(c):
    return "srgb:%.3f,%.3f,%.3f,1" % tuple(int(c[i:i + 2], 16) / 255 for i in (1, 3, 5))


(doc / "icon.json").write_text(json.dumps({
    "fill": {"linear-gradient": [srgb(TILE[0]), srgb(TILE[1])]},
    "groups": [
        {"name": "key", "layers": [{"glass": True, "image-name": "key.png", "name": "key"}],
         "shadow": {"kind": "neutral", "opacity": 0.5}, "translucency": {"enabled": False, "value": 0}},
        {"name": "shadow", "layers": [{"glass": False, "image-name": "shadow.png", "name": "shadow"}],
         "shadow": {"kind": "none", "opacity": 0}, "translucency": {"enabled": False, "value": 0}},
    ],
    "supported-platforms": {"squares": ["macOS"]},
}, indent=2) + "\n")

# Website: favicons and the touch icon, in the default azure.
docs = ROOT / "docs"
(docs / "favicon.svg").write_text(master(COLORS["azure"]) + "\n")
(docs / "favicon.ico").write_bytes(ico([(s, drawing(COLORS["azure"], s)) for s in (16, 32, 48)]))
(docs / "alttabio-icon.png").write_bytes(drawing(COLORS["azure"], 256))
