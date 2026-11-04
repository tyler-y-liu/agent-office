"""agent-office's people: chibi figures in hoodies with a staff badge, and
the hairstyles drawn for their heads.

Reads the untouched upstream sprites from the cargo registry copy of
pixtuoid-scene 0.21.0 and writes into vendor/pixtuoid-scene/sprites/default,
so running it twice changes nothing.

Every person frame keeps pixtuoid's body, poses and animation, so seats,
desks and walks still line up; what changes:
  - the head: a big round one at each frame's head mark, with big eyes that
    catch the light, cheeks and a small mouth;
  - a pink lanyard and a white staff badge over the chest (front views);
  - a hoodie: the hood's rim round the neck, and the hood down the back;
  - white-toed sneakers.
The nine hairstyles are drawn anew round that head, in every view the pack
names, so a style reads the same from the front, the back, the side and the
crown of a sleeper's bowed head.

Run from the repo root:  python3 tools/people.py
"""

import glob
import os
import re
import sys

UPSTREAM = glob.glob(
    os.path.expanduser("~/.cargo/registry/src/*/pixtuoid-scene-0.21.0/sprites/default")
)
OUT = "vendor/pixtuoid-scene/sprites/default"

LANYARD = "ǀ"  # pack.toml [palette]: the lanyard's strap
BLUSH = "ǁ"  # pack.toml [palette]: cheeks
CATCH = "w"  # off-white: an eye's catch-light, the badge, a sneaker's toe
BADGE_STRIPE = "c"  # cyan accent: the badge's header stripe
SHIRT = set("BvWU")
HOOD = "U"  # the shirt's deepest shade, so the hood takes each agent's colour
HOOD_LIT = "v"
SKIN, SKIN_SHADE, EYE, MOUTH = "S", "s", "e", "i"
HEAD_KEYS = set("Sseim")
HAIR, HAIR_SHADE, HAIR_LIT, HAIR_SHINE = "H", "h", "Y", "Z"
HAIR_TIE = "r"

PEOPLE = [
    "standing",
    "holding_coffee",
    "seated",
    "seated_back",
    "side_seated",
    "seated_sleeping",
    "seated_sleeping_alt",
    "back_couch",
] + [
    f"{name}_{i}"
    for name, n in [
        ("walking", 8),
        ("walking_back", 8),
        ("walking_coffee", 8),
        ("typing", 2),
        ("typing_back", 2),
    ]
    for i in range(n)
]

# The head, from the frame's head mark: its centre's offset and its radii.
HEAD = {
    "front": ((0, 2), (11, 11)),
    "back": ((0, 2), (11, 11)),
    "side": ((3, 2), (10, 11)),
}
# A hair sprite's own head mark sits this many rows lower than a body's: the
# hair has room over the frame's top.
HAIR_SIZE = (32, 54)
HAIR_MARK = (16, 18)
BODY_MARK_Y = 12
HAIR_DY = HAIR_MARK[1] - BODY_MARK_Y


# --- sprites ----------------------------------------------------------------


def parse(text):
    """Header lines, then frames of (mark lines, grid rows)."""
    header, frames = [], []
    for line in text.splitlines():
        if line.startswith("@frame"):
            frames.append(([line], []))
        elif not frames:
            header.append(line)
        elif line.startswith("@"):
            frames[-1][0].append(line)
        elif line.strip():
            frames[-1][1].append(line.split(" "))
    return header, frames


def render(header, frames):
    out = list(header)
    for marks, grid in frames:
        out += marks
        out += [" ".join(row) for row in grid]
    return "\n".join(out) + "\n"


def at(g, x, y):
    return g[y][x] if 0 <= y < len(g) and 0 <= x < len(g[0]) else None


def put(g, x, y, k, over):
    """Paint `k` at (x, y) when what is there is one of `over`."""
    if at(g, x, y) in over:
        g[y][x] = k


def inside(x, y, cx, cy, rx, ry):
    return ((x + 0.5 - cx) / rx) ** 2 + ((y + 0.5 - cy) / ry) ** 2 <= 1.0


# --- the head ---------------------------------------------------------------


def head(g, view, mark):
    """Wipe pixtuoid's head and draw the big round one in its place."""
    (ox, oy), (rx, ry) = HEAD[view]
    cx, cy = mark[0] + ox, mark[1] + oy
    w, h = len(g[0]), len(g)
    for y in range(min(h, cy + ry - 1)):
        for x in range(w):
            if g[y][x] in HEAD_KEYS | {BLUSH, CATCH}:
                g[y][x] = "."
    for y in range(max(0, cy - ry), min(h, cy + ry + 1)):
        for x in range(w):
            if not inside(x, y, cx, cy, rx, ry):
                continue
            if view == "back":
                k = SKIN_SHADE
            elif view == "side":
                k = SKIN_SHADE if x < cx - 4 else SKIN
            else:
                k = SKIN_SHADE if x >= cx + 6 or y >= cy + ry - 1 else SKIN
            g[y][x] = k
    if view == "front":
        for ex in (cx - 6, cx + 3):
            for dy in range(4):
                for dx in range(3):
                    g[cy + 1 + dy][ex + dx] = EYE
            g[cy + 1][ex] = CATCH
        for bx in (cx - 8, cx - 7, cx + 6, cx + 7):
            put(g, bx, cy + 5, BLUSH, {SKIN, SKIN_SHADE})
        g[cy + 6][cx - 1] = MOUTH
        g[cy + 6][cx] = MOUTH
    elif view == "side":
        ex = cx + 5
        for dy in range(4):
            for dx in range(2):
                g[cy + 1 + dy][ex + dx] = EYE
        g[cy + 1][ex] = CATCH
        put(g, cx + 2, cy + 5, BLUSH, {SKIN})
        put(g, cx + 3, cy + 5, BLUSH, {SKIN})
        put(g, cx + 7, cy + 7, MOUTH, {SKIN})


# --- the outfit -------------------------------------------------------------


def collar_row(g):
    """The first row the shirt shows in: where the neck meets the body."""
    for y, row in enumerate(g):
        if sum(k in SHIRT for k in row) >= 3:
            return y
    return None


def badge(g, cx, top):
    """The lanyard from the collar to a badge on the chest, centred on `cx`."""
    for dy in range(3):
        put(g, cx - 2 + dy // 2, top + dy, LANYARD, SHIRT | {"i"})
        put(g, cx + 1 - dy // 2, top + dy, LANYARD, SHIRT | {"i"})
    card = top + 3
    for dy in range(4):
        for dx in range(-2, 2):
            put(g, cx + dx, card + dy, BADGE_STRIPE if dy == 0 else CATCH, SHIRT)


def hood_rim(g, collar):
    """The hood bunched round the neck, just over the collar."""
    xs = [x for x, k in enumerate(g[collar]) if k in SHIRT or k == "i"]
    if xs:
        for x in range(min(xs), max(xs) + 1):
            put(g, x, collar - 1, HOOD, ".")


def hood_back(g, collar):
    """The hood hanging down the back, from the collar."""
    for dy, half in enumerate([5, 5, 4, 3]):
        for dx in range(-half, half):
            k = HOOD if dy == 3 or abs(dx + 0.5) > half - 1.5 else HOOD_LIT
            put(g, 16 + dx, collar + dy, k, SHIRT | {"i"})


def hood_side(g, collar):
    """The hood behind the neck, seen from the side: the figure faces east."""
    xs = [x for x, k in enumerate(g[collar]) if k in SHIRT or k == "i"]
    if not xs:
        return
    back = min(xs)
    for dy in range(-1, 3):
        put(g, back - 1, collar + dy, HOOD, ".")
        put(g, back, collar + dy, HOOD, SHIRT | {"i", "."})


def sneakers(g):
    for row in g:
        for x, k in enumerate(row):
            if k == ":":
                row[x] = CATCH


def dress(grid, view, mark):
    if view in HEAD:
        head(grid, view, mark)
    collar = collar_row(grid)
    if collar is not None:
        if view == "front":
            hood_rim(grid, collar)
            badge(grid, 16, collar + 1)
        elif view == "back":
            hood_back(grid, collar + 1)
        elif view == "side":
            hood_side(grid, collar)
        # A crown view is a head bowed onto the desk: the body is not shown
        # the way the hood and the badge assume.
    sneakers(grid)


def people(src):
    for name in PEOPLE:
        path = f"{name}@4x.sprite"
        header, frames = parse(open(os.path.join(src, path)).read())
        header = [
            "# agent-office: redrawn by tools/people.py from pixtuoid's art."
            if "gen-art.py" in h
            else h
            for h in header
        ]
        for marks, grid in frames:
            mark = next(
                (m for m in map(re.compile(r"@mark head\.(\w+) (\d+) (\d+)").match, marks) if m),
                None,
            )
            view, pos = ("front", (16, 12)) if mark is None else (
                mark.group(1),
                (int(mark.group(2)), int(mark.group(3))),
            )
            dress(grid, view, pos)
        open(os.path.join(OUT, path), "w").write(render(header, frames))


# --- hair -------------------------------------------------------------------

# Each style: where its fringe ends over the brow (rows from the head's
# centre), how far its sides fall past the centre, how far down the nape it
# covers from behind, and what it adds.
STYLES = {
    "crop": dict(fringe=-6, sides=-2, nape=3),
    "side_part": dict(fringe=-5, sides=1, nape=4, slant=0.35),
    "mop": dict(fringe=-1, sides=4, nape=6, volume=2),
    "messy": dict(fringe=-4, sides=1, nape=5, spikes=True),
    "shaggy": dict(fringe=-2, sides=6, nape=8, jag=True),
    "curls": dict(fringe=-4, sides=3, nape=6, curls=True),
    "long": dict(fringe=-5, sides=8, nape=7, long=True),
    "bun": dict(fringe=-6, sides=0, nape=3, bun=True),
    "ponytail": dict(fringe=-5, sides=0, nape=3, tail=True),
}


def blank():
    w, h = HAIR_SIZE
    return [["."] * w for _ in range(h)]


def shade(g, cx, cy, rx):
    """Light from the west: the east side and the underside turned away."""
    for y, row in enumerate(g):
        for x, k in enumerate(row):
            if k != HAIR:
                continue
            if x >= cx + rx * 0.45 or at(g, x, y + 1) == ".":
                row[x] = HAIR_SHADE
            elif x <= cx - rx * 0.4 and y <= cy - 3:
                row[x] = HAIR_LIT


def fill(g, test):
    for y in range(len(g)):
        for x in range(len(g[0])):
            if test(x, y):
                g[y][x] = HAIR


def hair_front(style):
    s = STYLES[style]
    (_, oy), (rx, ry) = HEAD["front"]
    cx, cy = HAIR_MARK[0], HAIR_MARK[1] + oy
    vol = s.get("volume", 1)
    orx, ory = rx + 1 + vol * 0.5, ry + 1 + vol * 0.5
    slant = s.get("slant", 0)

    def fringe(x):
        f = cy + s["fringe"] + (cx - x) * slant
        if s.get("jag") and x % 3 == 0:
            f += 2
        if s.get("spikes") and x % 4 == 1:
            f += 2
        return f

    def cap(x, y):
        if not inside(x, y, cx, cy - 0.5, orx, ory):
            return False
        edge = abs(x + 0.5 - cx) > rx - 3
        return y < fringe(x) or (edge and y < cy + s["sides"])

    over = blank()
    fill(over, cap)
    if s.get("spikes"):
        for x in range(cx - rx + 4, cx + rx - 4, 4):
            top = cy - ry - 1
            for dy in range(1, 3):
                over[top - dy][x] = HAIR
                over[top - dy][x + 1] = HAIR
    if s.get("curls"):
        for x in range(cx - rx, cx + rx + 1, 5):
            y = int(cy - (ry + 1) * (1 - ((x - cx) / (rx + 1)) ** 2) ** 0.5)
            for yy in range(y - 3, y + 4):
                for xx in range(x - 3, x + 4):
                    if (xx - x) ** 2 + (yy - y) ** 2 <= 9 and yy < cy:
                        over[yy][xx] = HAIR
    if s.get("bun"):
        bx, by = cx, cy - ry - 3
        for yy in range(by - 4, by + 5):
            for xx in range(bx - 4, bx + 5):
                if (xx - bx) ** 2 + (yy - by) ** 2 <= 17:
                    over[yy][xx] = HAIR
        over[by + 3][bx - 1] = HAIR_TIE
        over[by + 3][bx] = HAIR_TIE
    shade(over, cx, cy, rx)
    if style == "side_part":
        for y in range(cy - ry - 1, cy - 5):
            if over[y][cx + 4] != ".":
                over[y][cx + 4] = HAIR_LIT
    if s.get("curls"):
        for x, y in ((cx - 7, cy - 8), (cx + 1, cy - 10), (cx + 6, cy - 7)):
            over[y][x] = HAIR_SHINE

    behind = blank()

    def volume(x, y):
        if s.get("long") and abs(x + 0.5 - cx) <= rx + 2 and cy <= y <= cy + ry + 5:
            return True
        if s["sides"] >= 4 and inside(x, y, cx, cy, orx + 1, ory) and y <= cy + s["sides"]:
            return True
        return inside(x, y, cx, cy - 0.5, orx, ory) and y < cy

    fill(behind, volume)
    shade(behind, cx, cy, rx)
    return over, behind


def hair_back(style):
    s = STYLES[style]
    (_, oy), (rx, ry) = HEAD["back"]
    cx, cy = HAIR_MARK[0], HAIR_MARK[1] + oy
    vol = s.get("volume", 1)
    orx, ory = rx + 1 + vol * 0.5, ry + 1 + vol * 0.5
    over = blank()
    fill(over, lambda x, y: inside(x, y, cx, cy - 0.5, orx, ory) and y < cy + s["nape"])
    if s.get("long"):
        fill(over, lambda x, y: abs(x + 0.5 - cx) <= rx + 1 and cy <= y <= cy + ry + 5)
    if s.get("tail"):
        fill(over, lambda x, y: abs(x + 0.5 - cx) <= 2 and cy <= y <= cy + ry + 6)
    if s.get("bun"):
        fill(over, lambda x, y: (x - cx) ** 2 + (y - (cy - ry + 1)) ** 2 <= 20)
    if s.get("jag"):
        for x in range(cx - rx, cx + rx, 3):
            over[cy + s["nape"]][x] = HAIR
            over[cy + s["nape"] + 1][x] = HAIR
    shade(over, cx, cy, rx)
    if s.get("tail") or s.get("bun"):
        y = cy - 1 if s.get("tail") else cy - ry + 5
        over[y][cx - 1] = HAIR_TIE
        over[y][cx] = HAIR_TIE
    # Behind the body from the back is only what falls past the nape, under
    # the hood's rim, as pixtuoid draws it: anything higher would stand
    # behind the monitor a sitter faces.
    behind = blank()
    nape = cy + s["nape"]
    fill(behind, lambda x, y: abs(x + 0.5 - cx) <= rx - 2 and nape <= y < nape + 3)
    shade(behind, cx, cy, rx)
    return over, behind


def hair_side(style):
    """The head in profile, facing east: hair over its back half and brow."""
    s = STYLES[style]
    (ox, oy), (rx, ry) = HEAD["side"]
    cx, cy = HAIR_MARK[0] + ox, HAIR_MARK[1] + oy
    vol = s.get("volume", 1)
    orx, ory = rx + 1 + vol * 0.5, ry + 1 + vol * 0.5

    def cap(x, y):
        if not inside(x, y, cx, cy - 0.5, orx, ory):
            return False
        back = x < cx - 1 and y < cy + s["nape"]
        return back or y < cy + s["fringe"]

    over = blank()
    fill(over, cap)
    if s.get("spikes"):
        for x in range(cx - rx + 4, cx + rx - 4, 4):
            for dy in range(1, 3):
                over[cy - ry - 1 - dy][x] = HAIR
    behind = blank()
    fill(behind, lambda x, y: inside(x, y, cx, cy - 0.5, orx, ory) and y < cy and x < cx)
    if s.get("long"):
        fill(behind, lambda x, y: cx - rx - 1 <= x <= cx - 2 and cy <= y <= cy + ry + 5)
    if s.get("tail"):
        fill(behind, lambda x, y: cx - rx - 3 <= x <= cx - rx and cy - 3 <= y <= cy + 8)
    if s.get("bun"):
        bx, by = cx - rx + 2, cy - ry + 2
        fill(behind, lambda x, y: (x - bx) ** 2 + (y - by) ** 2 <= 17)
    shade(over, cx, cy, rx)
    shade(behind, cx, cy, rx)
    return over, behind


def hair_crown(style):
    """The top of a head bowed onto the desk."""
    s = STYLES[style]
    cx, cy, rx, ry = HAIR_MARK[0], HAIR_MARK[1] + 4, 12, 10
    over = blank()
    fill(over, lambda x, y: inside(x, y, cx, cy, rx, ry))
    if s.get("bun"):
        fill(over, lambda x, y: (x - cx) ** 2 + (y - cy) ** 2 <= 16)
    if s.get("tail") or s.get("long"):
        fill(over, lambda x, y: abs(x + 0.5 - cx) <= (rx if s.get("long") else 2) and cy <= y <= cy + ry + 4)
    shade(over, cx, cy, rx)
    # The whorl at the crown.
    over[cy - 2][cx] = HAIR_LIT
    over[cy - 2][cx + 1] = HAIR_LIT
    if s.get("bun"):
        for x in range(cx - 4, cx + 5):
            if over[cy - 4][x] != ".":
                over[cy - 4][x] = HAIR_SHADE
    return over


def write_hair(name, grid, view, comment):
    lines = [
        f"# {comment}",
        "# agent-office: drawn by tools/people.py; edit the script, not this file.",
        "@frame 0",
        f"@mark head.{view} {HAIR_MARK[0]} {HAIR_MARK[1]}",
    ]
    lines += [" ".join(row) for row in grid]
    with open(os.path.join(OUT, f"{name}@4x.sprite"), "w") as f:
        f.write("\n".join(lines) + "\n")


def hairs():
    for style in STYLES:
        front_over, front_behind = hair_front(style)
        back_over, back_behind = hair_back(style)
        side_over, side_behind = hair_side(style)
        layers = {
            ("front", "over"): front_over,
            ("front", "behind"): front_behind,
            ("back", "over"): back_over,
            ("back", "behind"): back_behind,
            ("side", "over"): side_over,
            ("side", "behind"): side_behind,
            ("crown", "over"): hair_crown(style),
        }
        for (view, layer), grid in layers.items():
            name = f"hair_{style}_{view}_{layer}"
            # Only the layers the pack's [hairstyles] names.
            if os.path.exists(os.path.join(OUT, f"{name}@4x.sprite")):
                write_hair(name, grid, view, f"The {style} hairstyle, {view} view: the layer {layer} the body.")


def main():
    if not UPSTREAM:
        sys.exit("pixtuoid-scene 0.21.0 is not in the cargo registry: run cargo fetch")
    people(UPSTREAM[0])
    hairs()
    print(f"drew {len(PEOPLE)} figures' files and {len(STYLES)} hairstyles")


if __name__ == "__main__":
    main()
