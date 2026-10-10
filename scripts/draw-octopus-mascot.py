#!/usr/bin/env python3
import math
import sys
import tempfile
from pathlib import Path

import cairo
from PIL import Image

CELL = 360
SHEET = 540
TAU = math.tau
BODY = (0.231, 0.541, 0.957)
BACK = (0.141, 0.369, 0.757)
INK = (0.160, 0.170, 0.210)
WHITE = (1.0, 1.0, 1.0)
PINK = (1.0, 0.45, 0.62)
GOLD = (1.0, 0.82, 0.25)
SNORE = (0.55, 0.70, 1.0)
EYES = (146.0, 214.0)
EYE_Y = 206.0
EYE_R = 19.0
DIRECTIONS = [
    (-0.75, -0.75),
    (0.0, -1.0),
    (0.75, -0.75),
    (-1.0, 0.0),
    (0.0, 0.0),
    (1.0, 0.0),
    (-0.75, 0.75),
    (0.0, 1.0),
    (0.75, 0.75),
]


def oval(ctx, x, y, rx, ry):
    ctx.save()
    ctx.translate(x, y)
    ctx.scale(rx, ry)
    ctx.new_sub_path()
    ctx.arc(0, 0, 1, 0, TAU)
    ctx.restore()


def line(ctx, color, width):
    ctx.set_source_rgb(*color)
    ctx.set_line_width(width)
    ctx.set_line_cap(cairo.LINE_CAP_ROUND)
    ctx.set_line_join(cairo.LINE_JOIN_ROUND)
    ctx.stroke()


def half(ctx, side):
    def x(value):
        return 180 + side * (value - 180)

    ctx.curve_to(x(250), 34, x(292), 70, x(292), 122)
    ctx.curve_to(x(292), 162, x(274), 190, x(272), 222)
    ctx.curve_to(x(270), 250, x(320), 254, x(344), 268)
    ctx.curve_to(x(358), 276, x(346), 292, x(328), 290)
    ctx.curve_to(x(300), 300, x(270), 326, x(236), 326)
    ctx.curve_to(x(208), 326, x(198), 290, 180, 290)


def body(ctx):
    for side in (-1, 1):
        for start, first, second, end in (
            ((262, 226), (296, 238), (328, 226), (320, 204)),
            ((258, 250), (300, 262), (330, 250), (340, 244)),
        ):
            ctx.new_path()
            ctx.move_to(180 + side * (start[0] - 180), start[1])
            ctx.curve_to(
                180 + side * (first[0] - 180),
                first[1],
                180 + side * (second[0] - 180),
                second[1],
                180 + side * (end[0] - 180),
                end[1],
            )
            line(ctx, BACK, 26)
    ctx.new_path()
    ctx.move_to(180, 34)
    half(ctx, 1)
    ctx.close_path()
    ctx.move_to(180, 34)
    half(ctx, -1)
    ctx.close_path()
    ctx.set_source_rgb(*BODY)
    ctx.fill()


def heart(ctx, x, y, s, color=PINK):
    ctx.new_path()
    ctx.move_to(x, y + s * 0.9)
    ctx.curve_to(x - s * 1.4, y - s * 0.1, x - s * 0.6, y - s * 1.1, x, y - s * 0.35)
    ctx.curve_to(x + s * 0.6, y - s * 1.1, x + s * 1.4, y - s * 0.1, x, y + s * 0.9)
    ctx.set_source_rgb(*color)
    ctx.fill()


def star(ctx, x, y, s, color=GOLD):
    ctx.new_path()
    for index in range(8):
        radius = s if index % 2 == 0 else s * 0.36
        angle = index * TAU / 8 - TAU / 4
        ctx.line_to(x + math.cos(angle) * radius, y + math.sin(angle) * radius)
    ctx.close_path()
    ctx.set_source_rgb(*color)
    ctx.fill()


def snore(ctx, x, y, s):
    ctx.new_path()
    ctx.move_to(x, y)
    ctx.line_to(x + s, y)
    ctx.line_to(x, y + s)
    ctx.line_to(x + s, y + s)
    line(ctx, SNORE, s * 0.24)


def eye(ctx, x, y, kind):
    ctx.new_path()
    if kind == "dot":
        ctx.arc(x, y, EYE_R, 0, TAU)
        ctx.set_source_rgb(*INK)
        ctx.fill()
    elif kind == "line":
        ctx.move_to(x - 16, y + 2)
        ctx.line_to(x + 16, y + 2)
        line(ctx, INK, 9)
    elif kind == "happy":
        ctx.move_to(x - 17, y + 8)
        ctx.curve_to(x - 8, y - 14, x + 8, y - 14, x + 17, y + 8)
        line(ctx, INK, 9)
    elif kind == "heart":
        heart(ctx, x, y + 1, 18)
    elif kind == "star":
        star(ctx, x, y, 23)
    elif kind == "wide":
        ctx.arc(x, y, EYE_R + 4, 0, TAU)
        ctx.set_source_rgb(*WHITE)
        ctx.fill()
        ctx.new_path()
        ctx.arc(x, y, 8, 0, TAU)
        ctx.set_source_rgb(*INK)
        ctx.fill()
    elif kind == "dizzy":
        for step in range(0, 40):
            angle = step * 0.45
            radius = 1.5 + step * 0.42
            ctx.line_to(x + math.cos(angle) * radius, y + math.sin(angle) * radius)
        line(ctx, INK, 5)


def mouth(ctx, kind, dx, dy):
    x, y = 180 + dx, 246 + dy
    ctx.new_path()
    if kind == "smile":
        ctx.move_to(x - 12, y - 4)
        ctx.curve_to(x - 6, y + 6, x + 6, y + 6, x + 12, y - 4)
        line(ctx, INK, 6)
    elif kind == "wavy":
        ctx.move_to(x - 20, y)
        ctx.curve_to(x - 13, y - 7, x - 7, y + 7, x, y)
        ctx.curve_to(x + 7, y - 7, x + 13, y + 7, x + 20, y)
        line(ctx, INK, 6)
    elif kind == "gasp":
        oval(ctx, x, y, 9, 12)
        ctx.set_source_rgb(*INK)
        ctx.fill()
    elif kind == "grin":
        ctx.move_to(x - 20, y - 8)
        ctx.line_to(x + 20, y - 8)
        ctx.curve_to(x + 18, y + 18, x - 18, y + 18, x - 20, y - 8)
        ctx.close_path()
        ctx.set_source_rgb(*INK)
        ctx.fill_preserve()
        ctx.save()
        ctx.clip()
        ctx.new_path()
        ctx.arc(x, y + 16, 12, 0, TAU)
        ctx.set_source_rgb(*PINK)
        ctx.fill()
        ctx.restore()


def blush(ctx, dx, dy):
    for x in (112, 248):
        ctx.new_path()
        oval(ctx, x + dx, EYE_Y + 26 + dy, 16, 9)
        ctx.set_source_rgba(*PINK, 0.7)
        ctx.fill()


def face(ctx, eyes, mouth_kind=None, look=(0.0, 0.0), flush=False):
    dx, dy = look[0] * 17, look[1] * 11
    if flush:
        blush(ctx, dx, dy)
    for x, kind in zip(EYES, eyes):
        eye(ctx, x + dx, EYE_Y + dy, kind)
    if mouth_kind:
        mouth(ctx, mouth_kind, dx, dy)


def gaze(look):
    return lambda ctx: face(ctx, ("dot", "dot"), look=look)


def blink(ctx):
    face(ctx, ("line", "line"))


def love(ctx):
    face(ctx, ("heart", "heart"), "smile", flush=True)
    heart(ctx, 44, 60, 15)
    heart(ctx, 318, 40, 19)
    heart(ctx, 86, 16, 11)


def sparkle(ctx):
    face(ctx, ("star", "star"), "smile")
    star(ctx, 40, 70, 22)
    star(ctx, 322, 44, 28)
    star(ctx, 330, 150, 14)
    star(ctx, 84, 18, 13)


def surprised(ctx):
    face(ctx, ("wide", "wide"), "gasp")


def wink(ctx):
    face(ctx, ("happy", "dot"), "smile")


def bashful(ctx):
    face(ctx, ("dot", "dot"), "wavy", (0.5, 0.7), flush=True)


def sleepy(ctx):
    face(ctx, ("line", "line"))
    snore(ctx, 290, 34, 20)
    snore(ctx, 320, 6, 28)


def dizzy(ctx):
    face(ctx, ("dizzy", "dizzy"), "wavy")
    star(ctx, 48, 50, 15)
    star(ctx, 316, 34, 19)


def delighted(ctx):
    face(ctx, ("happy", "happy"), "grin", flush=True)


def cell(extra):
    surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, CELL, CELL)
    ctx = cairo.Context(surface)
    ctx.translate(CELL / 2, 340)
    ctx.scale(0.9, 0.9)
    ctx.translate(-CELL / 2, -340)
    body(ctx)
    extra(ctx)
    return surface


def sheet(cells, path, scratch):
    surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, CELL * 3, CELL * 3)
    ctx = cairo.Context(surface)
    for index, extra in enumerate(cells):
        ctx.set_source_surface(cell(extra), (index % 3) * CELL, (index // 3) * CELL)
        ctx.paint()
    png = scratch / f"{path.stem}.png"
    surface.write_to_png(str(png))
    image = Image.open(png).convert("RGBA").resize((SHEET, SHEET), Image.LANCZOS)
    image.save(path, "WEBP", quality=92, method=6)


def main() -> None:
    out = Path(sys.argv[1])
    with tempfile.TemporaryDirectory() as scratch:
        sheet([gaze(look) for look in DIRECTIONS], out / "octopus-directions.webp", Path(scratch))
        sheet(
            [blink, love, sparkle, surprised, wink, bashful, sleepy, dizzy, delighted],
            out / "octopus-reactions.webp",
            Path(scratch),
        )


if __name__ == "__main__":
    main()
