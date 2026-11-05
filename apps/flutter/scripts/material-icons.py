"""Render the shared Web Material Symbols Rounded font as small Flutter fonts.

Usage: python3 scripts/material-icons.py <material-symbols@0.44.10 rounded.woff2>
Requires fonttools[woff]. The semantic mapping is checked in beside the fonts.
"""
from pathlib import Path
import json
import sys
from fontTools import subset
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

root = Path(__file__).resolve().parent.parent
mapping = json.loads((root / 'assets/fonts/material-symbols-map.json').read_text())
source = Path(sys.argv[1])
font = TTFont(source)
char_of = {glyph: chr(code) for code, glyph in font.getBestCmap().items()}
by_name = {}
for lookup in font['GSUB'].table.LookupList.Lookup:
    for table in lookup.SubTable:
        if lookup.LookupType == 7:
            table = table.ExtSubTable
        if hasattr(table, 'ligatures'):
            for first, ligatures in table.ligatures.items():
                for ligature in ligatures:
                    name = ''.join(char_of.get(g, '\0') for g in [first, *ligature.Component])
                    by_name[name] = ligature.LigGlyph
cmap = {entry['codepoint']: by_name[entry['web']] for entry in mapping.values()}
for fill in (0, 1):
    font = instantiateVariableFont(TTFont(source), {'FILL': fill, 'wght': 500, 'GRAD': 0, 'opsz': 24}, inplace=True)
    for table in font['cmap'].tables:
        if table.isUnicode():
            table.cmap = cmap.copy()
    options = subset.Options()
    options.layout_features = []
    cutter = subset.Subsetter(options=options)
    cutter.populate(unicodes=cmap.keys())
    cutter.subset(font)
    family = 'PPVPN Material Filled' if fill else 'PPVPN Material'
    for name_id, value in [(1, family), (2, 'Regular'), (4, family), (6, family.replace(' ', ''))]:
        font['name'].setName(value, name_id, 3, 1, 0x409)
    font.flavor = None
    font.save(root / 'assets/fonts' / ('MaterialSymbolsFilled.ttf' if fill else 'MaterialSymbols.ttf'))
