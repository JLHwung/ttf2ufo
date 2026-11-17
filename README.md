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
`--update` an existing UFO is updated in place: only files whose content
changed are rewritten, and files the font no longer produces (stale `.glif`
files, a dropped `kerning.plist`) are removed. Unchanged files keep their
mtime, so git does not re-hash them.

Either way the UFO is checked before anything is written, so a font that
extractufo would fail to save leaves the output untouched. `.glif` files are
rendered one at a time while they are written or compared, so the whole
package is never in memory at once.

Set `TTF2UFO_TIMING=1` to print how long each stage takes.

## Install

```sh
cargo install ttf2ufo
```

## Library

The crate is also a library. `ttf2ufo::extract` turns font data into an
in-memory `Ufo`, which `save` writes as a new package and `sync` uses to
update an existing one:

```rust
let ufo = ttf2ufo::extract(&std::fs::read("Font.ttf")?)?;
ufo.save(std::path::Path::new("Font.ttf.ufo"))?;
```

See the API documentation (`cargo doc --open`) for the rest.

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
- a cmap format 2 subtable on a Unicode platform;
- an EUC-KR name record using a KS X 1001 Annex 3 make-up sequence.

Where extractufo itself fails, ttf2ufo fails too rather than writing a UFO:
for example point-matched components, a non-ASCII OS/2 vendor ID, cmap
formats 8 and 10 on a Unicode platform, strings that are not valid in XML,
cyclic components, and unnamed lookups in a feaLib `Debg` table.

extractufo's own output is not reproducible in two places, because
fontFeatures iterates Python sets of glyph names (their order depends on
`PYTHONHASHSEED`): the order of `pos cursive` rules, and the glyph order of a
class-0 glyph class with two to five glyphs in a contextual format 2 rule.
ttf2ufo writes these in glyph order.

## Tests

`cargo test` runs:

- `tests/extractor_test.rs` and `tests/stream_test.rs`: ports of
  ufo-extractor's own tests (`extractor_test.py`, `stream_test.py`);
- `tests/golden_test.rs`: every file of the extracted UFO compared byte for
  byte with extractufo's output for the same font;
- `tests/features_test.rs`: `features.fea` alone, reporting the first line
  that differs;
- unit tests for the Python behaviors that leak into the output (float
  `repr`, CPython set iteration order, tuple hashing, legacy name-record
  codecs, glyph file names), checked against values generated with
  CPython 3.14 and fontTools.

`tests/compare.py` compares ttf2ufo with extractufo on any fonts. Pass
`--plain`: ttf2ufo matches unpatched extractufo, apart from the set orders
above. Without it, extractufo is patched with upstream fixes that ttf2ufo
does not make (robotools/extractor#78 and #80, simoncozens/fontFeatures#75),
so fonts those fixes touch differ. extractufo is slow, so the hashes of its
output are cached in `~/.cache/ttf2ufo`, per font and per version of the
reference packages:

```sh
cargo build --release
/path/to/reference/python tests/compare.py --plain --list fonts.txt
```

`tests/data/regenerate.py` rebuilds the expected outputs, the generated
tables in `src/agl.rs` and `src/codecs/`, and the codec and glyph file name
unit-test cases from the Python reference implementation; see
`tests/data/README.md` for where the test data comes from.

## License

MIT, see [LICENSE](LICENSE). The test data in `tests/data` comes from
third parties under their own licenses; see `tests/data/README.md`.
