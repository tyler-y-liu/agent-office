"""agent-office's Hong Kong pieces: pixtuoid's furniture slots redrawn as a
cha chaan teng, a Central office and the things it keeps for luck, and the
rooftop's own pieces.

Each piece is drawn at @4x with flat fills, outlined in the pack's `κ` round
its silhouette as the upstream art is, then shrunk to its 1x base. A redrawn
slot keeps the slot's size (the layout and the hover boxes key off it), so
only the look changes. A new piece (rooftop_*, laundry_pole, water_tank,
office_shelf) also
needs its `Piece` in pixtuoid-core and its `[animations]` entries in pack.toml,
written by hand. Writes into vendor/pixtuoid-scene/sprites/default; the palette
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
# agent-office: the rooftop
STOOL, STOOL_DARK, STOOL_LIT, LEMON_TEA = "Ǟ", "ǟ", "Ǡ", "ǡ"
SHIRT, PANTS, TOWEL, BEER = "Ǣ", "ǣ", "Ǥ", "ǥ"
# agent-office: the boss's room and the lobby
LEATHER, LEATHER_LIT, LEATHER_DARK, LEATHER_SEAM = "Ǧ", "ǧ", "Ǫ", "ǫ"
ROSEWOOD, ROSEWOOD_LIT, ROSEWOOD_DARK, CLAY = "Ǭ", "ǭ", "Ǯ", "ǯ"
GOLD_LIT, GOLD_DARK, TANK_BACK = "Ǳ", "ǲ", "ǳ"
# agent-office: the trading floor and the shrine
UP_GREEN, DOWN_RED, SCREEN, SMOKE, GUAN_FACE = "Ǵ", "ǵ", "Ƕ", "Ƿ", "Ǹ"
LAMP = "9"  # the pack's bulb key: it glows after dark
DARK_RED = "N"

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
    STOOL: ("#d8382e", "plastic stool, red"),
    STOOL_DARK: ("#9a2219", "plastic stool, shaded"),
    STOOL_LIT: ("#f0675a", "plastic stool, lit rim"),
    LEMON_TEA: ("#d9902a", "iced lemon tea"),
    SHIRT: ("#8ec1ea", "a shirt on the line, sky blue"),
    PANTS: ("#2e4672", "trousers on the line, navy"),
    TOWEL: ("#f1a1b5", "a towel on the line, pink"),
    BEER: ("#2f8a3a", "a beer bottle, green"),
    LEATHER: ("#7a3a24", "the boss's leather, oxblood"),
    LEATHER_LIT: ("#a2573a", "leather, lit"),
    LEATHER_DARK: ("#552516", "leather, shaded"),
    LEATHER_SEAM: ("#36150c", "leather's seams and tufting"),
    ROSEWOOD: ("#5c2a1c", "rosewood"),
    ROSEWOOD_LIT: ("#83402a", "rosewood, lit edge"),
    ROSEWOOD_DARK: ("#3d1a11", "rosewood, shaded"),
    CLAY: ("#7a4a3e", "a Yixing clay teapot"),
    GOLD_LIT: ("#fde58a", "gold, catching the light"),
    GOLD_DARK: ("#b8862a", "gold, shaded"),
    TANK_BACK: ("#1c2d4a", "an arowana tank's dark backdrop"),
    UP_GREEN: ("#3fd46a", "a quote up: Hong Kong prints gains green"),
    DOWN_RED: ("#f0483c", "a quote down, in red"),
    SCREEN: ("#0c1626", "a quote screen's black"),
    SMOKE: ("#cfcac0", "incense smoke"),
    GUAN_FACE: ("#b8322a", "Guan Gong's red face"),
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
    write_frames(name, [art], comment)


def write_frames(name, arts, comment):
    """`arts` as `name`'s frames, each outlined and shrunk to its 1x base."""
    for art in arts:
        art.outline()
    for suffix, grids in (("@4x", [a.px for a in arts]), ("", [a.base() for a in arts])):
        lines = [
            f"# {comment}",
            "# agent-office: drawn by tools/hk_art.py; edit the script, not this file.",
        ]
        for i, grid in enumerate(grids):
            lines.append(f"@frame {i}")
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
    """A lucky cat waving its paw, a gold coin at its chest: up, then down."""
    frames = []
    for up in (True, False):
        frames.append(lucky_cat_frame(up))
    write_frames("plant_succulent", frames, "A lucky cat waving its paw.")


def lucky_cat_frame(up):
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
    if up:
        a.rect(15, 2, 2, 6, CAT_WHITE)
        a.put(15, 2, LUCKY_RED)
    else:
        a.rect(15, 5, 2, 3, CAT_WHITE)
        a.rect(16, 4, 2, 2, CAT_WHITE)
        a.put(17, 4, LUCKY_RED)
    a.rect(3, 14, 14, 2, LUCKY_RED)
    return a


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


# --- the rooftop -------------------------------------------------------------


def stool(a, x, top, w, base):
    """A red plastic stool seen from the side: a rim with its handle slot,
    splayed legs with the arch cut between them."""
    a.rect(x, top, w, 3, STOOL)
    a.hline(x, top, w, STOOL_LIT)
    a.rect(x + w // 2 - 2, top + 1, 4, 1, STOOL_DARK)
    a.hline(x + 1, top + 3, w - 2, STOOL_DARK)
    legs = base - top - 4
    for i in range(legs):
        y = top + 4 + i
        spread = i * 2 // legs
        a.rect(x + 1 - spread, y, 4, 1, STOOL)
        a.rect(x + w - 5 + spread, y, 4, 1, STOOL)
        a.put(x + w - 2 + spread, y, STOOL_DARK)
    # The stretcher half way down.
    a.hline(x + 4, top + 4 + legs // 2, w - 8, STOOL_DARK)


def rooftop_bench(name, comment):
    """A backless park bench in the meeting room's sofa slot: teak slats on
    two cast-iron legs, the same from either side, so it faces the sea."""
    a = Art(80, 28)
    # The seat: three slats with the gaps between, lit along their tops.
    for i, y in enumerate((10, 14, 18)):
        a.rect(2, y, 76, 3, WOOD)
        a.hline(2, y, 76, WOOD_LIT)
        a.hline(2, y + 2, 76, WOOD_DARK)
    # Cast-iron ends, scrolled at the foot.
    for x in (6, 68):
        a.rect(x, 9, 6, 13, STEEL_DARK)
        a.rect(x + 1, 22, 4, 4, STEEL_DARK)
        a.rect(x - 2, 25, 10, 2, STEEL_DARK)
        a.vline(x + 1, 10, 11, STEEL)
    write(name, a, comment)


def rooftop_table():
    """A round bistro table: a white marble top on an iron pedestal, iced
    lemon teas and a slice of egg tart."""
    a = Art(44, 20)
    # The top, an ellipse seen from above and in front.
    for y in range(0, 10):
        half = int(21 * (1 - ((y - 4.5) / 5) ** 2) ** 0.5)
        a.hline(22 - half, y, 2 * half, TILE)
    for y in (7, 8, 9):
        half = int(21 * (1 - ((y - 4.5) / 5) ** 2) ** 0.5)
        a.hline(22 - half, y, 2 * half, GROUT)
    a.put(12, 3, GROUT)
    a.put(13, 4, GROUT)
    a.put(29, 5, GROUT)
    # The pedestal and its foot.
    a.rect(20, 10, 4, 7, STEEL_DARK)
    a.rect(14, 17, 16, 2, STEEL_DARK)
    a.hline(15, 17, 14, STEEL)
    # Two iced lemon teas and a tart.
    for gx in (9, 30):
        a.rect(gx, 1, 4, 5, LEMON_TEA)
        a.hline(gx, 1, 4, WHITE)
        a.vline(gx + 3, 0, 2, WHITE)
    a.rect(19, 3, 5, 2, TART)
    a.hline(19, 5, 5, BUN_TOP)
    write("rooftop_table", a, "A bistro table on the rooftop: marble, lemon teas and a tart.")


def rooftop_stool():
    """A bistro chair at the table's end: a round seat on a wire frame, its
    hooped back on the far side."""
    a = Art(28, 28)
    # The hooped back.
    a.rect(4, 4, 2, 13, STEEL_DARK)
    a.rect(4, 4, 10, 2, STEEL_DARK)
    a.vline(9, 5, 11, STEEL)
    # The round seat, teak.
    a.rect(4, 15, 20, 4, WOOD)
    a.hline(4, 15, 20, WOOD_LIT)
    a.hline(4, 18, 20, WOOD_DARK)
    # Splayed wire legs.
    for i in range(8):
        y = 19 + i
        a.put(6 - i // 3, y, STEEL_DARK)
        a.put(21 + i // 3, y, STEEL_DARK)
        a.put(11 - i // 4, y, STEEL)
        a.put(16 + i // 4, y, STEEL)
    write("rooftop_stool", a, "A bistro chair at the end of the rooftop's table.")


def laundry_pole():
    """A drying rack: two posts under a bamboo pole, a shirt, trousers and a
    towel pegged on, swaying a little."""
    frames = []
    for sway in (0, 1):
        a = Art(20, 48)
        a.rect(1, 6, 2, 41, STEEL)
        a.rect(17, 6, 2, 41, STEEL)
        a.rect(0, 46, 6, 2, STEEL_DARK)
        a.rect(14, 46, 6, 2, STEEL_DARK)
        a.rect(0, 4, 20, 2, BAMBOO)
        # The shirt: shoulders on the pole, sleeves out, its hem swinging.
        a.rect(3, 6, 7, 3, SHIRT)
        a.rect(4, 9, 5, 9, SHIRT)
        a.rect(4 + sway, 18, 5, 2, SHIRT)
        a.vline(6, 7, 6, WHITE)
        # The trousers, two legs.
        a.rect(11, 6, 5, 4, PANTS)
        a.rect(11, 10, 2, 14, PANTS)
        a.rect(14, 10, 2, 14, PANTS)
        a.rect(11 + sway, 24, 2, 2, PANTS)
        a.rect(14 + sway, 24, 2, 2, PANTS)
        # A towel over the pole's middle.
        a.rect(8, 2, 4, 2, TOWEL)
        frames.append(a)
    write_frames("laundry_pole", frames, "Laundry drying on the rooftop, swaying.")


# --- the boss's room and the lobby ---------------------------------------------

SOFA_KEYS = {"C": LEATHER, "G": LEATHER_LIT, "Λ": LEATHER_DARK, "Ω": LEATHER_SEAM}
LAMP_KEYS = {"¤": LUCKY_RED, "!": DARK_RED, "Θ": GOLD, "7": GOLD_DARK, "6": GOLD_DARK}


def leather():
    """The sofas, the lobby's couch with them, and the chairs: oxblood
    leather, the sofas tufted."""
    remap("meeting_chair", lambda k, *_: SOFA_KEYS.get(k, k))
    for name in ("meeting_sofa", "meeting_sofa_north"):
        def pick(k, x, y, w, h, scale):
            k = SOFA_KEYS.get(k, k)
            # Buttons tufting the back, at @4x.
            if scale == 4 and k == LEATHER and y < h // 2 and x % 8 == 4 and y % 6 == 3:
                return LEATHER_SEAM
            return k
        remap(name, pick)


def tea_table():
    """The boss's rosewood tea table: a tray, a clay pot and four cups."""
    a = Art(44, 20)
    a.rect(1, 1, 42, 18, ROSEWOOD)
    a.hline(1, 1, 42, ROSEWOOD_LIT)
    a.hline(1, 15, 42, ROSEWOOD_LIT)
    a.rect(1, 16, 42, 3, ROSEWOOD_DARK)
    # Carved edge.
    for x in range(3, 41, 4):
        a.put(x, 2, ROSEWOOD_DARK)
    # The bamboo tray with its slats.
    a.rect(9, 4, 26, 10, BAMBOO)
    for x in range(10, 34, 3):
        a.vline(x, 5, 8, LEAF_DARK)
    # The pot, its lid and spout.
    a.rect(14, 6, 6, 5, CLAY)
    a.rect(16, 5, 2, 1, CLAY)
    a.put(20, 7, CLAY)
    a.put(21, 6, CLAY)
    a.rect(12, 7, 2, 2, CLAY)
    # Four cups and the fairness pitcher.
    for cx in (24, 28):
        for cy in (6, 10):
            a.rect(cx, cy, 2, 2, WHITE)
            a.put(cx, cy, MILK_TEA)
    a.rect(31, 7, 2, 3, PORCELAIN_BLUE)
    write("meeting_table", a, "The boss's rosewood tea table, a gongfu tea set on its tray.")


def gold_lamp():
    """The lobby's lamp: a red silk shade on a gold stand."""
    remap("floor_lamp", lambda k, *_: LAMP_KEYS.get(k, k))


def arowana(a, x, y, east):
    """A golden arowana 22 long, `x` its tail end's column, facing east or
    west: the fish a Hong Kong boss keeps for luck."""
    def put(dx, dy, k):
        a.put(x + dx if east else x + 21 - dx, y + dy, k)

    for dx in range(3, 20):
        for dy in (1, 2, 3):
            put(dx, dy, GOLD)
        put(dx, 1, GOLD_LIT if dx % 2 else GOLD)
        put(dx, 4 if 5 <= dx <= 17 else 3, GOLD_DARK if dx >= 5 else GOLD)
    for dx in range(4, 18, 3):
        put(dx, 2, GOLD_LIT)
    # Head, upturned mouth, barbels, eye.
    for dx, dy in ((20, 1), (20, 2), (21, 1), (20, 3), (22, 0)):
        put(dx, dy, GOLD)
    put(18, 2, INK)
    # Fins and tail, red.
    for dx, dy in ((14, 0), (15, 0), (16, 0), (13, 4), (14, 5), (15, 5)):
        put(dx, dy, LUCKY_RED)
    for dy in range(0, 5):
        put(0, dy, LUCKY_RED)
        put(1, dy, LUCKY_RED)
    put(2, 1, LUCKY_RED)
    put(2, 3, LUCKY_RED)


def arowana_tank():
    """The lobby's feng shui tank: a golden arowana patrolling a dark tank
    over gold pebbles, bubbles rising, on a rosewood cabinet."""
    frames = []
    n = 16
    for f in range(n):
        a = Art(56, 44)
        a.rect(1, 1, 54, 3, "ж")
        a.hline(1, 1, 54, "ν")
        a.rect(1, 4, 3, 28, "ж")
        a.rect(52, 4, 3, 28, "ж")
        a.rect(4, 4, 48, 24, TANK_BACK)
        a.hline(4, 4, 48, "З")
        a.rect(4, 5, 48, 10, "Д")
        a.rect(4, 15, 48, 14, "д")
        # A red glow along the lid, for luck.
        a.hline(4, 5, 48, LUCKY_RED)
        for x in range(4, 52):
            a.put(x, 28 + (x % 3 == 0), GOLD_DARK)
            for y in range(29, 32):
                a.put(x, y, (GOLD, GOLD_DARK, "z")[(x + y) % 3])
        # Bubbles from the stone at the east end, rising a row a frame.
        a.rect(45, 26, 4, 2, "z")
        for i in range(3):
            by = 26 - ((f * 2 + i * 7) % 21)
            a.put(46 + (i % 2), by, "З")
        # The fish: east across the tank, then back.
        half = n // 2
        step = f % half
        east = f < half
        x = 6 + step * 3 if east else 27 - step * 3
        y = 12 + (1 if f % 4 in (1, 2) else 0)
        arowana(a, x, y, east)
        # The cabinet, rosewood with gold pulls.
        a.rect(1, 32, 54, 1, ROSEWOOD_LIT)
        a.rect(1, 33, 54, 10, ROSEWOOD)
        a.rect(1, 40, 54, 3, ROSEWOOD_DARK)
        a.vline(27, 33, 9, ROSEWOOD_DARK)
        a.rect(23, 36, 2, 2, GOLD)
        a.rect(30, 36, 2, 2, GOLD)
        frames.append(a)
    write_frames("fish_tank", frames, "A feng shui tank: a golden arowana on patrol.")


# --- the trading floor and the shrine ------------------------------------------


def upstream(name):
    """`name`'s upstream @4x art, first frame, as an Art to draw over."""
    import glob

    src = glob.glob(UPSTREAM)[0]
    lines = open(os.path.join(src, f"{name}@4x.sprite")).read().splitlines()
    grid, on = [], False
    for l in lines:
        if l.startswith("@frame"):
            if on:
                break
            on = True
        elif on and l and not l.startswith(("#", "@")):
            grid.append(l.split(" "))
    a = Art(len(grid[0]), len(grid))
    a.px = grid
    return a


def quote_screen():
    """The aisle's TV cart as a quote screen: the Hang Seng's line running
    green up and red down, a ticker crawling under it."""
    # A walk of the index, long enough to scroll through: deterministic.
    walk, v = [], 10
    for i in range(64):
        v += (1, -1, 1, 0, -1, 1, -1, -1, 1, 1, 0, -1)[(i * 7) % 12]
        v = max(4, min(14, v))
        walk.append(v)
    frames = []
    for f in range(8):
        a = upstream("tv_stand")
        a.rect(3, 3, 34, 19, SCREEN)
        # Header: a red band, the index's gold tag.
        a.hline(3, 3, 34, LUCKY_RED)
        a.rect(4, 4, 6, 2, GOLD)
        a.rect(28, 4, 8, 2, UP_GREEN if (f // 2) % 2 == 0 else DOWN_RED)
        # The line, scrolling a column a frame.
        prev = None
        for x in range(4, 36):
            y = 7 + (18 - walk[(x - 4 + f * 2) % len(walk)]) * 10 // 18
            col = UP_GREEN if prev is None or y <= prev else DOWN_RED
            lo, hi = (y, y) if prev is None else (min(y, prev), max(y, prev))
            for yy in range(lo, hi + 1):
                a.put(x, yy, col)
            prev = y
        # The ticker: blocks of quotes crawling west.
        for x in range(3, 37):
            k = (x + f * 3) % 9
            if k < 6:
                a.put(x, 20, UP_GREEN if (x + f * 3) // 9 % 3 else DOWN_RED)
        frames.append(a)
    write_frames("tv_stand", frames, "A Hang Seng quote screen on a cart, quotes crawling.")


def shrine():
    """Guan Gong's shrine where the bookshelf stood: red lacquer and gold,
    the general red-faced in green, his lamps lit, incense smoking."""
    frames = []
    for f in range(4):
        a = Art(32, 48)
        # The roof, its ends turned up.
        a.rect(2, 3, 28, 3, LUCKY_RED)
        a.hline(1, 2, 30, GOLD)
        a.put(0, 1, GOLD)
        a.put(31, 1, GOLD)
        # The cabinet and its dark niche.
        a.rect(3, 6, 26, 41, LUCKY_RED)
        a.rect(6, 8, 20, 21, DARK_RED)
        a.vline(3, 6, 41, GOLD)
        a.vline(28, 6, 41, GOLD)
        # Guan Gong: red face, black beard, green robe, gold belt.
        a.rect(14, 11, 4, 4, GUAN_FACE)
        a.rect(13, 10, 6, 1, INK)
        a.rect(15, 15, 2, 4, INK)
        a.rect(11, 15, 10, 13, LEAF)
        a.rect(11, 15, 2, 13, LEAF_DARK)
        a.hline(11, 21, 10, GOLD)
        # His guandao, gold.
        a.vline(23, 9, 19, GOLD)
        a.rect(22, 9, 3, 3, GOLD_LIT)
        # The two red lamps, lit.
        for lx in (7, 23):
            a.rect(lx, 23, 3, 4, LUCKY_RED)
            a.put(lx + 1, 24, LAMP)
            a.put(lx + 1, 25, LAMP)
        # The altar shelf: the burner, three sticks, their embers.
        a.rect(4, 30, 24, 2, GOLD)
        a.rect(12, 32, 8, 3, GOLD_DARK)
        a.hline(11, 32, 10, GOLD)
        for sx in (14, 16, 18):
            a.vline(sx, 26, 6, WOOD)
            a.put(sx, 26, LAMP)
        # Smoke, curling up a row each frame.
        for i, sx in enumerate((14, 16, 18)):
            for k in range(3):
                y = 25 - ((f + i + k * 3) % 9)
                x = sx + (1 if (y + i) % 4 < 2 else 0)
                if y > 8:
                    a.put(x, y, SMOKE)
        # The doors below, gold pulls and a gold 'fu' square.
        a.vline(16, 36, 10, DARK_RED)
        a.rect(13, 40, 2, 2, GOLD)
        a.rect(18, 40, 2, 2, GOLD)
        a.rect(14, 37, 4, 2, GOLD_LIT)
        frames.append(a)
    write_frames("bookshelf", frames, "Guan Gong's shrine: lamps lit, incense smoking.")


def water_tank():
    """A Hong Kong rooftop's water tank: a pale fibreglass box banded in
    blue, a ladder up its side, on a steel stand, its pipe down to the roof."""
    a = Art(48, 64)
    # The stand: four legs and a cross brace.
    for lx in (4, 14, 32, 42):
        a.rect(lx, 40, 3, 23, STEEL)
        a.vline(lx, 40, 23, STEEL_LIT)
    for i in range(18):
        a.put(7 + i * 25 // 18, 44 + i, STEEL_DARK)
        a.put(41 - i * 25 // 18, 44 + i, STEEL_DARK)
    a.rect(2, 38, 44, 3, STEEL_DARK)
    # The tank.
    a.rect(3, 6, 42, 32, WHITE)
    a.rect(3, 6, 42, 3, "¤")
    a.vline(3, 6, 32, "!")
    a.rect(35, 9, 10, 29, "!")
    for by in (14, 26):
        a.rect(3, by, 42, 2, PORCELAIN_BLUE)
    # The manhole on top, a vent pipe.
    a.rect(14, 3, 12, 3, STEEL)
    a.rect(30, 0, 3, 6, STEEL_DARK)
    # The ladder up its east side.
    a.vline(45, 4, 58, STEEL_LIT)
    a.vline(47, 4, 58, STEEL_LIT)
    for ry in range(6, 62, 5):
        a.hline(45, ry, 3, STEEL_LIT)
    # The outlet pipe down to the roof.
    a.rect(22, 38, 3, 25, PORCELAIN_BLUE)
    write("water_tank", a, "A rooftop water tank on its steel stand.")


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


def office_shelf():
    """Your office's bookshelf: rosewood, four shelves of books in their
    spines' colours, a gold lucky cat and a porcelain vase among them."""
    a = Art(32, 48)
    a.rect(0, 0, 32, 48, ROSEWOOD)
    a.hline(0, 0, 32, ROSEWOOD_LIT)
    a.rect(0, 45, 32, 3, ROSEWOOD_DARK)
    spines = [LUCKY_RED, PORCELAIN_BLUE, DINER_GREEN, GOLD_DARK, BOARD, CLAY, LEATHER]
    for shelf, top in enumerate((3, 14, 25, 36)):
        a.rect(2, top, 28, 9, ROSEWOOD_DARK)
        x, i = 3, shelf * 3
        while x < 29:
            w = 2 + (i * 5 + shelf) % 2
            h = 6 + (i * 7 + shelf) % 3
            if (i + shelf) % 6 == 5:
                # A gap with an ornament in it.
                if shelf == 1:
                    a.rect(x, top + 4, 4, 5, GOLD)
                    a.rect(x + 1, top + 3, 2, 1, GOLD_LIT)
                else:
                    a.rect(x, top + 3, 3, 6, PORCELAIN_BLUE)
                    a.vline(x + 1, top + 4, 4, WHITE)
                x += 5
            else:
                a.rect(x, top + 9 - h, w, h, spines[i % len(spines)])
                a.put(x, top + 9 - h + 1, GOLD_LIT)
                x += w
            i += 1
        a.hline(1, top + 9, 30, ROSEWOOD_LIT)
    write("office_shelf", a, "Your office's rosewood bookshelf.")


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
        lambda: rooftop_bench("rooftop_bench", "A backless teak bench on the rooftop, facing the sea."),
        lambda: rooftop_bench("rooftop_bench_north", "A backless teak bench on the rooftop, from behind."),
        rooftop_table,
        rooftop_stool,
        laundry_pole,
        leather,
        tea_table,
        gold_lamp,
        arowana_tank,
        quote_screen,
        shrine,
        water_tank,
        office_shelf,
    ):
        draw()
    print("drew the Hong Kong pieces")


if __name__ == "__main__":
    main()
