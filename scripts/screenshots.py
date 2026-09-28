#!/usr/bin/env python3
"""Rasterize sparktop cell dumps (examples/screenshots.rs) into PNG / GIF.

    cargo run --release --example screenshots        # -> target/screenshots
    python3 scripts/screenshots.py target/screenshots docs/img

Braille, block, meter and box-drawing glyphs are drawn geometrically so the
images look like a real terminal regardless of which font is installed; the
remaining text uses a monospace font (JetBrains Mono if found, override with
SPARKTOP_FONT / SPARKTOP_FONT_BOLD). Requires Pillow.
"""
import json
import os
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

FONT_CANDIDATES = [
    "~/Library/Fonts/JetBrainsMonoNerdFont-{w}.ttf",
    "/Library/Fonts/JetBrainsMonoNerdFont-{w}.ttf",
    "~/Library/Fonts/JetBrainsMono-{w}.ttf",
    "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-{w}.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono{d}.ttf",
]


def find_font(weight):
    env = os.environ.get("SPARKTOP_FONT_BOLD" if weight == "Bold" else "SPARKTOP_FONT")
    if env:
        return env
    for c in FONT_CANDIDATES:
        p = Path(os.path.expanduser(c.format(w=weight, d="-Bold" if weight == "Bold" else "")))
        if p.exists():
            return str(p)
    sys.exit("no monospace font found; set SPARKTOP_FONT")


def hex_rgb(h):
    return tuple(int(h[i : i + 2], 16) for i in (1, 3, 5))


def mix(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


class Renderer:
    def __init__(self, scale):
        self.s = scale
        size = 13 * scale
        self.font = ImageFont.truetype(find_font("Regular"), size)
        self.bold = ImageFont.truetype(find_font("Bold"), size)
        self.cw = round(self.font.getlength("M"))
        self.ch = round(size * 1.28)
        asc, desc = self.font.getmetrics()
        self.base = round((self.ch + asc - desc) / 2)

    def glyph(self, d, ch, x, y, fg, bold):
        cw, chh, s = self.cw, self.ch, self.s
        o = ord(ch) if len(ch) == 1 else 0
        if 0x2800 <= o <= 0x28FF:
            bits = o - 0x2800
            dots = [(0, 0, 0x01), (0, 1, 0x02), (0, 2, 0x04), (1, 0, 0x08), (1, 1, 0x10), (1, 2, 0x20), (0, 3, 0x40), (1, 3, 0x80)]
            r = max(1.0, cw * 0.17)
            for cx, cy, bit in dots:
                if bits & bit:
                    px = x + cw * (0.28 + 0.44 * cx)
                    py = y + chh * (0.125 + 0.25 * cy)
                    d.ellipse([px - r, py - r, px + r, py + r], fill=fg)
            return
        if 0x2581 <= o <= 0x2588:
            frac = (o - 0x2580) / 8
            d.rectangle([x, y + chh * (1 - frac), x + cw - 1, y + chh - 1], fill=fg)
            return
        if ch == "■":
            side = cw * 0.72
            px, py = x + (cw - side) / 2, y + (chh - side) / 2
            d.rounded_rectangle([px, py, px + side, py + side], radius=s, fill=fg)
            return
        mx, my, lw = x + cw // 2, y + chh // 2, max(1, s)
        if ch in "─━":
            w = lw * (2 if ch == "━" else 1)
            d.rectangle([x, my - w // 2, x + cw, my - w // 2 + w - 1], fill=fg)
            return
        if ch == "│":
            d.rectangle([mx - lw // 2, y, mx - lw // 2 + lw - 1, y + chh], fill=fg)
            return
        if ch in "╭╮╰╯":
            r = min(cw, chh) // 2
            right = ch in "╭╰"  # line continues to the right
            down = ch in "╭╮"
            hx0, hx1 = (mx + r, x + cw) if right else (x, mx - r)
            vy0, vy1 = (my + r, y + chh) if down else (y, my - r)
            d.rectangle([hx0, my - lw // 2, hx1, my - lw // 2 + lw - 1], fill=fg)
            d.rectangle([mx - lw // 2, vy0, mx - lw // 2 + lw - 1, vy1], fill=fg)
            bx = mx if right else mx - 2 * r
            by = my if down else my - 2 * r
            start = {"╭": 180, "╮": 270, "╰": 90, "╯": 0}[ch]
            d.arc([bx, by, bx + 2 * r, by + 2 * r], start, start + 90, fill=fg, width=lw)
            return
        if ch.strip():
            d.text((x, y + self.base), ch, font=self.bold if bold else self.font, fill=fg, anchor="ls")

    def grid(self, doc):
        w, h = doc["w"], doc["h"]
        img = Image.new("RGB", (w * self.cw, h * self.ch), hex_rgb(doc["bg"]))
        d = ImageDraw.Draw(img)
        for r, row in enumerate(doc["rows"]):
            for c, (sym, fg, bg, bold) in enumerate(row):
                x, y = c * self.cw, r * self.ch
                if bg != doc["bg"]:
                    d.rectangle([x, y, x + self.cw - 1, y + self.ch - 1], fill=hex_rgb(bg))
                self.glyph(d, sym, x, y, hex_rgb(fg), bold)
        return img

    def window(self, doc, title="sparktop"):
        """Terminal in a macOS-style window with a drop shadow."""
        s = self.s
        term = self.grid(doc)
        bg = hex_rgb(doc["bg"])
        bar, pad, margin = 28 * s, 10 * s, 24 * s
        ww, wh = term.width + 2 * pad, term.height + bar + pad
        canvas = Image.new("RGBA", (ww + 2 * margin, wh + 2 * margin), (0, 0, 0, 0))
        shadow = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
        ds = ImageDraw.Draw(shadow)
        for i in range(margin, 0, -2 * s):
            a = int(60 * (1 - i / margin) ** 2)
            ds.rounded_rectangle([margin - i, margin - i + 6 * s, margin + ww + i, margin + wh + i + 6 * s], radius=12 * s + i, fill=(0, 0, 0, a))
        canvas = Image.alpha_composite(canvas, shadow)
        d = ImageDraw.Draw(canvas)
        frame = mix(bg, (255, 255, 255), 0.06)
        d.rounded_rectangle([margin, margin, margin + ww, margin + wh], radius=12 * s, fill=bg + (255,), outline=mix(bg, (255, 255, 255), 0.14) + (255,), width=s)
        d.rounded_rectangle([margin + s, margin + s, margin + ww - s, margin + bar], radius=11 * s, fill=frame + (255,))
        d.rectangle([margin + s, margin + bar - 12 * s, margin + ww - s, margin + bar], fill=frame + (255,))
        for i, col in enumerate([(255, 95, 86), (255, 189, 46), (39, 201, 63)]):
            cx, cy, r = margin + (18 + 20 * i) * s, margin + bar // 2, 6 * s
            d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=col + (255,))
        tw = self.font.getlength(title)
        d.text((margin + (ww - tw) / 2, margin + bar // 2), title, font=self.font, fill=mix(bg, (255, 255, 255), 0.55) + (255,), anchor="lm")
        canvas.paste(term, (margin + pad, margin + bar))
        return canvas


def load(p):
    return json.loads(Path(p).read_text())


def main():
    src, out = Path(sys.argv[1]), Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    hi = Renderer(2)
    stills = ["cluster-panels", "cluster-table", "cluster-compare", "node", "vllm", "picker"]
    for name in stills:
        hi.window(load(src / f"{name}.json")).save(out / f"{name}.png", optimize=True)
        print("wrote", out / f"{name}.png")

    # theme gallery: 3 + 2 grid of smaller windows
    themes = sorted(src.glob("theme-*.json"), key=lambda p: ["gruvbox", "catppuccin", "tokyonight", "nord", "dracula"].index(p.stem[6:]))
    mid = Renderer(1)
    tiles = [mid.window(load(p), title=p.stem[6:]) for p in themes]
    if tiles:
        tw, th = tiles[0].size
        cols = 3
        rows = (len(tiles) + cols - 1) // cols
        sheet = Image.new("RGBA", (tw * cols, th * rows), (0, 0, 0, 0))
        for i, t in enumerate(tiles):
            r, c = divmod(i, cols)
            off = (tw // 2) * (cols - (len(tiles) - r * cols)) if r == rows - 1 else 0
            sheet.paste(t, (c * tw + off, r * th), t)
        sheet.save(out / "themes.png", optimize=True)
        print("wrote", out / "themes.png")

    frames = sorted((src / "gif").glob("*.json"))
    if frames:
        imgs = []
        for p in frames:
            im = mid.window(load(p))
            flat = Image.new("RGB", im.size, (13, 17, 23))  # GitHub dark page
            flat.paste(im, (0, 0), im)
            imgs.append(flat.quantize(colors=128, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE))
        imgs[0].save(out / "demo.gif", save_all=True, append_images=imgs[1:], duration=450, loop=0, optimize=True, disposal=1)
        print("wrote", out / "demo.gif", f"({len(imgs)} frames)")


if __name__ == "__main__":
    main()
