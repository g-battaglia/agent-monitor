#!/usr/bin/env python3
"""Render the deterministic TUI preview to assets/screenshot.png.

Uses the app's own TestBackend output (no live data, no terminal capture),
drawn with a monospace font into a dark PNG sized like a real terminal shot.
"""
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "assets" / "screenshot.png"
COLS, ROWS = 120, 32

BG = (13, 17, 23)          # neutral dark editor background
FG = (201, 209, 217)       # primary text
ACCENT = (88, 166, 255)    # focused borders / selection
MUTED = (110, 119, 129)    # secondary text


def capture() -> list[str]:
    proc = subprocess.run(
        ["cargo", "run", "--locked", "--example", "layout"],
        cwd=ROOT, capture_output=True, text=True, check=True, timeout=120,
    )
    lines = proc.stdout.splitlines()
    # Keep the TUI frame; ignore cargo build chatter if any. The two
    # bottom rows are the hints/status lines (plain text, no borders).
    box = [l for l in lines if l]
    assert len(box) == ROWS, f"expected {ROWS} rows, got {len(box)}"
    return [l[:COLS].ljust(COLS) for l in box]


def font(size: int):
    candidates = [
        "/System/Library/Fonts/SFNSMono.ttf",  # macOS, full box-drawing
        "/System/Library/Fonts/Menlo.ttc",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    ]
    for path in candidates:
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


def main() -> None:
    rows = capture()
    mono = font(15)
    probe = ImageDraw.Draw(Image.new("RGB", (8, 8)))
    advance = max(int(probe.textlength("M", font=mono)) + 1, 7)
    ascent, descent = mono.getmetrics()
    line_h = ascent + descent + 5
    pad = 22
    img = Image.new("RGB", (COLS * advance + pad * 2, ROWS * line_h + pad * 2), BG)
    draw = ImageDraw.Draw(img)
    for y, line in enumerate(rows):
        for x, ch in enumerate(line):
            if ch == " ":
                continue
            color = ACCENT if ch in "╭╮╰╯─│›" else (MUTED if x > 100 and False else FG)
            draw.text((pad + x * advance, pad + y * line_h), ch, font=mono, fill=color)
    OUT.parent.mkdir(exist_ok=True)
    img.save(OUT)
    print(f"wrote {OUT} ({img.width}x{img.height})")


if __name__ == "__main__":
    sys.exit(main())
