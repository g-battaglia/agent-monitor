#!/usr/bin/env python3
"""Render synthetic TestBackend cells, including their actual TUI styles.

Default: assets/screenshot.png. --menu: assets/actions.png (live-only selection).
No real catalog, provider history, or terminal capture is used.
"""
import json
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
COLS, ROWS = 120, 32
BG = (13, 17, 23)
FG = (201, 209, 217)
COLORS = {
    "Cyan": (125, 206, 251),
    "DarkGray": (70, 74, 96),
    "Gray": (180, 189, 202),
    "White": (235, 240, 255),
    "Yellow": (241, 186, 100),
    "Green": (100, 205, 145),
}


def capture(menu):
    args = ["cargo", "run", "--locked", "--example", "layout", "--", "--cells"]
    if menu:
        args.append("--menu-live")
    proc = subprocess.run(args, cwd=ROOT, capture_output=True, text=True,
                          check=True, timeout=120)
    cells = json.loads(proc.stdout)
    assert len(cells) == ROWS and all(len(row) == COLS for row in cells)
    return cells


def font(size):
    for path in ("/System/Library/Fonts/SFNSMono.ttf", "/System/Library/Fonts/Menlo.ttc",
                 "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"):
        try:
            return ImageFont.truetype(path, size)
        except OSError:
            continue
    for name in ("Menlo", "JetBrains Mono", "DejaVu Sans Mono"):
        try:
            return ImageFont.truetype(f"{name}.ttf", size)
        except OSError:
            continue
    return ImageFont.load_default()


def main():
    menu = "--menu" in sys.argv[1:]
    cells = capture(menu)
    mono = font(15)
    probe = ImageDraw.Draw(Image.new("RGB", (8, 8)))
    advance = max(int(probe.textlength("M", font=mono)) + 1, 7)
    ascent, descent = mono.getmetrics()
    line_h = ascent + descent + 5
    pad = 22
    img = Image.new("RGB", (COLS * advance + pad * 2, ROWS * line_h + pad * 2), BG)
    draw = ImageDraw.Draw(img)
    for y, row in enumerate(cells):
        for x, (symbol, foreground, background, reversed_) in enumerate(row):
            fg = COLORS.get(foreground, FG)
            bg = COLORS.get(background, BG)
            if reversed_:
                fg, bg = bg, fg
            left, top = pad + x * advance, pad + y * line_h
            if bg != BG:
                draw.rectangle((left, top, left + advance - 1, top + line_h - 1), fill=bg)
            if symbol != " ":
                draw.text((left, top), symbol, font=mono, fill=fg)
    out = ROOT / "assets" / ("actions.png" if menu else "screenshot.png")
    out.parent.mkdir(exist_ok=True)
    img.save(out)
    print(f"wrote {out} ({img.width}x{img.height})")


if __name__ == "__main__":
    main()
