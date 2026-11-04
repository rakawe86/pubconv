"""Hard test corpus: complex, realistic Publisher documents made with Microsoft
Publisher itself (pywin32), plus Publisher's own PDF of each as the answer key.

  python tools/make_hard_corpus.py            # build everything, then scrub
  python tools/make_hard_corpus.py nl_parish_8p wa_unicode   # only these
  python tools/make_hard_corpus.py --scrub    # only scrub + verify

Writes testdata/hard/<name>.pub, testdata/reference/hard/<name>.ref.pdf and
testdata/reference/hard/hard_meta.json (WordArt texts and boxes, linked chains
with Publisher's own record of which characters land in each box).

Documents are built in small batches, each in its own worker process with a
time limit; MSPUB.EXE is killed between batches so a stuck dialog can't stall
the run. Afterwards every new .pub/.ref.pdf is scrubbed of the Windows user
name (same-length byte replacement, ASCII and UTF-16LE; PDF author cleared).
"""
import json
import os
import random
import re
import subprocess
import sys
import time
import traceback
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "testdata" / "hard"
REF = ROOT / "testdata" / "reference" / "hard"
WORK = ROOT / ".build" / "hard"
IMG = WORK / "images"
METAD = WORK / "meta"
PDF = 2
H = 1
RECT, ROUNDED, OVAL, STAR = 1, 5, 9, 92
BATCH = 3
TIMEOUT = 900


def rgb(r, g, b):
    return r + (g << 8) + (b << 16)


# ------------------------------------------------------------------ prose
FIRST = ["Margaret", "Tomás", "Priya", "Desmond", "Aoife", "Kwame", "Helena", "Joaquín", "Bridget", "Oluwaseun",
         "Ingrid", "Rafael", "Siobhan", "Mateusz", "Yasmin", "Callum", "Leonie", "Dmitri", "Rosalind", "Hamish",
         "Fatima", "Benedict", "Imogen", "Thaddeus", "Wilhelmina", "Gareth", "Noor", "Cormac", "Esperanza", "Linnea"]
LAST = ["O'Sullivan", "Okafor", "Brennan", "Kowalski", "Nakamura", "Fitzgerald", "Haddad", "Lindqvist", "Moreau",
        "Achterberg", "Delacroix", "Mbeki", "Quinlan", "Ferreira", "Whitcombe", "Abernathy", "Castellanos", "Ravensworth"]
THEMES = {
    "parish": dict(
        event=["harvest supper", "Advent vigil", "parish picnic", "choir festival", "blessing of the animals", "jumble sale",
               "confirmation retreat", "carol service", "pancake breakfast", "memorial concert"],
        place=["the parish hall", "the old sacristy", "the churchyard garden", "St. Columba's chapel", "the bell tower",
               "the community kitchen", "the north transept", "the presbytery lawn"],
        thing=["stained-glass windows", "organ pipes", "hymn books", "altar linens", "roof slates", "pew cushions",
               "candle stands", "donation boxes", "flower rotas"],
        people=["parishioners", "families", "volunteers", "choristers", "pilgrims", "neighbours", "altar servers"],
        verb=["restoring", "polishing", "repainting", "cataloguing", "repairing", "sorting", "decorating"]),
    "school": dict(
        event=["science fair", "sports day", "spring musical", "robotics tournament", "spelling bee", "art exhibition",
               "debate championship", "charity fun-run", "field trip", "graduation assembly"],
        place=["the gymnasium", "the library annex", "the chemistry laboratory", "the playing fields", "the auditorium",
               "the music wing", "the portable classrooms", "the cafeteria"],
        thing=["microscopes", "costumes", "trophies", "laptops", "lockers", "scoreboards", "easels", "lab coats"],
        people=["pupils", "teachers", "parents", "governors", "sixth-formers", "coaches", "alumni"],
        verb=["assembling", "rehearsing", "painting", "labelling", "programming", "measuring", "tidying"]),
    "garden": dict(
        event=["seed swap", "allotment open day", "pumpkin weigh-in", "rose show", "pruning workshop", "compost clinic",
               "hedge-laying course", "bulb planting morning"],
        place=["the walled garden", "the greenhouse", "plot fourteen", "the orchard", "the wildflower meadow",
               "the potting shed", "the riverside beds"],
        thing=["seedlings", "trellises", "rain barrels", "beehives", "raised beds", "wheelbarrows", "dahlias", "espaliers"],
        people=["gardeners", "beekeepers", "members", "newcomers", "plot holders", "visitors"],
        verb=["mulching", "transplanting", "harvesting", "weeding", "grafting", "watering", "staking"]),
    "town": dict(
        event=["farmers' market", "council meeting", "heritage walk", "street festival", "library relaunch",
               "bridge reopening", "marathon", "film night", "craft fair"],
        place=["the market square", "the town hall", "the riverside path", "the old mill", "the bandstand",
               "the railway museum", "Victoria Park", "the high street"],
        thing=["benches", "lampposts", "bicycle racks", "planters", "murals", "bus shelters", "notice boards"],
        people=["residents", "shopkeepers", "councillors", "visitors", "commuters", "stallholders", "historians"],
        verb=["refurbishing", "installing", "surveying", "repainting", "inspecting", "cleaning", "mapping"]),
}
TEMPLATES = [
    "{N} told us that the {e} at {p} drew more than {n} {pp} this {s}.",
    "Volunteers spent {n} hours {v} the {t} before the {e} began.",
    "Thanks to a generous gift from {N}, the {t} at {p} will finally be replaced.",
    "If you would like to help with the {e}, please speak to {N} after Sunday's gathering.",
    "The committee agreed unanimously to move the {e} to {p}, which offers far more room.",
    "Many {pp} remember the {a} {e} of years gone by, when {N} first organised it.",
    "Our {pp} have been busy {v} the {t}, and the results are {a}.",
    "A small team led by {N} and {M} visited {p} on {d} to plan the next steps.",
    "Tickets for the {e} cost {n} pounds, and every penny goes towards the {t}.",
    "We were delighted to welcome {n} new {pp} during the {s}, many of them young families.",
    "Weather permitting, the {e} will take place at {p} on {d} afternoon.",
    "{N} reminds everyone that the {t} must be returned to {p} by {d}.",
    "It was a {a} sight: {n} {pp} {v} the {t} side by side.",
    "Next {s}, we hope to repeat the {e} with even more {pp} taking part.",
    "Special thanks go to {N}, whose patience with the {t} has been nothing short of {a}.",
    "Anyone with photographs of earlier {e}s at {p} is asked to contact {M}.",
    "The budget for {v} the {t} came in {n} pounds under the original estimate.",
    "Refreshments will be served at {p} once the {e} has finished.",
    "{N} and {M} have kindly offered to coordinate transport for older {pp}.",
    "Records kept at {p} show the {t} were last renewed almost {n} years ago.",
    "Questions about the {e} can be left in the box beside {p}.",
    "Please note the new start time for the {e}: doors now open at {h} o'clock.",
]
ADJ = ["remarkable", "heart-warming", "splendid", "memorable", "unexpected", "extraordinary", "cheerful", "humbling",
       "magnificent", "spirited"]
DAYS = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"]
SEASONS = ["spring", "summer", "autumn", "winter", "term", "year"]


def prose(seed, nwords, theme="parish", heads=None):
    """Plain, varied newsletter prose of about `nwords` words. `heads` (list of
    short titles) are inserted as their own paragraphs every few paragraphs."""
    rng = random.Random(seed)
    th = THEMES[theme]
    paras, count, hi = [], 0, 0
    while count < nwords:
        if heads and hi < len(heads) and paras and len(paras) % 3 == 0:
            paras.append(heads[hi])
            hi += 1
        sents = []
        for _ in range(rng.randint(3, 6)):
            s = rng.choice(TEMPLATES).format(
                N=f"{rng.choice(FIRST)} {rng.choice(LAST)}", M=f"{rng.choice(FIRST)} {rng.choice(LAST)}",
                e=rng.choice(th["event"]), p=rng.choice(th["place"]), t=rng.choice(th["thing"]),
                pp=rng.choice(th["people"]), v=rng.choice(th["verb"]), a=rng.choice(ADJ),
                d=rng.choice(DAYS), s=rng.choice(SEASONS), n=rng.randint(3, 480), h=rng.randint(2, 11))
            sents.append(s[0].upper() + s[1:])
        para = " ".join(sents)
        paras.append(para)
        count += len(para.split())
    return "\r".join(paras)


# ------------------------------------------------------------------ images
def make_images():
    from PIL import Image, ImageDraw
    IMG.mkdir(parents=True, exist_ok=True)
    rng = random.Random(7)
    for k, (c1, c2) in enumerate([((40, 90, 160), (230, 180, 60)), ((30, 120, 70), (240, 240, 200)),
                                  ((150, 40, 60), (250, 210, 170)), ((60, 60, 70), (200, 220, 240))]):
        im = Image.new("RGB", (640, 480))
        px = im.load()
        for y in range(480):
            for x in range(640):
                f = (x + y) / 1120
                n = rng.randint(-18, 18)
                px[x, y] = tuple(max(0, min(255, int(a + (b - a) * f) + n)) for a, b in zip(c1, c2))
        d = ImageDraw.Draw(im)
        d.ellipse([200 + k * 30, 120, 420 + k * 30, 340], fill=c2)
        d.rectangle([60, 330, 300, 460], fill=c1)
        im.save(IMG / f"photo{k}.jpg", quality=85)
    face = Image.new("RGB", (240, 300), (210, 215, 225))
    d = ImageDraw.Draw(face)
    d.ellipse([70, 50, 170, 160], fill=(180, 140, 110))
    d.rectangle([40, 180, 200, 300], fill=(60, 80, 140))
    face.save(IMG / "portrait.jpg", quality=85)
    logo = Image.new("RGBA", (300, 300), (0, 0, 0, 0))
    ImageDraw.Draw(logo).polygon([(150, 10), (290, 290), (10, 290)], fill=(200, 30, 60, 255))
    logo.save(IMG / "logo.png")


# ------------------------------------------------------------------ builder context
class Ctx:
    def __init__(self, doc, name):
        self.doc, self.name = doc, name
        self.wordart, self.stories = [], []

    # pages -----------------------------------------------------------
    def size(self, w, h):
        self.doc.PageSetup.PageWidth = w
        self.doc.PageSetup.PageHeight = h

    def pages(self, n):
        c = self.doc.Pages.Count
        if n > c:
            self.doc.Pages.Add(n - c, c)

    def _shapes(self, pg):
        return self.doc.MasterPages(1).Shapes if pg == "M" else self.doc.Pages(pg).Shapes

    # simple objects ---------------------------------------------------
    def tb(self, pg, text, l, t, w, h, size=11, font="Arial", bold=False, italic=False, color=None, align=None):
        s = self._shapes(pg).AddTextbox(H, l, t, w, h)
        r = s.TextFrame.TextRange
        r.Text = text
        r.Font.Name = font
        r.Font.Size = size
        r.Font.Bold = bold
        r.Font.Italic = italic
        if color is not None:
            r.Font.Color.RGB = color
        if align:
            r.ParagraphFormat.Alignment = align
        return s

    def rect(self, pg, l, t, w, h, fill, kind=RECT, line=None, transp=None):
        s = self._shapes(pg).AddShape(kind, l, t, w, h)
        if fill is None:
            s.Fill.Visible = False
        else:
            s.Fill.ForeColor.RGB = fill
        if line is None:
            s.Line.Visible = False
        else:
            s.Line.ForeColor.RGB = line[0]
            s.Line.Weight = line[1]
        if transp is not None:
            s.Fill.Transparency = transp
        return s

    def pic(self, pg, name, l, t, w, h):
        return self._shapes(pg).AddPicture(str(IMG / name), False, True, l, t, w, h)

    def hline(self, pg, x0, y, x1, weight=1, color=0):
        s = self._shapes(pg).AddLine(x0, y, x1, y)
        s.Line.Weight = weight
        s.Line.ForeColor.RGB = color
        return s

    def table(self, pg, data, l, t, w, h, size=9, font="Arial"):
        rows, cols = len(data), len(data[0])
        s = self._shapes(pg).AddTable(rows, cols, l, t, w, h)
        for r in range(rows):
            for c in range(cols):
                if data[r][c]:
                    tr = s.Table.Rows(r + 1).Cells(c + 1).TextRange
                    tr.Text = data[r][c]
                    tr.Font.Size = size
                    tr.Font.Name = font
                    if r == 0:
                        tr.Font.Bold = True
        return s

    def footer(self, label, w=612, h=792, font="Georgia"):
        """Master-page footer with automatic page number and a rule."""
        self.hline("M", 36, h - 44, w - 36, 0.75, rgb(120, 120, 120))
        self.tb("M", label, 36, h - 40, w - 160, 18, 8, font, italic=True, color=rgb(90, 90, 90))
        s = self.tb("M", "", w - 120, h - 40, 84, 18, 8, font, italic=True, color=rgb(90, 90, 90), align=3)
        s.TextFrame.TextRange.InsertPageNumber()
        return s

    # WordArt ------------------------------------------------------------
    def wa(self, pg, text, l, t, preset=1, font="Arial Black", size=36, bold=True, italic=False, shape=None,
           rotation=0, flip=None, color=None):
        preset = (preset - 1) % 29 + 1  # Publisher accepts presets 1-29 through COM
        try:
            s = self._shapes(pg).AddTextEffect(preset, text, font, size, bold, italic, l, t)
            if shape:
                s.TextEffect.PresetShape = shape
        except Exception as e:
            raise RuntimeError(f"WordArt {text!r} preset={preset} shape={shape} font={font}: {e}") from None
        if color is not None:
            try:
                s.Fill.ForeColor.RGB = color
            except Exception:
                pass
        if flip is not None:
            s.Flip(flip)
        if rotation:
            s.Rotation = rotation
        self.wordart.append(dict(page=pg, text=text, font=font, size=size, bold=bool(bold), italic=bool(italic),
                                 preset=preset, _s=s))
        return s

    # linked stories -------------------------------------------------------
    def story(self, name, boxes, text, font="Arial", size=10, align=None, cont=True, columns=None,
              space_after=None, heads=None, first_indent=None, color=None, fit=True):
        """boxes: list of (page, left, top, width, height). Text goes into box 1, then the
        boxes are linked in order. `heads` are paragraphs to make bold (mixed style).
        fit=True trims whole paragraphs so the story ends in its last box (as a real
        user would); fit=False keeps the overflow (Publisher hides the excess)."""
        shp = [self._shapes(pg).AddTextbox(H, l, t, w, h) for pg, l, t, w, h in boxes]

        def put(rng, text):
            rng.Text = text
            rng.Font.Name = font
            rng.Font.Size = size
            rng.Font.Bold = False
            if color is not None:
                rng.Font.Color.RGB = color
            if align:
                rng.ParagraphFormat.Alignment = align
            if space_after is not None:
                rng.ParagraphFormat.SpaceAfter = space_after
            if first_indent is not None:
                rng.ParagraphFormat.FirstLineIndent = first_indent
            if heads:
                pos = 0
                for para in text.split("\r"):
                    if para in heads:
                        c = rng.Characters(pos + 1, len(para))
                        c.Font.Bold = True
                        c.Font.Size = size + 2
                    pos += len(para) + 1

        put(shp[0].TextFrame.TextRange, text)
        if columns:
            for s in shp:
                s.TextFrame.Columns = columns
        for a, b in zip(shp, shp[1:]):
            a.TextFrame.NextLinkedTextFrame = b.TextFrame
        if cont:
            for (pa, *_), (pb, *_), a, b in zip(boxes, boxes[1:], shp, shp[1:]):
                if pa != pb:
                    a.TextFrame.IncludeContinuedOnPage = True
                    b.TextFrame.IncludeContinuedFromPage = True
        for _ in range(4):
            last = shp[-1].TextFrame
            if not (fit and last.Overflowing):
                break
            placed = last.TextRange.Start + last.TextRange.Length
            paras, keep, n = text.split("\r"), [], 0
            for para in paras:
                if n + len(para) + 1 > 0.93 * placed and keep:
                    break
                keep.append(para)
                n += len(para) + 1
            if heads and keep and keep[-1] in heads:
                keep.pop()
            text = "\r".join(keep)
            put(shp[0].TextFrame.Story.TextRange, text)
        self.stories.append(dict(name=name, font=font, size=size, text_len=len(text), mixed=bool(heads),
                                 columns=columns or 1, boxes=[dict(page=b[0], left=b[1], top=b[2], width=b[3],
                                                                   height=b[4]) for b in boxes], _s=shp))
        return shp

    # collect ------------------------------------------------------------------
    def collect(self):
        was = []
        for w in self.wordart:
            s = w.pop("_s")
            w.update(left=s.Left, top=s.Top, width=s.Width, height=s.Height, rotation=s.Rotation,
                     shape=s.TextEffect.PresetShape)
            was.append(w)
        sts = []
        for st in self.stories:
            shp = st.pop("_s")
            for b, s in zip(st["boxes"], shp):
                tr = s.TextFrame.TextRange
                t = tr.Text
                b.update(start=tr.Start, length=tr.Length, first=t[:30], last=t[-30:],
                         overflow=bool(s.TextFrame.Overflowing))
            sts.append(st)
        return dict(pages=self.doc.Pages.Count, page_w=self.doc.PageSetup.PageWidth,
                    page_h=self.doc.PageSetup.PageHeight, wordart=was, stories=sts)


def cols(n, x0=36, x1=576, gutter=14):
    w = (x1 - x0 - gutter * (n - 1)) / n
    return [(x0 + i * (w + gutter), w) for i in range(n)]


# ================================================================== documents
def nl_parish_8p(c):
    """8-page parish newsletter, 4 stories at once, jumps forward with continued notices."""
    c.pages(8)
    c.footer("St. Brendan's Messenger · Michaelmas 2026")
    c.wa(1, "St. Brendan's Messenger", 60, 24, preset=3, font="Old English Text MT", size=40, bold=False)
    c.tb(1, "Parish newsletter · Volume 31 · Issue 9 · Michaelmas 2026", 36, 110, 540, 18, 10, "Georgia", italic=True,
         align=2)
    c.hline(1, 36, 132, 576, 2, rgb(120, 20, 40))
    (x1, w1), (x2, w2), (x3, w3) = cols(3)
    for i, (title, x, w) in enumerate([("A Letter from Father Declan", x1, w1 * 2 + 14), ("Food Bank Record", x3, w3)]):
        c.tb(1, title, x, 140, w, 26, 15, "Georgia", bold=True)
    c.story("pastor", [(1, x1, 170, w1, 400), (1, x2, 170, w2, 400), (4, 36, 90, 255, 640)],
            prose(11, 1300, "parish"), "Georgia", 10, align=4, space_after=4)
    c.story("foodbank", [(1, x3, 170, w3, 230), (5, 321, 90, 255, 300), (7, 36, 420, 540, 120)],
            prose(12, 900, "parish"), "Georgia", 10, space_after=4)
    c.pic(1, "photo0.jpg", x3, 410, w3, 160)
    c.tb(1, "Inside: Youth pilgrimage p.2 · Restoration appeal p.3 · Diary p.6", 36, 590, 540, 20, 10, "Georgia",
         italic=True, align=2)
    c.story("brief", [(1, 36, 620, 540, 100)], prose(13, 160, "parish"), "Georgia", 9)
    c.tb(2, "Youth Pilgrimage to Lourdes", 36, 40, 540, 30, 20, "Georgia", bold=True)
    c.pic(2, "photo1.jpg", 36, 80, 540, 200)
    c.story("youth", [(2, 36, 300, 260, 420), (2, 316, 300, 260, 420), (6, 36, 380, 540, 330)],
            prose(14, 1400, "parish"), "Georgia", 10, align=4)
    c.tb(3, "Restoration Appeal", 36, 40, 540, 30, 20, "Georgia", bold=True)
    c.story("restore", [(3, 36, 80, 170, 600), (3, 221, 80, 170, 600), (8, 36, 60, 255, 640), (8, 321, 60, 255, 300)],
            prose(15, 1900, "parish"), "Georgia", 9.5, align=4)
    c.rect(3, 406, 80, 170, 300, rgb(245, 235, 210))
    c.tb(3, "Appeal target\r£48,500\rRaised so far\r£31,270", 416, 100, 150, 200, 16, "Georgia", bold=True, align=2)
    c.table(6, [["Date", "Event", "Where"]] + [[f"{d} Oct", e, p] for d, e, p in
            zip(range(4, 29, 3), THEMES["parish"]["event"], THEMES["parish"]["place"])], 36, 40, 540, 300, 9, "Georgia")
    c.pic(4, "photo2.jpg", 321, 90, 255, 220)
    c.pic(5, "portrait.jpg", 36, 90, 255, 300)
    c.tb(7, "Notices", 36, 40, 540, 30, 20, "Georgia", bold=True)
    c.tb(7, prose(16, 300, "parish"), 36, 80, 540, 320, 10, "Georgia")


def nl_school_12p(c):
    """12-page school newsletter on a 3-column grid; every column is its own box.
    Chains of 6-10 boxes jumping pages, bold subheads inside a story, WordArt section heads."""
    c.pages(12)
    c.footer("The Hillcrest Herald · Autumn Term", font="Verdana")
    C = cols(3)
    c.wa(1, "The Hillcrest Herald", 50, 20, preset=9, font="Impact", size=44, bold=False, color=rgb(20, 60, 140))
    c.tb(1, "News from Hillcrest Academy · Autumn Term 2026", 36, 100, 540, 18, 10, "Verdana", italic=True, align=2)
    c.tb(1, "Robotics Team Heads to Nationals", 36, 124, 540, 26, 16, "Verdana", bold=True)
    c.story("robotics", [(1, x, 156, w, 560) for x, w in C] + [(2, C[0][0], 40, C[0][1], 680),
            (9, C[1][0], 300, C[1][1], 420), (9, C[2][0], 300, C[2][1], 420)],
            prose(21, 3200, "school"), "Verdana", 9)
    c.pic(2, "photo3.jpg", C[1][0], 40, C[1][1] * 2 + 14, 300)
    c.tb(2, "Year 9 robotics captains with the regional trophy.", C[1][0], 345, C[1][1] * 2 + 14, 20, 8, "Verdana",
         italic=True)
    c.tb(2, prose(22, 180, "school"), C[1][0], 380, C[1][1] * 2 + 14, 330, 9, "Verdana")
    c.wa(3, "Arts & Music", 150, 24, preset=14, font="Brush Script MT", size=44, bold=False, shape=9)
    heads = ["Spring Musical Casting", "Art Exhibition Winners", "Choir Tour Dates", "Instrument Loans"]
    c.story("arts", [(3, x, 110, w, 610) for x, w in C] + [(4, C[0][0], 40, C[0][1], 680),
            (4, C[1][0], 40, C[1][1], 680), (10, C[0][0], 60, C[0][1], 400)],
            prose(23, 3000, "school", heads=heads), "Verdana", 9, heads=heads)
    c.pic(4, "photo1.jpg", C[2][0], 40, C[2][1], 250)
    c.wa(5, "Sports Corner", 120, 24, preset=20, font="Arial Black", size=40, shape=21)
    c.story("sports", [(p, x, 110 if p == 5 else 40, w, 610 if p == 5 else 680) for p in (5, 6, 7) for x, w in C]
            + [(8, C[0][0], 40, C[0][1], 300)], prose(24, 5200, "school"), "Verdana", 9, align=4)
    c.tb(8, "Fixtures", C[1][0], 40, 300, 24, 14, "Verdana", bold=True)
    c.table(8, [["Date", "Team", "Opponent", "Venue"]] + [[f"{d}/11", t, o, v] for d, t, o, v in
            zip(range(2, 30, 3), ["U12", "U14", "U16", "1st XI", "Netball", "U13", "U15", "Hockey", "Rugby"],
                ["Fairview", "St. Anne's", "Kingsmead", "Oakfield", "Riverside", "Elmhurst", "Northgate", "Brookside",
                 "Westmoor"], ["Home", "Away"] * 5)], C[1][0], 70, C[1][1] * 2 + 14, 280, 8, "Verdana")
    c.story("head", [(8, C[0][0], 360, C[0][1], 350), (8, C[1][0], 370, C[1][1], 340), (11, C[0][0], 60, C[0][1], 300)],
            prose(25, 1200, "school"), "Verdana", 9)
    c.wa(11, "Diary Dates", 330, 30, preset=6, font="Arial", size=30, shape=11)
    c.story("diary", [(11, C[1][0], 120, C[1][1], 600), (11, C[2][0], 120, C[2][1], 600), (12, C[0][0], 40, C[0][1], 680)],
            prose(26, 1500, "school"), "Verdana", 9)
    c.tb(9, "Robotics (continued)", C[1][0], 270, 300, 24, 12, "Verdana", bold=True)
    c.pic(9, "photo0.jpg", C[0][0], 40, 540, 240)
    c.pic(10, "photo2.jpg", C[1][0], 60, C[1][1] * 2 + 14, 400)
    c.pic(12, "logo.png", C[1][0] + 60, 200, 200, 200)
    c.tb(12, "Hillcrest Academy · 14 Ridgeway Lane · 01632 960 451", C[1][0], 440, C[1][1] * 2 + 14, 40, 10, "Verdana",
         align=2)


def nl_backwards_6p(c):
    """Chains that jump backwards and skip around: p5 -> p2 -> p6 -> p1, p3 -> p1 -> p4."""
    c.pages(6)
    c.footer("Riverside Residents' Bulletin")
    c.wa(1, "Riverside Bulletin", 120, 20, preset=2, font="Georgia", size=40, bold=True)
    c.story("bridge", [(5, 36, 60, 540, 300), (2, 36, 400, 260, 320), (6, 316, 60, 260, 600), (1, 36, 500, 260, 220)],
            prose(31, 2300, "town"), "Times New Roman", 11)
    c.story("market", [(3, 36, 60, 260, 650), (1, 316, 120, 260, 360), (4, 36, 60, 540, 250)],
            prose(32, 1700, "town"), "Times New Roman", 11, align=4)
    c.story("mill", [(6, 36, 60, 260, 650), (4, 36, 340, 260, 370), (2, 36, 60, 540, 300)],
            prose(33, 1900, "town"), "Times New Roman", 11)
    c.tb(1, "Bridge works: the full story", 36, 470, 260, 24, 13, "Times New Roman", bold=True)
    c.pic(1, "photo3.jpg", 36, 120, 260, 330)
    c.pic(3, "photo1.jpg", 316, 60, 260, 300)
    c.pic(4, "photo2.jpg", 316, 340, 260, 370)
    c.pic(5, "photo0.jpg", 36, 380, 540, 330)
    c.tb(2, "From the Mill Trust", 316, 400, 260, 30, 14, "Times New Roman", bold=True)
    c.tb(2, prose(34, 160, "town"), 316, 440, 260, 280, 11, "Times New Roman")


def nl_tiny_boxes(c):
    """One story through 10 boxes of wildly different sizes (one-line strips, a 25 pt square,
    a 40 pt-wide column), plus a 3-box story that overflows."""
    c.pages(2)
    c.wa(1, "Odd Shapes Gazette", 140, 18, preset=11, font="Rockwell", size=32, bold=True, shape=3)
    boxes = [(1, 36, 100, 300, 260), (1, 350, 100, 60, 40), (1, 420, 100, 156, 16), (1, 350, 150, 226, 210),
             (1, 36, 370, 40, 340), (1, 86, 370, 490, 70), (1, 86, 450, 25, 25), (1, 120, 450, 456, 120),
             (2, 36, 40, 540, 30), (2, 36, 80, 250, 300)]
    c.story("odd", boxes, prose(41, 2600, "garden"), "Arial", 9)
    c.story("over", [(1, 86, 590, 230, 120), (1, 330, 590, 246, 120), (2, 300, 80, 276, 300)],
            prose(42, 2400, "garden"), "Arial", 10, fit=False)
    c.tb(2, "Plot holders' notes", 36, 400, 540, 24, 14, bold=True)
    c.tb(2, prose(43, 200, "garden"), 36, 430, 540, 280, 10)


def nl_overflow(c):
    """Stories much longer than their boxes; Publisher hides the excess. Also a single
    overfull box and a story that ends in its first of four boxes."""
    c.pages(2)
    c.tb(1, "Allotment Society Newsletter", 36, 30, 540, 40, 26, "Century Gothic", bold=True)
    c.story("long", [(1, 36, 90, 170, 300), (1, 221, 90, 170, 300), (1, 406, 90, 170, 300)],
            prose(51, 4000, "garden"), "Century Gothic", 10, fit=False)
    c.story("short", [(1, 36, 410, 260, 300), (1, 316, 410, 260, 140), (2, 36, 40, 260, 300), (2, 316, 40, 260, 300)],
            prose(52, 120, "garden"), "Century Gothic", 10)
    c.tb(1, prose(53, 600, "garden"), 316, 560, 260, 150, 10, "Century Gothic")
    c.story("big", [(2, 36, 360, 540, 160), (2, 36, 540, 540, 180)], prose(54, 3000, "garden"), "Century Gothic", 14, fit=False)


def nl_fonts_mix(c):
    """Four stories in four serif/sans fonts and sizes, each flowing across pages:
    the 'single line height calibrated for Arial' limit."""
    c.pages(4)
    c.footer("The Lantern · community magazine", font="Garamond")
    c.wa(1, "The Lantern", 170, 20, preset=4, font="Garamond", size=48, bold=True)
    specs = [("garamond", "Garamond", 12, "garden"), ("bookman", "Bookman Old Style", 9.5, "town"),
             ("century", "Century", 11, "parish"), ("trebuchet", "Trebuchet MS", 10, "school")]
    for i, (name, font, size, theme) in enumerate(specs):
        p1 = 1 + i % 2 * 1
        top = 110 if i < 2 else 420
        if i < 2:
            boxes = [(1, 36 + i * 280, 110, 260, 300), (3, 36 + i * 280, 40, 260, 330)]
        else:
            boxes = [(2, 36 + (i - 2) * 280, 40, 260, 680), (4, 36 + (i - 2) * 280, 40, 260, 330),
                     (4, 36 + (i - 2) * 280, 400, 260, 310)]
        c.story(name, boxes, prose(61 + i, 1300, theme), font, size)
    c.pic(1, "photo1.jpg", 36, 430, 540, 280)
    c.pic(3, "photo2.jpg", 36, 390, 540, 320)


def nl_columns_in_box(c):
    """Linked boxes that themselves have 2 or 3 columns."""
    c.pages(3)
    c.wa(1, "Harbour Lights", 150, 18, preset=7, font="Arial Black", size=40, shape=2)
    c.story("cols3", [(1, 36, 100, 540, 300), (2, 36, 40, 540, 340), (3, 36, 40, 540, 200)],
            prose(71, 3000, "town"), "Arial", 9, columns=3)
    c.story("cols2", [(1, 36, 420, 540, 290), (2, 36, 400, 540, 310), (3, 36, 260, 540, 450)],
            prose(72, 2600, "town"), "Arial", 9.5, columns=2, align=4)


def nl_wrap_pictures(c):
    """Linked columns with pictures and a pull-quote placed on top of them: Publisher wraps
    the text around the objects (square wrap, the default)."""
    c.pages(3)
    c.footer("Greenway Gardeners")
    c.wa(1, "Greenway Gardeners", 110, 18, preset=16, font="Georgia", size=38, shape=5)
    C = cols(3)
    c.story("wrap", [(1, x, 100, w, 610) for x, w in C] + [(2, x, 40, w, 680) for x, w in C] + [(3, C[0][0], 40, C[0][1], 400)],
            prose(81, 3600, "garden"), "Georgia", 10)
    c.pic(1, "photo1.jpg", C[0][0] + 60, 260, 220, 160)
    c.pic(2, "photo2.jpg", C[1][0] - 30, 300, 230, 170)
    q = c.tb(1, "“Every plot tells a story of patience.”", C[2][0], 500, C[2][1], 80, 14, "Georgia", italic=True,
             color=rgb(30, 100, 40), align=2)
    q.Line.ForeColor.RGB = rgb(30, 100, 40)
    q.Line.Weight = 1.5
    c.pic(3, "photo0.jpg", C[1][0], 40, C[1][1] * 2 + 14, 300)


def nl_interleaved_10p(c):
    """Two long stories alternating boxes page by page (A,B,A,B...) plus a sidebar story
    that skips every other page: 3 simultaneous chains of 5-10 boxes."""
    c.pages(10)
    c.footer("Northside Chronicle")
    c.wa(1, "Northside Chronicle", 90, 18, preset=1, font="Arial Black", size=40, shape=1)
    a, b, s = [], [], []
    for p in range(1, 11):
        top = 100 if p == 1 else 40
        a.append((p, 36, top, 250, 340))
        b.append((p, 36, top + 360, 250, 260 if p == 1 else 320))
        if p % 2:
            s.append((p, 310, top, 266, 300))
        else:
            c.pic(p, f"photo{p % 4}.jpg", 310, top, 266, 300)
        c.tb(p, prose(90 + p, 120, "town"), 310, top + 320, 266, 300 if p == 1 else 340, 9, "Arial", italic=True)
    c.story("alpha", a, prose(101, 3600, "town"), "Arial", 9.5)
    c.story("beta", b, prose(102, 3400, "parish"), "Times New Roman", 10.5)
    c.story("sidebar", s, prose(103, 1600, "garden"), "Verdana", 8.5)


def wa_shapes_gallery(c):
    """All 40 WordArt shapes (types 136-175), ten per page, different fonts and presets."""
    c.pages(4)
    fonts = ["Arial Black", "Impact", "Georgia", "Times New Roman", "Comic Sans MS", "Verdana", "Garamond",
             "Rockwell", "Century Gothic", "Trebuchet MS"]
    words = ["Harvest Fair", "Open Day", "Bake Sale", "Jazz Night", "Fun Run", "Book Club", "Car Boot", "Quiz Night",
             "Craft Fair", "Talent Show"]
    for k in range(40):
        p, i = 1 + k // 10, k % 10
        x, y = 50 + (i % 2) * 280, 30 + (i // 2) * 150
        c.wa(p, words[(k * 3) % 10], x, y, preset=1 + (k * 7) % 30, font=fonts[k % 10], size=28, bold=k % 3 != 2,
             italic=k % 4 == 1, shape=k + 1)
        c.tb(p, f"Shape {k + 1}", x, y + 118, 200, 16, 8, "Arial", italic=True, color=rgb(100, 100, 100))


def wa_rotated_styles(c):
    """WordArt rotated, flipped, bold/italic, 12-96 pt, assorted fonts and colours."""
    c.pages(2)
    rots = [0, 15, -30, 45, 90, 180, 270, -8, 330, 120]
    fonts = ["Arial Black", "Impact", "Georgia", "Brush Script MT", "Bodoni MT", "Broadway", "Engravers MT",
             "Gill Sans Ultra Bold", "Comic Sans MS", "Old English Text MT"]
    texts = ["Grand Opening", "SALE", "Welcome Home", "Summer Picnic", "Gala Dinner", "Jive Night", "ESTATE",
             "Big Match", "Kids Club", "Ye Olde Fayre"]
    pos = [(60, 40), (360, 60), (60, 230), (390, 250), (230, 420), (60, 560), (400, 520), (100, 680), (330, 690), (200, 300)]
    for i in range(10):
        c.wa(1, texts[i], *pos[i], preset=1 + i * 3, font=fonts[i], size=[24, 40, 28, 36, 30, 20, 26, 22, 32, 28][i],
             bold=i % 2 == 0, italic=i % 3 == 0, rotation=rots[i], color=rgb(40 * i % 255, 80, 200 - 15 * i))
    sizes = [12, 18, 24, 36, 48, 72, 96]
    y = 30
    for i, sz in enumerate(sizes):
        c.wa(2, f"Size {sz}", 40 + (i % 2) * 140, y, preset=[1, 2, 13, 17, 21, 25, 28][i], font="Arial", size=sz,
             bold=True, italic=i % 2 == 1, flip=(0 if i == 4 else 1 if i == 5 else None))
        y += sz * 1.3 + 14
    c.wa(2, "Upside Down", 360, 600, preset=5, font="Impact", size=36, rotation=180)


def wa_master_pages(c):
    """WordArt on the master page (shows on every page) plus a WordArt per page and
    a linked story running under it across all six pages."""
    c.pages(6)
    c.wa("M", "Riverside Garden Club", 150, 14, preset=12, font="Georgia", size=26, shape=9)
    c.wa("M", "DRAFT", 190, 340, preset=1, font="Arial Black", size=90, rotation=-35, color=rgb(220, 220, 220))
    c.footer("Riverside Garden Club yearbook")
    titles = ["Spring Bulbs", "Rose Pruning", "Pond Life", "Bees & Wasps", "Winter Veg", "Show Results"]
    boxes = []
    for p in range(1, 7):
        c.wa(p, titles[p - 1], 60, 110, preset=1 + p * 4, font=["Impact", "Garamond", "Arial Black", "Comic Sans MS",
                                                                  "Rockwell", "Century Gothic"][p - 1], size=30,
             shape=[17, 18, 21, 25, 3, 13][p - 1])
        boxes.append((p, 36, 220, 260, 480))
        boxes.append((p, 316, 220, 260, 480))
    c.story("club", boxes, prose(121, 6000, "garden"), "Arial", 10)


def wa_unicode(c):
    """WordArt with accents and non-Latin scripts: French, German, Spanish, Polish, Turkish,
    Czech, Icelandic, Greek, Cyrillic, Vietnamese, Arabic, Hebrew."""
    c.pages(2)
    texts = ["Fête de la Musique", "Größtes Straßenfest", "¡Año Nuevo Español!", "Zażółć gęślą jaźń",
             "İstanbul Çarşı Şöleni", "Přehlídka řemesel", "Þorrablót Ísland", "Ελληνική Γιορτή",
             "Русский праздник", "Tết Nguyên Đán", "مهرجان الربيع", "חג שמח לכולם"]
    fonts = ["Georgia", "Arial Black", "Comic Sans MS", "Arial", "Times New Roman", "Verdana", "Garamond",
             "Times New Roman", "Arial", "Arial", "Arial", "Arial"]
    for i, t in enumerate(texts):
        p, j = 1 + i // 6, i % 6
        c.wa(p, t, 50, 30 + j * 120, preset=[1, 3, 8, 12, 15, 19, 22, 26, 2, 6, 1, 9][i], font=fonts[i], size=24,
             bold=i % 2 == 0, shape=[1, 9, 21, 17, 25, 3, 13, 37, 1, 10, 1, 21][i])
    c.story("intl", [(1, 440, 40, 136, 700), (2, 440, 40, 136, 700)],
            "Bienvenue à la fête! Größere Straßenfeste, señoritas y niños, żółte kwiaty, İstanbul çarşısı, "
            "Ελληνικά γράμματα, русский текст, Tiếng Việt có dấu. " * 25, "Arial", 9)


def wa_dense_flyer(c):
    """One busy page: eight WordArt objects around two linked text columns and a photo."""
    c.wa(1, "COMMUNITY FUN DAY", 60, 16, preset=15, font="Impact", size=44, shape=9)
    c.wa(1, "Free Entry!", 420, 120, preset=8, font="Comic Sans MS", size=24, rotation=-12, shape=11)
    c.wa(1, "Bouncy Castle", 40, 120, preset=21, font="Arial Black", size=18, shape=21)
    c.wa(1, "Live Band", 230, 150, preset=10, font="Broadway", size=22, shape=3)
    c.pic(1, "photo0.jpg", 36, 210, 540, 200)
    c.story("fun", [(1, 36, 430, 170, 220), (1, 221, 430, 170, 220), (1, 406, 430, 170, 150)],
            prose(141, 1000, "town"), "Arial", 9)
    c.wa(1, "Sat 12 July", 410, 600, preset=3, font="Georgia", size=22, shape=17)
    c.wa(1, "10am-4pm", 50, 670, preset=24, font="Arial Black", size=20, shape=27)
    c.wa(1, "Raffle", 250, 670, preset=27, font="Rockwell", size=22, rotation=8, shape=33)
    c.wa(1, "Food Stalls", 420, 690, preset=5, font="Brush Script MT", size=26, bold=False, shape=1)


def programme_event_8p(c):
    """A5 event programme booklet: cover WordArt, running order, speaker bios
    as one story over five boxes on four pages, sponsors page with WordArt."""
    c.size(419.5, 595.3)  # A5: a paper size the PDF printer has, so Publisher exports one page per sheet
    c.pages(8)
    c.wa(1, "Midsummer Festival", 40, 60, preset=18, font="Bodoni MT", size=34, shape=9)
    c.wa(1, "2026", 130, 200, preset=24, font="Arial Black", size=48, shape=1)
    c.pic(1, "photo1.jpg", 36, 330, 324, 200)
    c.tb(1, "Official Programme · £2", 36, 550, 324, 24, 12, "Georgia", italic=True, align=2)
    c.tb(2, "Running order", 36, 36, 324, 26, 16, "Georgia", bold=True)
    c.table(2, [["Time", "Act", "Stage"]] + [[f"{h}:{m:02d}", a, s] for h, m, a, s in zip(
        [10, 10, 11, 12, 13, 13, 14, 15, 16, 17, 18, 19], [0, 45, 30, 15, 0, 40, 30, 15, 0, 0, 30, 15],
        ["Brass band", "Morris dancers", "Children's choir", "Poetry slam", "Folk duo", "Magic show", "Jazz trio",
         "Dog agility", "Ceilidh", "Tug of war", "Headliner", "Fireworks"], ["Main", "Green", "Main", "Tent"] * 3)],
        36, 70, 324, 420, 9, "Georgia")
    c.tb(3, "Meet the performers", 36, 36, 324, 26, 16, "Georgia", bold=True)
    c.story("bios", [(3, 36, 70, 324, 500), (4, 36, 36, 155, 530), (4, 205, 36, 155, 530), (5, 36, 300, 324, 270),
                     (6, 36, 36, 324, 250)], prose(151, 2300, "town"), "Georgia", 9, space_after=3)
    c.pic(5, "portrait.jpg", 120, 36, 156, 240)
    c.wa(6, "Food & Drink", 60, 310, preset=4, font="Georgia", size=28, shape=17)
    c.tb(6, prose(152, 140, "town"), 36, 400, 324, 170, 9, "Georgia")
    c.tb(7, "Site map", 36, 36, 324, 26, 16, "Georgia", bold=True)
    c.pic(7, "photo3.jpg", 36, 70, 324, 300)
    c.tb(7, prose(153, 120, "town"), 36, 390, 324, 180, 9, "Georgia")
    c.wa(8, "Thank You", 70, 60, preset=13, font="Brush Script MT", size=44, bold=False, shape=9)
    c.tb(8, "Our sponsors\r" + "\r".join(f"{a} {b} & Sons" for a, b in zip(FIRST[:10], LAST[:10])), 36, 200, 324, 300,
         11, "Georgia", align=2)


def church_bulletin_4p(c):
    """Folded landscape bulletin: two panels per page, announcements story running
    p1 right -> p2 left -> p3 right -> p4 left, WordArt welcome, hymn table."""
    c.size(792, 612)
    c.pages(4)
    c.wa(1, "Welcome", 470, 40, preset=13, font="Brush Script MT", size=54, bold=False, shape=9)
    c.tb(1, "Grace Community Church\r27th Sunday in Ordinary Time", 432, 160, 324, 50, 14, "Garamond", align=2)
    c.pic(1, "photo2.jpg", 432, 230, 324, 250)
    c.story("ann", [(1, 36, 60, 324, 520), (2, 36, 60, 324, 520), (3, 432, 60, 324, 520), (4, 36, 60, 324, 260)],
            prose(161, 3100, "parish"), "Garamond", 11, cont=True)
    c.tb(1, "Announcements", 36, 30, 324, 24, 14, "Garamond", bold=True)
    c.tb(2, "Order of Service", 432, 30, 324, 24, 16, "Garamond", bold=True, align=2)
    c.table(2, [["", "Hymn / reading", "No."]] + [[str(i + 1), t, str(100 + i * 37)] for i, t in enumerate(
        ["Gathering hymn", "First reading", "Psalm", "Second reading", "Gospel acclamation", "Offertory hymn",
         "Communion hymn", "Recessional"])], 432, 60, 324, 300, 10, "Garamond")
    c.wa(3, "Prayer List", 60, 40, preset=3, font="Garamond", size=30, shape=17)
    c.tb(3, "\r".join(f"{a} {b}" for a, b in zip(FIRST, LAST)), 36, 120, 324, 450, 11, "Garamond")
    c.wa(4, "Coffee Rota", 470, 40, preset=2, font="Garamond", size=30)
    c.table(4, [["Week", "Hosts"]] + [[f"{d} Nov", f"{FIRST[i]} & {FIRST[i + 7]}"] for i, d in enumerate([2, 9, 16, 23, 30])],
            432, 120, 324, 220, 10, "Garamond")
    c.tb(4, prose(162, 100, "parish"), 36, 340, 324, 230, 10, "Garamond", italic=True)


def yearbook_6p(c):
    """Yearbook pages: 4x4 portrait grids with name captions, WordArt class headings,
    and a 'Year in Review' story across the last two pages."""
    c.pages(6)
    classes = ["Class 7 Oak", "Class 7 Ash", "Class 8 Birch", "Class 8 Rowan"]
    k = 0
    for p in range(1, 5):
        c.wa(p, classes[p - 1], 150, 20, preset=[9, 16, 22, 28][p - 1], font=["Comic Sans MS", "Arial Black", "Georgia",
                                                                            "Impact"][p - 1], size=34,
             shape=[9, 21, 17, 3][p - 1])
        for r in range(4):
            for col in range(4):
                x, y = 48 + col * 132, 110 + r * 160
                c.pic(p, "portrait.jpg", x, y, 100, 120)
                c.tb(p, f"{FIRST[k % 30]} {LAST[(k * 7) % 18]}", x - 10, y + 124, 120, 28, 8, "Arial", align=2)
                k += 1
    c.wa(5, "Year in Review", 140, 20, preset=19, font="Rockwell", size=36, shape=13)
    c.story("review", [(5, 36, 100, 260, 340), (5, 316, 100, 260, 340), (6, 36, 40, 540, 200), (6, 36, 260, 260, 450)],
            prose(171, 2200, "school"), "Arial", 10)
    c.pic(5, "photo0.jpg", 36, 460, 540, 250)
    c.pic(6, "photo3.jpg", 316, 260, 260, 450)


def certificates_30p(c):
    """Bulk certificates: 30 landscape pages, title WordArt on the master page, each
    recipient's name as its own WordArt (some with accents), border, signature lines."""
    c.size(792, 612)
    c.pages(30)
    c.rect("M", 24, 24, 744, 564, None, line=(rgb(170, 130, 30), 6))
    c.rect("M", 36, 36, 720, 540, None, line=(rgb(170, 130, 30), 1.5))
    c.wa("M", "Certificate of Completion", 130, 60, preset=3, font="Old English Text MT", size=40, bold=False, shape=9)
    c.tb("M", "This is to certify that", 246, 200, 300, 24, 16, "Garamond", italic=True, align=2)
    c.tb("M", "has successfully completed the Community First Aid course, 2026", 146, 360, 500, 24, 15, "Garamond",
         align=2)
    for x, lab in ((110, "Course leader"), (480, "Chair of trustees")):
        c.hline("M", x, 500, x + 200, 1)
        c.tb("M", lab, x, 504, 200, 18, 10, "Garamond", align=2)
    names = [f"{a} {b}" for a, b in zip(FIRST, LAST + LAST)]
    names[3] = "Zoë Brontë-Müller"
    names[9] = "José Ñúñez García"
    names[17] = "Łukasz Wójcik"
    names[24] = "Søren Ågård"
    for p in range(1, 31):
        c.wa(p, names[p - 1], 180, 250, preset=[1, 4, 13, 17][p % 4], font=["Brush Script MT", "Garamond", "Georgia",
                                                                           "Bodoni MT"][p % 4], size=40,
             bold=p % 4 == 2, italic=p % 4 == 1, shape=[1, 1, 17, 9][p % 4])
        c.tb(p, f"Certificate no. FA-2026-{p:03d}", 36, 560, 200, 14, 7, "Arial", color=rgb(120, 120, 120))


def menu_trifold(c):
    """Tri-fold restaurant menu (landscape, 2 sides): WordArt panel headings, dish lists,
    and the 'Our story' text flowing panel to panel across both sides."""
    c.size(792, 612)
    c.pages(2)
    P = [(30, 228), (282, 228), (534, 228)]
    c.wa(1, "Trattoria Lucia", 545, 40, preset=13, font="Brush Script MT", size=38, bold=False, shape=9)
    c.pic(1, "photo2.jpg", 534, 150, 228, 200)
    c.tb(1, "Via Roma 12 · Tel 01632 960 118\rOpen Tue-Sun 12-22", 534, 370, 228, 50, 11, "Garamond", align=2)
    c.wa(1, "La Nostra Storia", 290, 40, preset=3, font="Garamond", size=20, shape=17)
    c.story("storia", [(1, 282, 110, 228, 470), (1, 30, 330, 228, 250), (2, 534, 420, 228, 160)],
            prose(191, 1400, "town"), "Garamond", 10.5, align=4)
    c.pic(1, "photo0.jpg", 30, 40, 228, 270)
    sections = [("Antipasti", 2, 0), ("Primi", 2, 1), ("Secondi", 2, 2)]
    dishes = ["Bruschetta al pomodoro", "Carpaccio di manzo", "Burrata e pesche", "Arancini al ragù", "Tagliatelle al tartufo",
              "Risotto ai funghi porcini", "Gnocchi di zucca", "Linguine alle vongole", "Saltimbocca alla romana",
              "Branzino al forno", "Ossobuco milanese", "Pollo alla cacciatora"]
    for i, (sec, pg, col) in enumerate(sections):
        x, w = P[col]
        c.wa(pg, sec, x + 20, 30, preset=[4, 13, 22][i], font="Georgia", size=28, shape=[17, 9, 1][i])
        items = "\r".join(f"{d}  ·  €{9 + (j * 3) % 14},50" for j, d in enumerate(dishes[i * 4:i * 4 + 4] * (2 if i < 2 else 1)))
        c.tb(pg, items, x, 110, w, 280 if col == 2 else 470, 11, "Garamond")


def brochure_trifold(c):
    """Tri-fold brochure where the main story starts on the inside and finishes on the
    outside (backwards jump), WordArt cover, pictures."""
    c.size(792, 612)
    c.pages(2)
    c.wa(1, "Discover" + chr(13) + "Ashford Valley", 550, 50, preset=19, font="Rockwell", size=22, shape=17)
    c.pic(1, "photo1.jpg", 534, 160, 228, 300)
    c.tb(1, "Walks · Heritage · Food", 534, 480, 228, 30, 14, "Rockwell", align=2)
    c.story("valley", [(2, 30, 40, 228, 540), (2, 282, 240, 228, 340), (2, 534, 40, 228, 540), (1, 30, 40, 228, 300),
                       (1, 282, 40, 228, 540)], prose(201, 3200, "town"), "Trebuchet MS", 9.5, space_after=4)
    c.pic(2, "photo3.jpg", 282, 40, 228, 180)
    c.tb(1, "Visitor centre\r" + prose(202, 60, "town"), 30, 360, 228, 220, 9, "Trebuchet MS")


def calendar_12p(c):
    """12-month landscape wall calendar: WordArt month names (a different shape each
    month), a table grid of days, and an events note box."""
    import calendar as cal
    c.size(792, 612)
    c.pages(12)
    months = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October",
              "November", "December"]
    for m in range(12):
        p = m + 1
        c.wa(p, f"{months[m]} 2027", 220, 10, preset=1 + (m * 5) % 30, font=["Georgia", "Arial Black", "Impact",
                                                                             "Comic Sans MS"][m % 4], size=36,
             shape=[1, 9, 17, 21, 3, 13, 25, 37, 5, 11, 27, 33][m])
        weeks = cal.Calendar(0).monthdayscalendar(2027, m + 1)
        data = [["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]] + [[str(d) if d else "" for d in w] for w in weeks]
        c.table(p, data, 36, 110, 560, 460, 12, "Arial")
        c.tb(p, "Events\r" + prose(210 + m, 50, "town"), 610, 110, 146, 460, 9, "Arial")


def poster_concert(c):
    """Tabloid (11x17) gig poster: full-bleed photo, six rotated WordArt objects over it."""
    c.size(792, 1224)
    c.pic(1, "photo3.jpg", 0, 0, 792, 1224)
    c.rect(1, 40, 820, 712, 360, rgb(0, 0, 0), transp=0.35)
    c.wa(1, "THE HOLLOW PINES", 60, 60, preset=15, font="Impact", size=80, shape=9, color=rgb(255, 230, 0))
    c.wa(1, "Live in Concert", 180, 330, preset=13, font="Brush Script MT", size=60, bold=False, rotation=-10)
    c.wa(1, "Special guests", 80, 560, preset=6, font="Arial Black", size=30, rotation=6, shape=21)
    c.wa(1, "Velvet Static", 420, 600, preset=26, font="Broadway", size=34, rotation=-6, shape=25)
    c.wa(1, "Sat 21 Nov", 70, 840, preset=1, font="Arial Black", size=48, color=rgb(255, 255, 255))
    c.wa(1, "Doors 7:30pm", 430, 860, preset=2, font="Impact", size=36, rotation=4, color=rgb(255, 255, 255))
    c.tb(1, "The Corn Exchange · Tickets £14 advance / £17 door · All ages until 10pm\r"
            "theholowpines.example · No re-admission", 60, 1000, 672, 80, 18, "Arial", bold=True,
         color=rgb(255, 255, 255), align=2)


def report_40p(c):
    """40-page annual report: 8 chapters, each one story flowing over ~5 pages in 2-column
    boxes (chains of 9-10 boxes), chapter WordArt, master header/footer."""
    c.pages(40)
    c.tb("M", "Ashford Valley Community Trust · Annual Report 2025/26", 36, 18, 540, 16, 8, "Arial", italic=True,
         color=rgb(110, 110, 110))
    c.footer("Annual Report 2025/26", font="Arial")
    chapters = ["Chair's Report", "Our Year", "Food Bank", "Youth Work", "Green Spaces", "Heritage", "Finances",
                "Looking Ahead"]
    themes = ["town", "parish", "parish", "school", "garden", "town", "town", "school"]
    for ch in range(8):
        first = 1 + ch * 5
        c.wa(first, chapters[ch], 60, 50, preset=[1, 3, 4, 9, 13, 16, 17, 2][ch], font="Georgia", size=40,
             shape=[1, 17, 9, 21, 3, 13, 1, 25][ch])
        boxes = [(first, 36, 160, 260, 560), (first, 316, 160, 260, 560)]
        for p in range(first + 1, first + 5):
            boxes += [(p, 36, 50, 260, 680), (p, 316, 50, 260, 680)]
        c.story(f"ch{ch + 1}", boxes, prose(300 + ch, 5600, themes[ch], heads=[f"{chapters[ch]}: part {i}" for i in range(1, 4)]),
                "Arial", 10, space_after=6, align=4)


def magazine_60p(c):
    """60-page community magazine mixing everything: 10 articles of 12 boxes each that jump
    pages (with continued notices), WordArt on every third page and on the master,
    tables and photos."""
    c.pages(60)
    c.wa("M", "VALLEY LIFE", 470, 10, preset=1, font="Arial Black", size=14)
    c.footer("Valley Life magazine · Winter 2026", font="Verdana")
    rng = random.Random(5)
    themes = list(THEMES)
    for art in range(10):
        start = 1 + art * 6
        boxes = []
        pages_used = [start, start + 1, start + 2, start + 3]
        jump = min(60, start + 6 + rng.randint(0, 20)) if art < 9 else 60
        for p in pages_used:
            top = 150 if p == start else 110
            boxes += [(p, 36, top, 170, 690 - top), (p, 221, top, 170, 690 - top), (p, 406, top, 170, 300)]
        boxes = boxes[:11] + [(start + 4, 36, 380, 540, 330)]
        c.story(f"art{art + 1}", boxes, prose(400 + art, 6500, themes[art % 4]), ["Verdana", "Georgia", "Arial",
                                                                                     "Times New Roman"][art % 4],
                [9, 10, 9.5, 10.5][art % 4], align=4 if art % 2 else None)
        c.tb(start, f"Feature {art + 1}", 36, 40, 300, 20, 10, "Verdana", italic=True)
        for p in pages_used[1:]:
            c.pic(p, f"photo{p % 4}.jpg", 406, 430, 170, 260)
        c.pic(start + 4, "photo1.jpg", 36, 50, 540, 310)
        c.table(start + 5, [["When", "What", "Where"]] + [[f"{d} Dec", e, pl] for d, e, pl in zip(
            range(1, 25, 3), THEMES[themes[art % 4]]["event"], THEMES[themes[art % 4]]["place"])], 36, 400, 540, 300, 9)
    for p in range(1, 61, 3):
        c.wa(p, ["Feature", "Spotlight", "On the Town", "Green Pages", "Young Voices", "History Corner", "Food & Drink",
                 "Letters", "Puzzles", "What's On"][(p // 3) % 10] + f" {p}", 340,
             40, preset=1 + p % 30, font=["Impact", "Georgia", "Arial Black", "Rockwell"][p % 4], size=26,
             shape=1 + (p * 3) % 40)
    for p in range(6, 61, 6):
        c.tb(p, prose(500 + p, 200, "town"), 36, 50, 540, 330, 10, "Georgia")


def nl_grouped_masthead(c):
    """Newsletter whose WordArt masthead is grouped with its banner rectangle and a
    grouped photo + caption (the README's 'grouped shapes' limit), with linked text."""
    c.pages(2)
    r = c.rect(1, 36, 20, 540, 90, rgb(20, 60, 110))
    w = c.wa(1, "Meadowbank Monthly", 80, 34, preset=1, font="Georgia", size=36, color=rgb(255, 255, 255))
    c.doc.Pages(1).Shapes.Range([r.Name, w.Name]).Group()
    p = c.pic(1, "photo2.jpg", 316, 130, 260, 200)
    cap = c.tb(1, "The new footbridge on opening day.", 316, 334, 260, 20, 8, italic=True)
    c.doc.Pages(1).Shapes.Range([p.Name, cap.Name]).Group()
    c.story("bridge", [(1, 36, 130, 260, 590), (1, 316, 370, 260, 350), (2, 36, 40, 540, 300)],
            prose(601, 2300, "town"), "Arial", 10)
    w2 = c.wa(2, "Diary", 60, 380, preset=9, font="Impact", size=30, shape=21)
    b2 = c.rect(2, 300, 380, 276, 60, rgb(250, 230, 120))
    c.doc.Pages(2).Shapes.Range([w2.Name, b2.Name]).Group()
    c.tb(2, prose(602, 200, "town"), 36, 460, 540, 250, 10)


DOCS = {f.__name__: f for f in [
    nl_parish_8p, nl_school_12p, nl_backwards_6p, nl_tiny_boxes, nl_overflow, nl_fonts_mix, nl_columns_in_box,
    nl_wrap_pictures, nl_interleaved_10p, nl_grouped_masthead, wa_shapes_gallery, wa_rotated_styles,
    wa_master_pages, wa_unicode, wa_dense_flyer, programme_event_8p, church_bulletin_4p, yearbook_6p,
    certificates_30p, menu_trifold, brochure_trifold, calendar_12p, poster_concert, report_40p, magazine_60p]}


# ================================================================== worker
def worker(names):
    import win32com.client
    METAD.mkdir(parents=True, exist_ok=True)
    app = win32com.client.Dispatch("Publisher.Application")
    try:
        for name in names:
            t0 = time.time()
            doc = app.NewDocument()
            try:
                ctx = Ctx(doc, name)
                DOCS[name](ctx)
                meta = ctx.collect()
                meta["about"] = " ".join(DOCS[name].__doc__.split())
                doc.SaveAs(str(OUT / f"{name}.pub"))
                doc.ExportAsFixedFormat(PDF, str(REF / f"{name}.ref.pdf"))
                fit_ref_pages(REF / f"{name}.ref.pdf", meta["page_w"], meta["page_h"])
                (METAD / f"{name}.json").write_text(json.dumps(meta, ensure_ascii=False, indent=1), encoding="utf-8")
                print(f"OK {name} {meta['pages']}p {time.time() - t0:.0f}s", flush=True)
            except Exception as e:
                print(f"FAILED {name} {e!r}", flush=True)
                traceback.print_exc()
            finally:
                try:
                    doc.Close()
                except Exception:
                    pass
    finally:
        try:
            app.Quit()
        except Exception:
            pass
    os._exit(0)  # Publisher's COM teardown can hang the interpreter


def fit_ref_pages(pdf, w, h):
    """Publisher prints a page smaller than the printer's paper centred on that paper
    (e.g. A5 on A4). Cut each sheet back to the publication's page size."""
    import fitz
    src = fitz.open(pdf)
    r = src[0].rect
    if abs(r.width - w) < 1 and abs(r.height - h) < 1:
        return
    out = fitz.open()
    for i in range(len(src)):
        sr = src[i].rect
        x, y = (sr.width - w) / 2, (sr.height - h) / 2
        out.new_page(width=w, height=h).show_pdf_page(fitz.Rect(0, 0, w, h), src, i, clip=fitz.Rect(x, y, x + w, y + h))
    tmp = pdf.with_suffix(".fit")
    out.save(tmp, garbage=3, deflate=True)
    out.close()
    src.close()
    os.replace(tmp, pdf)


def kill_publisher():
    subprocess.run(["taskkill", "/F", "/IM", "MSPUB.EXE"], capture_output=True)


# ================================================================== privacy scrub
def _scrub_list():
    """Same-length replacements for this PC's user name. Publisher writes it into
    files; it's read from Windows here so it never has to be written down."""
    user = os.environ.get("USERNAME", "")
    parts = [p for p in re.split(r"[.\s_-]+", user) if len(p) >= 3]
    fill = lambda n, base: (base * (n // len(base) + 1))[:n]
    out = []
    if user:
        out.append((user, fill(len(user), "pubconv-dev")))
        try:  # 8.3 short folder name, e.g. FIRSTN~1.LAS
            import ctypes
            buf = ctypes.create_unicode_buffer(260)
            ctypes.windll.kernel32.GetShortPathNameW(os.environ["USERPROFILE"], buf, 260)
            short = Path(buf.value).name
            if short.lower() != user.lower():
                out.append((short, fill(len(short), "pubco~1.dev")))
        except Exception:
            pass
    if len(parts) > 1:
        full = " ".join(parts)
        out.append((full, fill(len(full), "Test Author")))
    out += [(p, fill(len(p), "tests")) for p in parts]
    return out


SCRUB = _scrub_list()


def _ci(word, utf16):
    parts = []
    for ch in word:
        if ch.isalpha():
            cls = b"[" + re.escape(ch.lower().encode()) + re.escape(ch.upper().encode()) + b"]"
        else:
            cls = re.escape(ch.encode())
        parts.append(cls + (b"\x00" if utf16 else b""))
    return re.compile(b"".join(parts))


def scrub_bytes(data):
    n = 0
    for old, new in SCRUB:
        assert len(old) == len(new)
        for utf16 in (False, True):
            def rep(m, new=new, utf16=utf16):
                s = m.group(0)
                src = s[::2].decode("latin-1") if utf16 else s.decode("latin-1")
                out = new.upper() if src.isupper() else new.lower() if src.islower() else new
                return out.encode("utf-16-le") if utf16 else out.encode("latin-1")
            data, k = _ci(old, utf16).subn(rep, data)
            n += k
    return data, n


def scrub_all():
    import fitz
    total = 0
    for pdf in sorted(REF.glob("*.ref.pdf")):
        d = fitz.open(pdf)
        d.set_metadata({"title": d.metadata.get("title", ""), "author": "", "creator": "Microsoft Publisher",
                        "producer": "Microsoft Publisher", "subject": "", "keywords": ""})
        try:
            d.del_xml_metadata()
        except Exception:
            pass
        tmp = pdf.with_suffix(".tmp")
        d.save(tmp, garbage=3, deflate=True)
        d.close()
        os.replace(tmp, pdf)
    for f in sorted(list(OUT.glob("*.pub")) + list(REF.glob("*.ref.pdf")) + list(REF.glob("*.json"))):
        data = f.read_bytes()
        new, n = scrub_bytes(data)
        if n:
            assert len(new) == len(data)
            f.write_bytes(new)
            total += n
    print(f"scrubbed {total} occurrences", flush=True)
    return verify_clean()


def verify_clean():
    import fitz
    bad = []
    words = [p for p, _ in SCRUB if len(p) >= 3 and p.replace(" ", "").isalpha()] or ["pubconv-no-user-name"]
    pat = [_ci(w, u) for w in words for u in (False, True)]
    for f in sorted(list(OUT.glob("*")) + list(REF.glob("*"))):
        if any(p.search(f.read_bytes()) for p in pat):
            bad.append(f.name)
        if f.suffix == ".pdf":  # also look inside compressed streams
            d = fitz.open(f)
            for x in range(1, d.xref_length()):
                try:
                    s = d.xref_stream(x) if d.xref_is_stream(x) else d.xref_object(x).encode()
                except Exception:
                    continue
                if s and any(p.search(s) for p in pat):
                    bad.append(f"{f.name} (xref {x})")
                    break
    print("privacy check:", "CLEAN" if not bad else f"FOUND in {bad}", flush=True)
    return not bad


# ================================================================== main
def main():
    args = sys.argv[1:]
    if args and args[0] == "--worker":
        worker(args[1:])
        return
    OUT.mkdir(parents=True, exist_ok=True)
    REF.mkdir(parents=True, exist_ok=True)
    if args == ["--scrub"]:
        sys.exit(0 if scrub_all() else 1)
    names = args or list(DOCS)
    unknown = [n for n in names if n not in DOCS]
    if unknown:
        sys.exit(f"unknown: {unknown}")
    make_images()
    kill_publisher()
    failed = []
    # big documents get their own batch
    big = {"magazine_60p", "report_40p", "certificates_30p"}
    batches = [[n] for n in names if n in big]
    small = [n for n in names if n not in big]
    batches += [small[i:i + BATCH] for i in range(0, len(small), BATCH)]
    for b in batches:
        print("batch:", " ".join(b), flush=True)
        try:
            p = subprocess.run([sys.executable, "-u", __file__, "--worker", *b], timeout=TIMEOUT, text=True,
                               capture_output=True, encoding="utf-8", errors="replace")
            out = p.stdout + p.stderr
        except subprocess.TimeoutExpired as e:
            out = (e.stdout or b"").decode("utf-8", "replace") if isinstance(e.stdout, bytes) else (e.stdout or "")
            out += "\nTIMEOUT"
        print(out.strip(), flush=True)
        failed += [n for n in b if f"OK {n} " not in out]
        kill_publisher()
        time.sleep(1)
    meta = {}
    mf = REF / "hard_meta.json"
    if mf.exists():
        meta = json.loads(mf.read_text(encoding="utf-8"))
    for n in names:
        f = METAD / f"{n}.json"
        if f.exists() and n not in failed:
            meta[n] = json.loads(f.read_text(encoding="utf-8"))
    mf.write_text(json.dumps(dict(sorted(meta.items())), ensure_ascii=False, indent=1), encoding="utf-8")
    ok = scrub_all()
    print(f"\nmade {len(names) - len(failed)}/{len(names)}" + (f"; FAILED: {failed}" if failed else ""))
    sys.exit(0 if ok and not failed else 1)


if __name__ == "__main__":
    main()
