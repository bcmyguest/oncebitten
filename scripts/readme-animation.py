#!/usr/bin/env python3
"""Render docs/media/oncebitten.gif and oncebitten-dark.gif: a mistake made twice becomes a proven rule the pre-push gate enforces."""
import math
import os
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
OUT = {"light": ROOT / "docs/media/oncebitten.gif", "dark": ROOT / "docs/media/oncebitten-dark.gif"}
# DMSans.ttf from github.com/google/fonts (ofl/dmsans); OB_FONT_DIR overrides the directory.
FONT_DIR = Path(os.environ.get("OB_FONT_DIR", Path.home() / ".cache/oncebitten/fonts"))
SANS_FONTS = [FONT_DIR / "DMSans.ttf", Path("/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf")]
MONO_FONTS = [
    Path.home() / ".local/share/fonts/nerd-fonts/JetBrainsMonoNerdFontMono-Bold.ttf",
    FONT_DIR / "JetBrainsMono-Bold.ttf",
    Path("/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf"),
]

W, H = 1068, 536
SS = 2
FRAME_MS = 100
HOLD_FRAMES = 15

THEMES = {
    "light": {
        "outside": (245, 245, 249), "bg": (236, 239, 248), "grid": (214, 218, 236), "frame": (225, 227, 238),
        "card": (252, 253, 253), "card_hl": (248, 248, 253), "border": (214, 218, 236), "accent": (69, 81, 165),
        "text": (26, 28, 41), "muted": (85, 91, 114), "orange": (246, 234, 217), "brown": (184, 135, 95),
        "bad": (168, 72, 52), "ok": (46, 122, 84), "note": (232, 234, 249), "note_text": (54, 59, 99),
    },
    "dark": {
        "outside": (14, 16, 23), "bg": (17, 20, 32), "grid": (38, 43, 69), "frame": (52, 58, 84),
        "card": (20, 23, 38), "card_hl": (28, 32, 47), "border": (48, 54, 80), "accent": (170, 180, 245),
        "text": (236, 238, 247), "muted": (180, 185, 208), "orange": (58, 46, 28), "brown": (174, 156, 134),
        "bad": (236, 150, 120), "ok": (128, 204, 160), "note": (38, 45, 85), "note_text": (180, 187, 232),
    },
}

LX, LW = 37, 360
RX, RW = 470, 561
CH, PITCH = 32, 38
HEAD_ROW, LINE_ROW = 30, 20
BLOCK_GAP = 12

SESSION_Y = {"A": 100, "B": 240}
PILL_W = 118
CAPTION_Y = 494

# Frame timeline: one constant per storyboard beat.
T_A_CODE, T_A_FIX, T_A_CAP = 0, 6, 12
T_B_CODE, T_B_FIX, T_B_CAP = 24, 30, 36
T_EVID = 46
T_RULE = 60
T_APPROVE = 70
T_REPLAY = 82
T_BAD, T_FIXED, T_CORPUS, T_PROVEN = 86, 90, 94, 100
T_PUSH = 112
T_FINDING, T_EXIT = 117, 122
T_FINAL = 132
N_FRAMES = 140 + HOLD_FRAMES

CAPTIONS = [
    (T_A_CODE, "Session A: the user corrects the agent. Hooks capture it."),
    (T_B_CODE, "Session B, another day: the same mistake. Captured again."),
    (T_EVID, "The same correction in 2 sessions becomes evidence."),
    (T_RULE, "oncebitten proposes a rule. The user approves it."),
    (T_REPLAY, "Replay proves the rule in an OpenShell sandbox."),
    (T_PUSH, "git push: the pre-push gate blocks the mistake."),
]
FINAL_CAPTION = "Mistake twice  →  rule  →  proof  →  gate"


def load_font(paths, size, weight=None):
    """First loadable font from paths at size, with the variable weight axis set when present."""
    for p in paths:
        if p.exists():
            f = ImageFont.truetype(str(p), size * SS)
            if weight is not None:
                try:
                    f.set_variation_by_axes([min(max(size, 9), 40), weight])
                except OSError:
                    pass
            return f
    sys.exit(f"no font found in {paths}")


FONTS = {
    "card": load_font(SANS_FONTS, 17, 560), "tag": load_font(SANS_FONTS, 13, 600),
    "head": load_font(SANS_FONTS, 16, 700), "label": load_font(SANS_FONTS, 15, 400),
    "caption": load_font(SANS_FONTS, 17, 500), "final": load_font(SANS_FONTS, 23, 700),
    "mono": load_font(MONO_FONTS, 15), "mono_s": load_font(MONO_FONTS, 14),
}


def ease(t):
    """Smoothstep on [0, 1]."""
    t = max(0.0, min(1.0, t))
    return t * t * (3 - 2 * t)


def ramp(f, start, length=4):
    """0 before start, eased to 1 over length frames."""
    return ease((f - start) / length)


def lerp(a, b, t):
    return a + (b - a) * t


def mix(c1, c2, t):
    return tuple(round(lerp(a, b, t)) for a, b in zip(c1, c2))


def bezier(p0, p1, p2, p3, t):
    u = 1 - t
    return tuple(u ** 3 * a + 3 * u * u * t * b + 3 * u * t * t * c + t ** 3 * d for a, b, c, d in zip(p0, p1, p2, p3))


class Canvas:
    """Supersampled frame; all public coordinates are in output pixels."""

    def __init__(self, theme):
        self.c = THEMES[theme]
        self.img = Image.new("RGBA", (W * SS, H * SS), self.c["outside"] + (255,))

    def layer(self, alpha, fn):
        """Draw fn onto a transparent layer and composite it at alpha."""
        if alpha <= 0.01:
            return
        lay = Image.new("RGBA", self.img.size, (0, 0, 0, 0))
        fn(ImageDraw.Draw(lay))
        if alpha < 0.99:
            lay.putalpha(lay.getchannel("A").point(lambda a: round(a * alpha)))
        self.img.alpha_composite(lay)

    def background(self):
        d = ImageDraw.Draw(self.img)
        d.rounded_rectangle([0, 0, W * SS - 1, H * SS - 1], 16 * SS, fill=self.c["bg"], outline=self.c["frame"], width=3)
        for x in range(36, W - 4, 36):
            d.rectangle([x * SS, 3 * SS, x * SS + 1, (H - 3) * SS], fill=self.c["grid"])
        for y in range(36, H - 4, 36):
            d.rectangle([3 * SS, y * SS - 1, (W - 3) * SS, y * SS + 1], fill=self.c["grid"])

    def box(self, d, x, y, w, h, fill, outline, width=1.0, r=8):
        d.rounded_rectangle([x * SS, y * SS, (x + w) * SS, (y + h) * SS], r * SS, fill=fill, outline=outline,
                            width=max(1, round(width * SS)))

    def text(self, d, x, y, s, font, fill, anchor="lm", tracking=0.0):
        if not tracking:
            d.text((x * SS, y * SS), s, font=FONTS[font], fill=fill, anchor=anchor)
            return
        for ch in s:
            d.text((x * SS, y * SS), ch, font=FONTS[font], fill=fill, anchor=anchor)
            x += FONTS[font].getlength(ch) / SS + tracking

    def dashed_curve(self, d, p0, p3, color, progress=1.0, bend=0.5):
        """Dashed cubic curve from p0 toward p3, drawn up to progress."""
        dx = p3[0] - p0[0]
        p1, p2 = (p0[0] + dx * bend, p0[1]), (p3[0] - dx * bend, p3[1])
        n = 160
        pts = [bezier(p0, p1, p2, p3, i / n) for i in range(int(n * progress) + 1)]
        for i in range(0, len(pts) - 1, 4):
            seg = pts[i:i + 3]
            if len(seg) > 1:
                d.line([(px * SS, py * SS) for px, py in seg], fill=color, width=round(2.2 * SS))
        if progress >= 0.999 and len(pts) > 2:
            self.arrowhead(d, pts[-3], pts[-1], color)

    def arrowhead(self, d, a, b, color):
        ang = math.atan2(b[1] - a[1], b[0] - a[0])
        pts = [b] + [(b[0] - 9 * math.cos(ang + s), b[1] - 9 * math.sin(ang + s)) for s in (0.45, -0.45)]
        d.polygon([(px * SS, py * SS) for px, py in pts], fill=color)

    def final(self):
        return self.img.convert("RGB").resize((W, H), Image.LANCZOS)


def heading(cv, x, title, y=36):
    cv.layer(1.0, lambda d: cv.text(d, x, y, title, "head", cv.c["muted"], tracking=2.4))


def card(cv, alpha, x, y, label, tag=None, font="card", fill=None, outline=None, dx=0.0):
    """One session card with an optional right-aligned tag."""
    c = cv.c

    def draw(d):
        cv.box(d, x + dx, y, LW, CH, fill or c["card"], outline or c["border"], 1.5 if outline else 1.0)
        cv.text(d, x + dx + 15, y + CH / 2, label, font, c["text"])
        if tag:
            cv.text(d, x + dx + LW - 11, y + CH / 2 + 1, tag, "tag", c["muted"], anchor="rm")

    cv.layer(alpha, draw)


def pill(cv, alpha, x, y, w, label, color, fill):
    def draw(d):
        cv.box(d, x, y - 12, w, 24, fill, color, 1.5, r=12)
        cv.text(d, x + w / 2, y, label, "tag", color, anchor="mm")

    cv.layer(alpha, draw)


def session(cv, f, key, sub, t_code, t_fix, t_cap):
    c = cv.c
    y = SESSION_Y[key]

    def draw_label(d):
        cv.text(d, LX, y, f"SESSION {key}", "head", c["text"], tracking=1.6)
        if sub:
            cv.text(d, LX + 112, y, sub, "label", c["muted"])

    cv.layer(ramp(f, t_code, 3), draw_label)
    a = ramp(f, t_code + 1, 3)
    card(cv, a, LX, y + 18, "stamp = datetime.utcnow()", "src/app.py", font="mono", dx=10 * (1 - a))
    b = ramp(f, t_fix, 3)
    card(cv, b, LX, y + 18 + PITCH, "“use datetime.now(UTC)”", "user", fill=c["orange"], outline=c["brown"],
         dx=10 * (1 - b))
    pill(cv, ramp(f, t_cap, 3), LX + LW - PILL_W, y, PILL_W, "hooks captured", c["accent"], c["card_hl"])


def block(cv, alpha, y, rows, cmd=None, title=None, tag=None, fill=None, outline=None, extra=None):
    """A terminal-style block: a command or title row, then mono output rows of (text, color, alpha)."""
    c = cv.c
    h = HEAD_ROW + LINE_ROW * len(rows) + 8

    def draw(d):
        cv.box(d, RX, y, RW, h, fill or c["card"], outline or c["border"], 1.5 if outline else 1.0)
        if cmd:
            cv.text(d, RX + 14, y + HEAD_ROW / 2 + 2, "$ " + cmd, "mono", c["accent"])
        if title:
            cv.text(d, RX + 14, y + HEAD_ROW / 2 + 2, title, "card", c["text"])
        if tag:
            cv.text(d, RX + RW - 12, y + HEAD_ROW / 2 + 3, tag, "tag", c["muted"], anchor="rm")

    cv.layer(alpha, draw)
    for i, (s, color, a) in enumerate(rows):
        cv.layer(alpha * a, lambda d, s=s, color=color, i=i:
                 cv.text(d, RX + 18, y + HEAD_ROW + 8 + i * LINE_ROW, s, "mono_s", color))
    if extra:
        cv.layer(alpha, lambda d: extra(d, y))
    return y + h + BLOCK_GAP


def right_column(cv, f):
    c = cv.c
    heading(cv, RX, "ONCEBITTEN")
    y = 76
    y = block(cv, ramp(f, T_EVID + 4, 3), y, [("…  sessions=2  occurrences=2  …", c["text"], ramp(f, T_EVID + 7, 3))],
              cmd="oncebitten evidence list", tag="seen in 2 sessions")

    approved = ramp(f, T_APPROVE, 3)
    flash = ramp(f, T_APPROVE, 2) * (1 - ramp(f, T_APPROVE + 10, 4))

    def status_chip(d, top):
        w = 150
        x = RX + RW - w - 12
        col = mix(c["muted"], c["accent"], approved)
        cv.box(d, x, top + 4, w, 22, mix(c["card"], c["note"], approved), col, 1.5, r=11)
        label = "proposed  →  active" if approved > 0.5 else "status: proposed"
        cv.text(d, x + w / 2, top + 15, label, "tag", col, anchor="mm")

    rule_fill = mix(c["card"], c["card_hl"], flash)
    rule_outline = mix(c["border"], c["accent"], max(flash, approved * 0.6)) if approved > 0.01 else None
    y = block(cv, ramp(f, T_RULE, 3), y,
              [("engine: ast-grep    check.yml  pattern: datetime.utcnow()", c["text"], ramp(f, T_RULE + 3, 3))],
              title="py-utcnow", fill=rule_fill, outline=rule_outline, extra=status_chip)

    y = block(cv, ramp(f, T_REPLAY, 3), y, [
        ("fixtures/bad.py      fires", c["bad"], ramp(f, T_BAD, 3)),
        ("fixtures/fixed.py    clean", c["ok"], ramp(f, T_FIXED, 3)),
        ("corpus/              clean", c["ok"], ramp(f, T_CORPUS, 3)),
        ("py-utcnow: proven; wrote …/py-utcnow/proof.json", c["text"], ramp(f, T_PROVEN, 3)),
    ], cmd="oncebitten replay py-utcnow", tag="OpenShell sandbox")

    block(cv, ramp(f, T_PUSH, 3), y, [
        ("py-utcnow: src/app.py:2: use datetime.now(UTC)", c["bad"], ramp(f, T_FINDING, 3)),
        ("exit 1", c["bad"], ramp(f, T_EXIT, 3)),
    ], cmd="git push", tag="pre-push gate: blocked" if f >= T_FINDING else "pre-push gate",
        fill=mix(c["card"], c["orange"], ramp(f, T_FINDING, 3)),
        outline=c["brown"] if f >= T_FINDING else None)


def arrows(cv, f):
    c = cv.c
    target = (RX - 8, 76 + (HEAD_ROW + LINE_ROW + 8) / 2)
    for key in ("A", "B"):
        p0 = (LX + LW + 6, SESSION_Y[key])
        prog = ramp(f, T_EVID, 6)
        cv.layer(min(1.0, prog * 4), lambda d, p0=p0, prog=prog: cv.dashed_curve(d, p0, target, c["brown"], prog))


def caption(cv, f):
    c = cv.c
    final = ramp(f, T_FINAL, 5)
    for i, (start, s) in enumerate(CAPTIONS):
        end = CAPTIONS[i + 1][0] if i + 1 < len(CAPTIONS) else T_FINAL
        a = ramp(f, start, 3) * (1 - ramp(f, end - 2, 2))
        cv.layer(a, lambda d, s=s: cv.text(d, W / 2, CAPTION_Y, s, "caption", c["muted"], anchor="mm"))

    def draw_final(d):
        tw = FONTS["final"].getlength(FINAL_CAPTION) / SS
        cv.box(d, W / 2 - tw / 2 - 22, CAPTION_Y - 21, tw + 44, 42, c["note"], c["accent"], 2, r=21)
        cv.text(d, W / 2, CAPTION_Y, FINAL_CAPTION, "final", c["note_text"], anchor="mm")

    cv.layer(final, draw_final)


def render(theme, f):
    cv = Canvas(theme)
    cv.background()
    heading(cv, LX, "AGENT SESSIONS")
    session(cv, f, "A", None, T_A_CODE, T_A_FIX, T_A_CAP)
    session(cv, f, "B", "another day", T_B_CODE, T_B_FIX, T_B_CAP)
    right_column(cv, f)
    arrows(cv, f)
    caption(cv, f)
    return cv.final()


def save_gif(frames, path):
    """Quantize all frames to one shared palette so unchanged regions stay identical between frames."""
    sample = Image.new("RGB", (W, H * 4))
    for i, f in enumerate((T_EVID + 10, T_APPROVE + 4, T_PUSH + 14, N_FRAMES - 1)):
        sample.paste(frames[f], (0, H * i))
    ref = sample.quantize(colors=255, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)
    pal = [fr.quantize(palette=ref, dither=Image.Dither.NONE) for fr in frames]
    pal[0].save(path, save_all=True, append_images=pal[1:], duration=FRAME_MS, loop=0, optimize=True, disposal=1)


def main():
    preview = sys.argv[1] if len(sys.argv) > 1 else None
    for theme, path in OUT.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        frames = [render(theme, f) for f in range(N_FRAMES)]
        if preview:
            for f in range(N_FRAMES):
                frames[f].save(Path(preview) / f"{theme}_{f:03d}.png")
        save_gif(frames, path)
        print(path, len(frames))


if __name__ == "__main__":
    main()
