"""Rebuild N4 Grid Sans with fontTools. All stroke drawings below are original. CC0-1.0."""
from math import hypot
from pathlib import Path

from fontTools.feaLib.builder import addOpenTypeFeaturesFromString
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen

# Each pair is a pen position (column, row). A space starts another stroke.
# Row 2 is the baseline, row 8 is cap height, rows 0 and 1 are descenders.
DRAWINGS = {
    "A":"02284642 1444", "B":"0208283746250525 2544332302",
    "C":"47281807030312023243", "D":"0208284737433202",
    "E":"48080242 0545", "F":"020848 0535", "G":"472818070303123243454525",
    "H":"0802 4842 0545", "I":"0818 1812 0222", "J":"3843221203",
    "K":"0802 480542", "L":"080242", "M":"0208254842",
    "N":"02084248", "O":"183847433212030718", "P":"02083847463505",
    "Q":"183847433212030718 2441", "R":"02083847463505 2542",
    "S":"473818070615354443321203", "T":"0848 2822",
    "U":"080312324348", "V":"082248", "W":"0812253248",
    "X":"0842 4802", "Y":"082548 2522", "Z":"08480242",
    "a":"0636454232 45150403123243", "b":"0802 0516364543321203",
    "c":"4536160503123243", "d":"4842 4536160503123243",
    "e":"04444536160503123243", "f":"12362738 0626",
    "g":"4536160503123243 4641301001", "h":"0802 0516364542",
    "i":"1612 1818", "j":"26211000 2828", "k":"0802 360432",
    "l":"181222", "m":"0602 05162522 25364542",
    "n":"0602 0516364542", "o":"163645433212030516",
    "p":"0600 0516364543321203", "q":"4640 4536160503123243",
    "r":"0602 05162635", "s":"45361605043443321203",
    "t":"17132232 0626", "u":"0603123243 4642", "v":"062246",
    "w":"0612243246", "x":"0642 4602", "y":"062246 221000",
    "z":"06460242", "0":"183847433212030718 1337", "1":"071822 0232",
    "2":"071838474635040242", "3":"07183847463525 354443321203",
    "4":"4248 380444", "5":"480806364543321203", "6":"473818070312324344351504",
    "7":"084812", "8":"183847463515060718 153544433212030415",
    "9":"433515060718384743321203",
    ".":"1212", ",":"1311", ":":"1616 1212", ";":"1616 1311",
    "!":"1814 1212", "?":"0718384746352424 2222",
    "-":"0545", "_":"0242", "+":"0545 2723", "=":"0646 0444",
    "/":"0248", "\\":"0842", "(":"280706041322", ")":"182726241312",
    "[":"2808080222", "]":"0828282202", "|":"1810", "'":"1816",
    '"':"0806 2826", "#":"1822 3842 0747 0545", "%":"0248 0818170708 3343433233",
    "&":"420607182827160503123244", "*":"2528 072443 472403",
    "<":"370527", ">":"174517", "@":"413212030718384746443434362625232333",
    "$":"2811 473818070615354443321203", "^":"062846", "`":"0817",
    "~":"0416253546", "{":"38272615242332", "}":"18272635242312",
    "·":"2525", "×":"0642 4602", "–":"0545", "—":"0575", "…":"0202 2222 4242",
    "‹":"260425", "›":"162415", "←":"250504 0517 0545", "→":"0545 274523",
    "−":"0545", "א":"0842 26063748 240302", "ב":"0838474202", "ג":"1827262203 2542",
    "é":"04444536160503123243 2738", "\u0301":"2738",
}

def glyph_name(char):
    return "uni%04X" % ord(char)

def contours(drawing, x_offset=0):
    """Stroke square-ended segments using clockwise contours with overlap at the joins."""
    polygons = []
    for stroke in drawing.split():
        points = [(85 + x_offset + int(stroke[i]) * 100, (int(stroke[i+1])-2) * 110)
                  for i in range(0, len(stroke), 2)]
        for (x,y), (u,v) in zip(points, points[1:]):
            length = hypot(u-x, v-y)
            dx, dy = ((u-x)/length, (v-y)/length) if length else (1, 0)
            half = 43
            corners = [(x-half*dx-half*dy,y-half*dy+half*dx),
                       (u+half*dx-half*dy,v+half*dy+half*dx),
                       (u+half*dx+half*dy,v+half*dy-half*dx),
                       (x-half*dx+half*dy,y-half*dy-half*dx)]
            polygons.append([(round(a),round(b)) for a,b in corners])
    return polygons

def main():
    outlines = {".notdef": contours("0802424808 0842"), "space": []}
    advances = {".notdef": 570, "space": 265}
    cmap = {32: "space"}
    for char, drawing in DRAWINGS.items():
        name = glyph_name(char)
        polygons = contours(drawing)
        outlines[name] = polygons
        right = max(x for contour in polygons for x, _ in contour)
        advances[name] = 0 if char == "\u0301" else right + 45
        cmap[ord(char)] = name
    # The shaping regression exercises one real ffi substitution and a combining accent.
    outlines["ffi"] = []
    advance = 0
    for char in "ffi":
        outlines["ffi"].extend(contours(DRAWINGS[char], advance))
        advance += advances[glyph_name(char)] - 15
    advances["ffi"] = advance + 15

    font = FontBuilder(1000, isTTF=True)
    font.setupGlyphOrder(list(outlines))
    font.setupCharacterMap(cmap)
    glyphs, metrics = {}, {}
    for name, polygons in outlines.items():
        pen = TTGlyphPen(None)
        for first, *rest in polygons:
            pen.moveTo(first)
            for point in rest:
                pen.lineTo(point)
            pen.closePath()
        glyphs[name] = pen.glyph()
        metrics[name] = (advances[name], min((x for p in polygons for x, _ in p), default=0))
    font.setupGlyf(glyphs)
    font.setupHorizontalMetrics(metrics)
    font.setupHorizontalHeader(ascent=850, descent=-250)
    font.setupNameTable(dict(familyName="N4 Grid Sans", styleName="Regular",
                             uniqueFontIdentifier="N4GridSans-Regular-1", fullName="N4 Grid Sans",
                             psName="N4GridSans-Regular", version="Version 1.0",
                             licenseDescription="CC0 1.0 Universal",
                             licenseInfoURL="https://creativecommons.org/publicdomain/zero/1.0/"))
    font.setupOS2(sTypoAscender=850, sTypoDescender=-250, usWinAscent=850, usWinDescent=250)
    font.setupPost()
    font.setupMaxp()
    addOpenTypeFeaturesFromString(font.font, """
        languagesystem DFLT dflt;
        languagesystem latn dflt;
        feature liga { sub uni0066 uni0066 uni0069 by ffi; } liga;
    """)
    font.font["head"].created = font.font["head"].modified = 3700000000
    font.font.recalcTimestamp = False
    font.save(Path(__file__).with_name("N4GridSans.ttf"))

if __name__ == "__main__":
    main()
