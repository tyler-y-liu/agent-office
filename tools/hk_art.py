"""agent-office's Hong Kong pieces: pixtuoid's furniture slots redrawn as a
cha chaan teng and the things a Hong Kong office keeps for luck.

Each piece is drawn at @4x with flat fills, outlined in the pack's `κ` round
its silhouette as the upstream art is, then shrunk to its 1x base. Sizes are
the slots' own (the layout and the hover boxes key off them), so only the
look changes. Writes into vendor/pixtuoid-scene/sprites/default; the palette
keys it uses beyond pixtuoid's are under "agent-office: Hong Kong" in
pack.toml.

Run from the repo root:  python3 tools/hk_art.py
"""

import os
from collections import Counter

OUT = "vendor/pixtuoid-scene/sprites/default"

T = "."  # transparent
OUTLINE = "κ"

# pixtuoid's keys this uses
WHITE, CHROME, STEEL, STEEL_DARK, INK = "w", "K", "ξ", "E", "q"
WOOD, WOOD_DARK, WOOD_LIT = "D", "d", "O"
LEAF, LEAF_DARK, LEAF_LIT, LEAF_RIB = "l", "L", "Φ", "Ψ"
# agent-office: Hong Kong
MINT, DINER_GREEN, TILE, GROUT = "ǆ", "Ǉ", "ǈ", "ǉ"
BUN, BUN_TOP, TART, MILK_TEA = "Ǌ", "ǋ", "ǌ", "Ǎ"
LUCKY_RED, GOLD, KUMQUAT, FORMICA = "ǎ", "Ǐ", "ǐ", "Ǒ"
STEEL_LIT, BAMBOO, PORCELAIN_BLUE, CAT_WHITE, BOARD = "ǒ", "Ǔ", "ǔ", "Ǖ", "ǖ"

PALETTE = {
    MINT: ("#8fd3b6", "cha chaan teng wall panel, mint"),
    DINER_GREEN: ("#2f7d5c", "cha chaan teng trim, deep green"),
    TILE: ("#f1ecdc", "mosaic tile, cream"),
    GROUT: ("#c8c0aa", "mosaic grout"),
    BUN: ("#e6a93e", "pineapple bun"),
    BUN_TOP: ("#b9772a", "pineapple bun's crackled top, a tart's crust"),
    TART: ("#f6cf47", "egg tart custard"),
    MILK_TEA: ("#b98454", "milk tea"),
    LUCKY_RED: ("#c8282a", "lucky red: lai see, couplets, lanterns"),
    GOLD: ("#f0c43c", "gold"),
    KUMQUAT: ("#f39a1e", "kumquat"),
    FORMICA: ("#e9e3cf", "formica table top"),
    STEEL_LIT: ("#c9ccd2", "stainless steel, lit"),
    BAMBOO: ("#7cb342", "lucky bamboo"),
    PORCELAIN_BLUE: ("#3b5ea8", "blue-and-white porcelain"),
    CAT_WHITE: ("#f6f2ea", "lucky cat"),
    BOARD: ("#1d3b2e", "menu board"),
    "Ǘ": ("#dcd9d2", "desk laminate"),
    "ǘ": ("#eceae4", "desk laminate, lit"),
    "Ǚ": ("#b4b0a6", "desk laminate, shaded"),
    "ǚ": ("#f7f5ef", "desk laminate's lit edge"),
    "Ǜ": ("#6f6b62", "desk laminate, darkest: legs and seams"),
    "ǜ": ("#f2dfb8", "the desk lamp's pool on laminate"),
}

# Desks: pixtuoid's wood as a Hong Kong office's pale laminate, the drawer
# pedestal in the cha chaan teng's greens. Only the desks' own sprites, so the
# shelves and tables keep their wood.
DESK_KEYS = {"D": "Ǘ", "O": "ǘ", "d": "Ǚ", "σ": "ǚ", "ψ": "Ǜ", "ω": "ǜ"}
PEDESTAL_KEYS = {"D": MINT, "O": MINT, "σ": MINT, "d": DINER_GREEN, "ψ": DINER_GREEN, "6": WHITE}
CABINET_KEYS = {"K": MINT, "ξ": DINER_GREEN, "&": TILE, "¤": WHITE}
DESKS = ["desk", "desk_north", "standing_desk"]
UPSTREAM = os.path.expanduser("~/.cargo/registry/src/*/pixtuoid-scene-0.21.0/sprites/default")


class Art:
    def __init__(self, w, h):
        self.w, self.h = w, h
        self.px = [[T] * w for _ in range(h)]

    def put(self, x, y, k):
        if 0 <= x < self.w and 0 <= y < self.h:
            self.px[y][x] = k

    def rect(self, x, y, w, h, k):
        for yy in range(y, y + h):
            for xx in range(x, x + w):
                self.put(xx, yy, k)

    def hline(self, x, y, w, k):
        self.rect(x, y, w, 1, k)

    def vline(self, x, y, h, k):
        self.rect(x, y, 1, h, k)

    def disc(self, cx, cy, r, k):
        for yy in range(cy - r, cy + r + 1):
            for xx in range(cx - r, cx + r + 1):
                if (xx - cx) ** 2 + (yy - cy) ** 2 <= r * r + r:
                    self.put(xx, yy, k)

    def at(self, x, y):
        return self.px[y][x] if 0 <= x < self.w and 0 <= y < self.h else T

    def outline(self):
        """`κ` on every empty pixel beside a drawn one, as pixtuoid's art."""
        edge = [
            (x, y)
            for y in range(self.h)
            for x in range(self.w)
            if self.at(x, y) == T
            and any(
                self.at(x + dx, y + dy) not in (T, OUTLINE)
                for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1))
            )
        ]
        for x, y in edge:
            self.px[y][x] = OUTLINE

    def base(self):
        """The 1x art: each 4x4 block's commonest colour, outlines in `n`."""
        w, h = self.w // 4, self.h // 4
        out = [[T] * w for _ in range(h)]
        for by in range(h):
            for bx in range(w):
                keys = [
                    self.px[by * 4 + dy][bx * 4 + dx] for dy in range(4) for dx in range(4)
                ]
                drawn = [k for k in keys if k != T]
                fill = [k for k in drawn if k != OUTLINE]
                last_row = by == h - 1
                if len(drawn) >= (1 if last_row else 6):
                    out[by][bx] = Counter(fill).most_common(1)[0][0] if fill else "n"
        return out


def write(name, art, comment):
    art.outline()
    for suffix, grid in (("@4x", art.px), ("", art.base())):
        lines = [
            f"# {comment}",
            "# agent-office: drawn by tools/hk_art.py; edit the script, not this file.",
            "@frame 0",
        ]
        lines += [" ".join(row) for row in grid]
        with open(os.path.join(OUT, f"{name}{suffix}.sprite"), "w") as f:
            f.write("\n".join(lines) + "\n")


# --- the cha chaan teng ---------------------------------------------------


def tile_front(a, x, y, w, h):
    """The counter's face: mint panel under a band of cream mosaic, a green
    rail between."""
    a.rect(x, y, w, h, MINT)
    band = max(3, h // 3)
    for yy in range(y, y + band):
        for xx in range(x, x + w):
            a.put(xx, yy, GROUT if (xx % 3 == 0 or (yy - y) % 3 == 0) else TILE)
    a.hline(x, y + band, w, DINER_GREEN)
    a.hline(x, y + h - 2, w, DINER_GREEN)


def urn(a, x, y, w, h):
    """The milk-tea urn: a steel drum, lit down its west side, with a tap."""
    a.rect(x, y + 2, w, h - 2, CHROME)
    a.vline(x + 1, y + 2, h - 2, STEEL_LIT)
    a.vline(x + w - 2, y + 2, h - 2, STEEL)
    a.rect(x + 1, y, w - 2, 2, STEEL)
    a.rect(x + w // 2 - 1, y - 1, 2, 1, INK)
    a.rect(x + w, y + h - 5, 2, 1, INK)
    a.put(x + w + 1, y + h - 4, INK)
    a.hline(x + 2, y + h // 2, w - 4, STEEL)


def cup(a, x, y):
    """A cup of milk tea on its saucer."""
    a.rect(x, y + 1, 4, 3, WHITE)
    a.hline(x + 1, y + 1, 2, MILK_TEA)
    a.put(x + 4, y + 2, WHITE)
    a.hline(x - 1, y + 4, 6, STEEL_LIT)


def bun(a, x, y):
    a.rect(x, y + 1, 5, 2, BUN)
    a.hline(x + 1, y, 3, BUN_TOP)
    a.put(x + 2, y + 1, BUN_TOP)


def tart(a, x, y):
    a.rect(x, y, 5, 3, BUN_TOP)
    a.rect(x + 1, y, 3, 2, TART)


def display_case(a, x, y, w, h):
    """A bakery case: steel frame, glass, shelves of buns and tarts."""
    a.rect(x, y, w, h, CHROME)
    a.rect(x + 1, y + 1, w - 2, h - 2, "¤")
    shelves = max(1, (h - 2) // 7)
    for i in range(shelves):
        sy = y + 2 + i * 7
        for j in range(x + 2, x + w - 6, 6):
            (bun if i % 2 == 0 else tart)(a, j, sy + 1)
        a.hline(x + 1, sy + 5, w - 2, STEEL)


def menu(a, x, y, w, h):
    """The day's menu: red header, white lines of dishes on a green board."""
    a.rect(x, y, w, h, BOARD)
    a.rect(x, y, w, 3, LUCKY_RED)
    a.hline(x + 1, y + 1, w - 2, GOLD)
    for ly in range(y + 5, y + h - 1, 3):
        a.hline(x + 2, ly, w - 6, WHITE)
        a.hline(x + w - 3, ly, 2, GOLD)


def pantry():
    a = Art(128, 40)
    display_case(a, 1, 8, 30, 31)
    a.rect(31, 18, 64, 3, STEEL_LIT)
    a.hline(31, 20, 64, STEEL)
    tile_front(a, 31, 21, 64, 18)
    urn(a, 36, 4, 14, 14)
    for i, cx in enumerate((58, 66, 74)):
        cup(a, cx, 13)
    menu(a, 82, 2, 12, 16)
    # A drinks fridge of lemon tea and soy milk.
    a.rect(96, 2, 31, 37, CHROME)
    a.rect(98, 4, 27, 30, "¤")
    for row, y in enumerate(range(6, 32, 7)):
        for i, bx in enumerate(range(100, 123, 4)):
            a.rect(bx, y, 2, 5, (LUCKY_RED, GOLD, WHITE, BUN)[(row + i) % 4])
        a.hline(98, y + 5, 27, STEEL)
    a.rect(96, 35, 31, 4, DINER_GREEN)
    write("pantry", a, "A cha chaan teng counter: bakery case, milk-tea urn, menu, drinks fridge.")


def pantry_small():
    a = Art(80, 32)
    display_case(a, 1, 6, 20, 25)
    a.rect(21, 14, 40, 3, STEEL_LIT)
    a.hline(21, 16, 40, STEEL)
    tile_front(a, 21, 17, 40, 14)
    urn(a, 25, 2, 11, 12)
    cup(a, 42, 9)
    cup(a, 50, 9)
    menu(a, 63, 1, 16, 30)
    write("pantry_small", a, "A small cha chaan teng counter: bakery case, urn, menu.")


def kitchen_island():
    a = Art(80, 28)
    # A formica table with its chrome rim, two stools each side.
    a.rect(14, 4, 52, 8, FORMICA)
    a.hline(14, 11, 52, GROUT)
    a.rect(14, 12, 52, 2, CHROME)
    a.rect(38, 14, 4, 8, CHROME)
    a.rect(32, 22, 16, 2, STEEL)
    cup(a, 22, 4)
    cup(a, 52, 4)
    # A plate of pineapple buns and the chopstick pot.
    a.rect(33, 6, 12, 3, WHITE)
    bun(a, 34, 4)
    bun(a, 39, 4)
    a.rect(60, 2, 3, 6, PORCELAIN_BLUE)
    a.vline(61, 0, 2, WOOD)
    for sx in (2, 69):
        for sy in (6, 16):
            a.rect(sx, sy, 9, 3, LUCKY_RED)
            a.hline(sx, sy + 3, 9, "N")
            a.rect(sx + 4, sy + 4, 1, 4 if sy == 16 else 3, CHROME)
    a.rect(2, 26, 76, 1, STEEL)
    write("kitchen_island", a, "A cha chaan teng table: formica top, milk tea and buns, red stools.")


def snack_shelf():
    a = Art(28, 40)
    display_case(a, 1, 1, 26, 38)
    a.rect(1, 1, 26, 3, LUCKY_RED)
    a.hline(3, 2, 22, GOLD)
    write("snack_shelf", a, "A bakery case of pineapple buns and egg tarts.")


# --- luck -----------------------------------------------------------------


def red_pot(a, x, y, w, h):
    a.rect(x, y, w, h, LUCKY_RED)
    a.hline(x, y, w, GOLD)
    a.hline(x + 1, y + h - 2, w - 2, "N")
    a.rect(x + w // 2 - 2, y + 2, 4, 3, GOLD)


def money_tree():
    """A money tree: a braided trunk under broad leaves, in a red pot."""
    a = Art(24, 28)
    for i in range(10):
        a.put(11 + (1 if (i // 2) % 2 else -1), 9 + i, WOOD)
        a.put(12 + (-1 if (i // 2) % 2 else 1), 9 + i, WOOD_DARK)
    for cx, cy, r in ((6, 6, 4), (12, 4, 4), (18, 6, 4), (9, 10, 3), (15, 10, 3)):
        a.disc(cx, cy, r, LEAF)
        a.put(cx, cy, LEAF_RIB)
        a.put(cx - 1, cy - 1, LEAF_LIT)
        a.put(cx + 1, cy + 2, LEAF_DARK)
    red_pot(a, 5, 19, 14, 8)
    write("plant", a, "A money tree in a red pot.")


def lucky_bamboo():
    """Lucky bamboo: stalks banded at the nodes, a red ribbon, a porcelain pot."""
    a = Art(24, 40)
    for i, (x, top) in enumerate(((8, 4), (11, 1), (14, 6), (17, 9), (5, 10))):
        a.rect(x, top, 2, 30 - top, BAMBOO)
        for ny in range(top + 5, 30, 6):
            a.hline(x, ny, 2, LEAF_DARK)
        a.rect(x - 2, top, 2, 1, LEAF)
        a.rect(x + 2, top + 2, 3, 1, LEAF_LIT)
    a.hline(5, 22, 14, LUCKY_RED)
    a.put(11, 23, GOLD)
    a.rect(4, 30, 16, 9, WHITE)
    a.hline(4, 30, 16, PORCELAIN_BLUE)
    for x in range(5, 19, 3):
        a.rect(x, 33, 2, 2, PORCELAIN_BLUE)
    a.hline(5, 37, 14, PORCELAIN_BLUE)
    write("plant_tall", a, "Lucky bamboo in a blue-and-white pot.")


def kumquat():
    """A New Year kumquat: fruit and lai see in the leaves, a red pot."""
    a = Art(24, 24)
    a.disc(12, 8, 7, LEAF)
    for x, y in ((8, 4), (15, 3), (6, 9), (17, 8), (11, 12), (14, 10), (9, 7)):
        a.rect(x, y, 2, 2, KUMQUAT)
    for x, y in ((12, 5), (5, 6)):
        a.rect(x, y, 2, 3, LUCKY_RED)
        a.put(x, y + 1, GOLD)
    a.put(13, 13, LEAF_DARK)
    a.rect(11, 15, 2, 2, WOOD)
    red_pot(a, 6, 17, 12, 6)
    write("plant_flower", a, "A New Year kumquat tree, lai see in its leaves.")


def lucky_cat():
    """A lucky cat waving its paw, a gold coin at its chest."""
    a = Art(20, 16)
    a.rect(5, 6, 10, 9, CAT_WHITE)
    a.rect(6, 1, 8, 6, CAT_WHITE)
    a.put(6, 0, CAT_WHITE)
    a.put(13, 0, CAT_WHITE)
    a.put(6, 1, LUCKY_RED)
    a.put(13, 1, LUCKY_RED)
    a.put(8, 3, INK)
    a.put(11, 3, INK)
    a.put(9, 5, LUCKY_RED)
    a.hline(6, 7, 8, LUCKY_RED)
    a.put(9, 8, GOLD)
    a.rect(8, 10, 4, 3, GOLD)
    a.rect(15, 2, 2, 6, CAT_WHITE)
    a.put(15, 2, LUCKY_RED)
    a.rect(3, 14, 14, 2, LUCKY_RED)
    write("plant_succulent", a, "A lucky cat waving its paw.")


def fu():
    """A 'fu' diamond: red paper on its point, a gold square at its heart."""
    a = Art(32, 20)
    cx, cy = 16, 9
    for y in range(20):
        half = 9 - abs(y - cy)
        if half >= 0:
            a.hline(cx - half, y, 2 * half + 1, LUCKY_RED)
    for y in range(20):
        half = 9 - abs(y - cy)
        if half >= 0:
            a.put(cx - half, y, GOLD)
            a.put(cx + half, y, GOLD)
    a.rect(cx - 3, cy - 3, 7, 7, GOLD)
    a.rect(cx - 1, cy - 1, 3, 3, LUCKY_RED)
    write("notice_board", a, "A 'fu' diamond for luck on the wall.")


def couplets():
    """Spring couplets: two red strips either side of the day's specials."""
    a = Art(40, 24)
    for x in (1, 33):
        a.rect(x, 1, 6, 22, LUCKY_RED)
        for y in range(3, 21, 4):
            a.rect(x + 2, y, 2, 2, GOLD)
    menu(a, 9, 2, 22, 20)
    write("bulletin_board", a, "The day's specials between two spring couplets.")


def remap(name, pick):
    """Rewrite `name`'s upstream art at both densities, each pixel's key
    through `pick(key, x, y, w, h, scale)`."""
    import glob

    src = glob.glob(UPSTREAM)[0]
    for suffix, scale in (("@4x", 4), ("", 1)):
        lines = open(os.path.join(src, f"{name}{suffix}.sprite")).read().splitlines()
        grid = [l.split(" ") for l in lines if l and not l.startswith(("#", "@"))]
        h, w = len(grid), len(grid[0])
        out, row = [], 0
        for l in lines:
            if "gen-art.py" in l:
                out.append("# agent-office: recoloured by tools/hk_art.py from pixtuoid's art.")
            elif not l or l.startswith(("#", "@")):
                out.append(l)
            else:
                keys = l.split(" ")
                out.append(" ".join(pick(k, x, row, w, h, scale) for x, k in enumerate(keys)))
                row += 1
        with open(os.path.join(OUT, f"{name}{suffix}.sprite"), "w") as f:
            f.write("\n".join(out) + "\n")


def desks():
    for name in DESKS:
        def pick(k, x, y, w, h, scale):
            # The pedestal: the desk's east drawers, under its top's front
            # edge, short of the outer columns, which the north-facing desk
            # mirrors. Only at @4x: the base keeps one laminate, its bottom row
            # the shade the cutaway derives a desk's face from.
            pedestal = scale == 4 and w - 4 * scale <= x < w - 2 and y >= h * 2 // 3
            if name != "standing_desk" and pedestal:
                return PEDESTAL_KEYS.get(k, k)
            return DESK_KEYS.get(k, k)

        remap(name, pick)
    remap("filing_cabinet", lambda k, *_: CABINET_KEYS.get(k, k))


def palette_block():
    lines = ["# --- agent-office: Hong Kong (tools/hk_art.py)"]
    lines += [f'"{k}" = "{v}"   # {c}' for k, (v, c) in PALETTE.items()]
    return "\n".join(lines) + "\n"


def ensure_palette():
    path = os.path.join(OUT, "pack.toml")
    s = open(path).read()
    head = "# --- agent-office: Hong Kong"
    if head in s:
        start = s.index(head)
        s = s[:start] + s[s.index("\n\n", start) + 2 :]
    anchor = s.index("[ramps]")
    s = s[:anchor] + palette_block() + "\n" + s[anchor:]
    open(path, "w").write(s)


def main():
    ensure_palette()
    for draw in (
        pantry,
        pantry_small,
        kitchen_island,
        snack_shelf,
        money_tree,
        lucky_bamboo,
        kumquat,
        lucky_cat,
        fu,
        couplets,
        desks,
    ):
        draw()
    print("drew the Hong Kong pieces")


if __name__ == "__main__":
    main()
