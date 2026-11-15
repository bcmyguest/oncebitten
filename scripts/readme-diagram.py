#!/usr/bin/env python3
"""Render docs/media/oncebitten.svg and oncebitten-dark.svg: a caught mistake becomes an approved rule that git enforces."""
from pathlib import Path
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parent.parent
OUT = {"light": ROOT / "docs/media/oncebitten.svg", "dark": ROOT / "docs/media/oncebitten-dark.svg"}

W, H = 1068, 536
SANS = "'DM Sans', ui-sans-serif, system-ui, -apple-system, 'Segoe UI', Helvetica, Arial, sans-serif"
MONO = "'JetBrains Mono', 'JetBrainsMono Nerd Font Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace"

THEMES = {
    "light": {
        "bg": (236, 239, 248), "grid": (214, 218, 236), "frame": (225, 227, 238),
        "card": (252, 253, 253), "card_hl": (248, 248, 253), "border": (214, 218, 236), "accent": (69, 81, 165),
        "text": (26, 28, 41), "muted": (85, 91, 114), "orange": (246, 234, 217), "brown": (184, 135, 95),
        "bad": (168, 72, 52), "ok": (46, 122, 84), "note": (232, 234, 249), "note_text": (54, 59, 99),
        "dot": (226, 128, 48), "faint": (140, 145, 165),
    },
    "dark": {
        "bg": (17, 20, 32), "grid": (38, 43, 69), "frame": (52, 58, 84),
        "card": (20, 23, 38), "card_hl": (28, 32, 47), "border": (48, 54, 80), "accent": (170, 180, 245),
        "text": (236, 238, 247), "muted": (180, 185, 208), "orange": (58, 46, 28), "brown": (174, 156, 134),
        "bad": (236, 150, 120), "ok": (128, 204, 160), "note": (38, 45, 85), "note_text": (180, 187, 232),
        "dot": (240, 150, 70), "faint": (120, 126, 150),
    },
}

# Column geometry: x, width.
C1, C2, C3 = (14, 300), (342, 396), (766, 288)
TOP = 116
HEAD_Y = 98
CAPTION_Y = 456
LINE = 25

CAPTIONS = [
    (C1, ["A correction, a revert,", "a failing test: each one counts."]),
    (C2, ["Proposes the strongest rule that fits.", "You approve."]),
    (C3, ["Every later violation", "stops at git."]),
]
HEADLINE = "Caught. Approved. Enforced."
TITLE = "oncebitten: caught, approved, enforced"
DESC = ("Three columns. Caught: a Claude correction, a Codex revert and a failing Cursor test each count as evidence. "
        "Rule: oncebitten proposes the strongest rule that fits, a check over an instruction over nothing, and you "
        "approve it. Enforced: git push, CI and pull requests stop on py-utcnow: use datetime.now(UTC).")


def hexc(rgb):
    return "#%02x%02x%02x" % rgb


class Svg:
    """Accumulates SVG elements with the theme colors resolved by name."""

    def __init__(self, theme):
        self.c = {k: hexc(v) for k, v in THEMES[theme].items()}
        self.parts = []

    def add(self, s):
        self.parts.append(s)

    def rect(self, x, y, w, h, fill, stroke=None, r=8, sw=1.0, extra=""):
        st = f' stroke="{self.c[stroke]}" stroke-width="{sw}"' if stroke else ""
        self.add(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" fill="{self.c[fill]}"{st}{extra}/>')

    def text(self, x, y, s, color, size=20, mono=False, weight=None, anchor="start", spacing=None, raw=False):
        """Text with its vertical center at y; raw=True passes s through unescaped (for tspans)."""
        attrs = f'x="{x}" y="{y}" fill="{self.c[color]}" font-size="{size}" dominant-baseline="central"'
        attrs += f' font-family="{MONO if mono else SANS}"'
        if weight:
            attrs += f' font-weight="{weight}"'
        if anchor != "start":
            attrs += f' text-anchor="{anchor}"'
        if spacing:
            attrs += f' letter-spacing="{spacing}"'
        self.add(f"<text {attrs}>{s if raw else escape(s)}</text>")

    def tspan(self, s, color, mono=False, weight=None):
        w = f' font-weight="{weight}"' if weight else ""
        fam = f' font-family="{MONO if mono else SANS}" font-size="{18 if mono else 20}"'
        return f'<tspan fill="{self.c[color]}"{fam}{w}>{escape(s)}</tspan>'

    def line(self, x1, y1, x2, y2, color, sw=2.0, extra=""):
        self.add(f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{self.c[color]}" stroke-width="{sw}"{extra}/>')

    def dot(self, x, y, r=6):
        self.add(f'<circle cx="{x}" cy="{y}" r="{r}" fill="{self.c["dot"]}"/>')

    def arrowhead(self, x, y, direction, color, size=9):
        """Filled triangle with its tip at (x, y), pointing right or up."""
        if direction == "right":
            pts = [(x, y), (x - size, y - size * 0.6), (x - size, y + size * 0.6)]
        else:
            pts = [(x, y), (x - size * 0.6, y + size), (x + size * 0.6, y + size)]
        self.add(f'<polygon points="{" ".join(f"{px:g},{py:g}" for px, py in pts)}" fill="{self.c[color]}"/>')

    def lock(self, x, y, color):
        """Padlock icon, 18 wide, top-left at (x, y)."""
        c = self.c[color]
        self.add(f'<path d="M{x + 4} {y + 10} v-3 a5 5 0 0 1 10 0 v3" fill="none" stroke="{c}" stroke-width="2.4"/>')
        self.add(f'<rect x="{x}" y="{y + 9}" width="18" height="14" rx="3" fill="{c}"/>')
        self.add(f'<rect x="{x + 8}" y="{y + 13}" width="2.4" height="6" rx="1.2" fill="{self.c["card"]}"/>')

    def render(self):
        head = (f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" '
                f'role="img" aria-labelledby="t d">\n<title id="t">{escape(TITLE)}</title>\n<desc id="d">{escape(DESC)}</desc>\n')
        return head + "\n".join(self.parts) + "\n</svg>\n"


def background(s):
    s.rect(1, 1, W - 2, H - 2, "bg", "frame", r=16, sw=2)
    grid = [f"M{x} 3V{H - 3}" for x in range(36, W - 4, 36)] + [f"M3 {y}H{W - 3}" for y in range(36, H - 4, 36)]
    s.add(f'<path d="{" ".join(grid)}" stroke="{s.c["grid"]}" stroke-width="1"/>')


def headline(s):
    w = 420
    s.rect((W - w) / 2, 26, w, 46, "note", "accent", r=23, sw=2)
    s.text(W / 2, 49, HEADLINE, "note_text", size=26, weight=700, anchor="middle")


def column_heads(s):
    for (x, _), label in zip((C1, C2, C3), ("CAUGHT", "RULE", "ENFORCED")):
        s.text(x + 2, HEAD_Y, label, "muted", weight=700, spacing=2.4)


def trigger_card(s, y, h, agent, tag=None):
    x, w = C1
    s.rect(x, y, w, h, "card", "border")
    s.dot(x + 20, y + 22)
    s.text(x + 34, y + 22, agent, "text", weight=600)
    if tag:
        s.text(x + w - 14, y + 22, tag, "muted", size=18, mono=True, anchor="end")


def caught(s):
    x, w = C1
    y = TOP
    trigger_card(s, y, 112, "Claude")
    s.text(x + 15, y + 50, "stamp = datetime.utcnow()", "text", size=18, mono=True)
    bw = 256
    s.rect(x + w - bw - 12, y + 66, bw, 34, "orange", "brown", r=17, sw=1.5)
    s.text(x + w - 12 - bw / 2, y + 83, "“use datetime.now(UTC)”", "text", anchor="middle")

    y += 112 + 12
    trigger_card(s, y, 88, "Codex")
    s.text(x + 15, y + 50, "git revert a1b2c3", "text", size=18, mono=True)
    s.text(x + 15, y + 72, "utcnow → now(UTC)", "muted", size=18, mono=True)

    y += 88 + 12
    trigger_card(s, y, 88, "Cursor", tag="pytest")
    s.text(x + 15, y + 50, "FAILED test_stamp_is_utc", "bad", size=18, mono=True, weight=700)
    s.text(x + 15, y + 72, "PASSED", "ok", size=18, mono=True, weight=700)


def rule(s):
    x, w = C2
    lane = x + 20
    rx, rw = x + 40, w - 40
    check_y, check_h = TOP + 62, 64
    instr_y, instr_h = check_y + check_h + 14, 64
    none_y = instr_y + instr_h + 30

    # Ladder rails behind the rungs.
    for rail_x in (rx + 10, rx + rw - 10):
        s.line(rail_x, check_y, rail_x, none_y + 16, "border", sw=3)

    # Rule card sitting on the CHECK rung.
    cw = 270
    cx = rx + rw - cw - 16
    s.rect(cx, TOP + 2, cw, check_y - TOP - 2 + 8, "card_hl", "accent", r=8, sw=1.5)
    s.text(cx + 14, TOP + 20, "py-utcnow", "accent", size=18, mono=True, weight=700)
    s.text(cx + 14, TOP + 44, s.tspan("flags ", "muted") + s.tspan("datetime.utcnow()", "text", mono=True),
           "text", raw=True)

    # CHECK: strongest, lit.
    s.rect(rx, check_y, rw, check_h, "note", "accent", sw=2.5)
    s.lock(rx + 16, check_y + 9, "accent")
    s.text(rx + 44, check_y + 20, "CHECK", "accent", weight=700, spacing=1.5)
    s.text(rx + 16, check_y + 46, "ast-grep · linter · diff", "text", size=18, mono=True)

    # INSTRUCTION: dashed, half opacity.
    s.add('<g opacity="0.5">')
    s.rect(rx, instr_y, rw, instr_h, "card", "muted", sw=1.5, extra=' stroke-dasharray="6 5"')
    s.text(rx + 16, instr_y + 20, "INSTRUCTION", "text", weight=700, spacing=1.5)
    s.text(rx + 16, instr_y + 45, "markdown, the agent may ignore it", "muted")
    s.add("</g>")

    # NOTHING: grey text only.
    s.text(rx + 16, none_y, s.tspan("NOTHING", "faint", weight=700) + s.tspan("  it happens again", "faint"),
           "faint", spacing=None, raw=True)

    # Dotted climb from NOTHING to CHECK.
    mid = check_y + check_h / 2
    dash = ' stroke-dasharray="2 6" stroke-linecap="round" fill="none"'
    s.add(f'<path d="M{rx + 6} {none_y} H{lane} V{mid} H{rx - 2}" stroke="{s.c["accent"]}" stroke-width="2.5"{dash}/>')
    s.arrowhead(lane, instr_y + instr_h / 2 - 4, "up", "accent")
    s.arrowhead(lane, check_y + check_h + 4, "up", "accent")
    s.arrowhead(rx - 1, mid, "right", "accent")

    # Approval and proof.
    s.text(rx + 2, none_y + 34, "✓ approved by you", "ok", weight=700)
    s.text(rx + 2, none_y + 34 + LINE,
           s.tspan("proven: ", "muted") + s.tspan("fires on bad", "bad") + s.tspan(" · ", "muted")
           + s.tspan("clean on fixed", "ok"), "muted", raw=True)
    return mid


def enforced(s):
    x, w = C3
    y = TOP
    for label in ("git push", "CI", "pull request"):
        s.rect(x, y, w, 52, "orange", "brown", sw=1.5)
        s.text(x + 14, y + 26, label, "text", weight=600)
        s.text(x + w - 14, y + 26, "✗ py-utcnow", "bad", size=18, mono=True, weight=700, anchor="end")
        y += 52 + 12
    y += 14
    s.rect(x, y, w, 52, "note", "accent", sw=1.5)
    s.text(x + w / 2, y + 26, "use datetime.now(UTC)", "note_text", size=18, mono=True, weight=700, anchor="middle")


def arrow(s, x1, x2, y):
    s.line(x1 + 4, y, x2 - 10, y, "accent", sw=3)
    s.arrowhead(x2 - 2, y, "right", "accent", size=11)


def captions(s):
    for (x, w), lines in CAPTIONS:
        for i, ln in enumerate(lines):
            s.text(x + 2, CAPTION_Y + i * LINE, ln, "muted", weight=500)


def build(theme):
    s = Svg(theme)
    background(s)
    headline(s)
    column_heads(s)
    caught(s)
    check_mid = rule(s)
    enforced(s)
    arrow(s, C1[0] + C1[1], C2[0], TOP + 112 + 12 + 44)
    arrow(s, C2[0] + C2[1], C3[0], check_mid)
    captions(s)
    return s.render()


def main():
    for theme, path in OUT.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(build(theme), encoding="utf-8")
        print(f"wrote {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
