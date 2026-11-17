# ttf2ufo

Extract a UFO from a TrueType font, byte-for-byte as
[ufo-extractor](https://github.com/robotools/extractor)'s `extractufo` 0.8.1
does (fontTools 4.61, ufoLib2, fontFeatures 1.9).

```sh
ttf2ufo Font.ttf                 # writes Font.ttf.ufo, like extractufo
ttf2ufo Font.ttf Out.ufo
ttf2ufo --update Font.ttf Out.ufo
```

Without `--update` the output must not exist (extractufo refuses too). With
`--update` an existing UFO is updated in place: every file is generated in
memory first, then only files whose content changed are rewritten and files
the font no longer produces (stale `.glif` files, a dropped `kerning.plist`)
are removed. Unchanged files keep their mtime, so git does not re-hash them.

Set `TTF2UFO_TIMING=1` to print how long each stage takes.

## What is extracted

Everything `extractufo` writes for a TrueType font:

- glyphs: outlines (including fontTools' x shift when a glyph's stored `xMin`
  disagrees with its `hmtx` left side bearing), components with their
  `public.objectLibs` flags and `public.truetype.overlap`, unicodes (in
  CPython's set order, as extractor lists them), vertical metrics, GPOS mark
  anchors, and per-glyph TrueType instructions with fontTools' outline hash;
- `fontinfo.plist` (head, name, OS/2, hhea, vhea, post, gasp), with ufoLib's
  validators applied the way extractor's `RelaxedInfo` does;
- `lib.plist`: glyph order, Unicode variation sequences, the font-wide
  TrueType programs (`fpgm`, `prep`, `cvt `, `maxp` limits);
- `kerning.plist` and `groups.plist` from GPOS pair positioning (or `kern`);
- `features.fea`, a port of fontFeatures' `unparse(font).asFea()`.

## Not supported

These stop with an error instead of producing output that could differ from
extractufo's:

- CFF-flavored (`.otf`) fonts;
- fonts without glyph names in `post` (format 3), whose names fontTools
  derives from the cmap and the Adobe Glyph List;
- name records in legacy CJK, Greek, Cyrillic or other non-Roman Mac/Windows
  encodings, and a non-ASCII OS/2 vendor ID;
- point-matched (anchor-positioned) components;
- cmap subtable formats 2, 8 and 10 on a Unicode platform.

## Tests

`cargo test` runs:

- `tests/extractor_test.rs` and `tests/stream_test.rs`: ports of
  ufo-extractor's own tests (`extractor_test.py`, `stream_test.py`);
- `tests/golden_test.rs`: every file of the extracted UFO compared byte for
  byte with extractufo's output for the same font;
- unit tests for the Python behaviors that leak into the output (float
  `repr`, CPython set iteration order, tuple hashing), checked against values
  generated with CPython 3.14.

`tests/data/regenerate.py` rebuilds the expected outputs from the Python
reference implementation; see `tests/data/README.md` for where the test data
comes from.

## License

MIT, see [LICENSE](LICENSE). The test data in `tests/data` comes from
third parties under their own licenses; see `tests/data/README.md`.
