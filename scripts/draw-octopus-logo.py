#!/usr/bin/env python3
import struct
import sys
from pathlib import Path

import gi

gi.require_version("Rsvg", "2.0")
from gi.repository import Rsvg

TOP = "#4F9BFF"
BOTTOM = "#2A6FE0"
BODY = "#FFFFFF"
BACK = "#BFD8FF"
INK = "#1D3F8F"
OUTLINE = (
    "M180 34C250 34 292 70 292 122C292 162 274 190 272 222C270 250 320 254 344 268"
    "C358 276 346 292 328 290C300 300 270 326 236 326C208 326 198 290 180 290"
    "C162 290 152 326 124 326C90 326 60 300 32 290C14 292 2 276 16 268"
    "C40 254 90 250 88 222C86 190 68 162 68 122C68 70 110 34 180 34Z"
)
ARMS = (
    "M262 226C296 238 328 226 320 204",
    "M258 250C300 262 330 250 340 244",
    "M98 226C64 238 32 226 40 204",
    "M102 250C60 262 30 250 20 244",
)
EYES = "M165 206a19 19 0 1 1-38 0a19 19 0 1 1 38 0ZM233 206a19 19 0 1 1-38 0a19 19 0 1 1 38 0Z"
SPAN = 346
FRAMES = (16, 24, 32, 48, 64, 128, 256)


def arms(color):
    paths = "".join(f'<path d="{d}"/>' for d in ARMS)
    return f'<g fill="none" stroke="{color}" stroke-width="26" stroke-linecap="round">{paths}</g>'


def icon(size, inset, cover=0.74, shadow=False):
    side = 1024 - 2 * inset
    radius = side * 0.225
    scale = cover * side / SPAN
    origin = 512 - 180 * scale
    defs = (
        '<linearGradient id="tile" x1="0" y1="0" x2="0" y2="1">'
        f'<stop offset="0" stop-color="{TOP}"/><stop offset="1" stop-color="{BOTTOM}"/>'
        "</linearGradient>"
    )
    cast = ""
    if shadow:
        defs += (
            '<filter id="shadow"><feDropShadow dx="0" dy="10" stdDeviation="10" '
            'flood-color="#000000" flood-opacity="0.3"/></filter>'
        )
        cast = ' filter="url(#shadow)"'
    return "\n".join(
        (
            f'<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" viewBox="0 0 1024 1024">',
            f"<defs>{defs}</defs>",
            f'<rect x="{inset}" y="{inset}" width="{side}" height="{side}" rx="{radius:.1f}" '
            f'fill="url(#tile)"{cast}/>',
            f'<rect x="{inset + 4}" y="{inset + 4}" width="{side - 8}" height="{side - 8}" '
            f'rx="{radius - 4:.1f}" fill="none" stroke="#FFFFFF" stroke-opacity="0.2" stroke-width="8"/>',
            f'<g transform="translate({origin:.1f} {origin:.1f}) scale({scale:.4f})">',
            arms(BACK),
            f'<path fill="{BODY}" d="{OUTLINE}"/>',
            f'<path fill="{INK}" d="{EYES}"/>',
            "</g>",
            "</svg>",
            "",
        )
    )


def frame(size):
    return icon(size, 0, 0.84) if size <= 32 else icon(size, 40)


def mark():
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 360 360" fill="currentColor">'
        f'<path fill-rule="evenodd" d="{OUTLINE}{EYES}"/>{arms("currentColor")}</svg>\n'
    )


def png(svg):
    return Rsvg.Handle.new_from_data(svg.encode()).get_pixbuf().save_to_bufferv("png", [], [])[1]


def ico(frames):
    offset = 6 + 16 * len(frames)
    entries = []
    for size, data in frames:
        entries.append(struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(data), offset))
        offset += len(data)
    return struct.pack("<HHH", 0, 1, len(frames)) + b"".join(entries) + b"".join(data for _, data in frames)


def main() -> None:
    root = Path(sys.argv[1])
    dist = root / "dist"
    (dist / "octodock.svg").write_text(icon(1024, 100))
    (dist / "octodock.png").write_bytes(png(icon(1024, 40)))
    (dist / "macos" / "icon-1024.png").write_bytes(png(icon(1024, 100, shadow=True)))
    (dist / "windows" / "octodock.ico").write_bytes(ico([(size, png(frame(size))) for size in FRAMES]))
    (root / "crates" / "ui" / "assets" / "icons" / "zeron-logo.svg").write_text(mark())


if __name__ == "__main__":
    main()
