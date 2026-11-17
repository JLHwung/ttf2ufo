#!/usr/bin/env python3
"""Regenerate the expected test outputs from the Python reference implementation.

    python3 tests/data/regenerate.py

Requires ufo-extractor 0.8.1 (with fontFeatures) and ufoLib2, which is what the
expected files were produced with. It writes:

- tests/data/expected/<font>.ufo for every tests/data/fonts/*.ttf, exactly as
  `extractufo` does (extractor.extractUFO into a ufoLib2 Font, saved as a
  package);
- tests/data/stream/*.bin: bytecode for the assembly samples of extractor's
  tests/stream_test.py, compiled with fontTools.
"""
import shutil
import sys
from pathlib import Path

import extractor
import ufoLib2
from fontTools.ttLib.tables.ttProgram import Program

HERE = Path(__file__).parent

# Samples from extractor/tests/stream_test.py
STREAM_SAMPLES = {
    "sample": """PUSHB[ ]	/* 4 values pushed */
1 2 3 4
PUSHW[ ]	/* 2 values pushed */
1 -1
PUSHW[ ]	/* 2 values pushed */
5 512
SPVTCA[0]	/* SetPVectorToAxis */
SPVTCA[1]	/* SetPVectorToAxis */
SFVTL[0]	/* SetFVectorToLine */
SFVTL[1]	/* SetFVectorToLine */
MDAP[0]	/* MoveDirectAbsPt */
MDAP[1]	/* MoveDirectAbsPt */
IF[ ]	/* If */
  SHP[0]	/* ShiftPointByLastPoint */
  SHC[1]	/* ShiftContourByLastPt */
EIF[ ]	/* EndIf */
MSIRP[1]	/* MoveStackIndirRelPt */
MD[0]	/* MeasureDistance */
GC[1]	/* GetCoordOnPVector */
ROUND[01]	/* Round */
MDRP[00110]	/* MoveDirectRelPt */""",
    "sample_keyerror": "MDRP[00111]	/* MoveDirectRelPt */",
}

# Glyph names for tests/data/filename_cases.json: case clashes, reserved
# names, illegal characters and names long enough to be truncated.
FILENAME_CASES = [
    "A", "a_", "A", "con", "a.con.b", ".notdef", "x" * 260, "X" * 260,
    "x" * 248 + ".con", "x" * 240, "x" * 240, "nul.Prn", "a\x00b", "a\x01b",
    "\u00c9", "\u00e9_",
]


def write_filename_cases():
    """File names userNameToFileName gives FILENAME_CASES in sequence."""
    import json

    from fontTools.misc.filenames import userNameToFileName

    existing, cases = set(), []
    for name in FILENAME_CASES:
        file_name = userNameToFileName(name, existing, suffix=".glif")
        existing.add(file_name.lower())
        cases.append((name, file_name))
    (HERE / "filename_cases.json").write_text(json.dumps(cases) + "\n")


def write_agl():
    """src/agl.rs: fontTools.agl.UV2AGL as a sorted Rust table."""
    import fontTools
    from fontTools import agl

    lines = [
        "//! fontTools.agl.UV2AGL (fontTools %s): code point -> Adobe Glyph List" % fontTools.version,
        "//! name, used for glyph names derived from the cmap. Generated; see",
        "//! tests/data/regenerate.py.",
        "",
        "/// (code point, glyph name), sorted by code point.",
        "pub static UV2AGL: &[(u32, &str)] = &[",
    ]
    lines += ['    (0x%04X, "%s"),' % item for item in sorted(agl.UV2AGL.items())]
    lines += [
        "];",
        "",
        "/// `agl.UV2AGL.get(uv)`",
        "pub fn uv2agl(uv: u32) -> Option<&'static str> {",
        "    UV2AGL",
        "        .binary_search_by_key(&uv, |&(u, _)| u)",
        "        .ok()",
        "        .map(|i| UV2AGL[i].1)",
        "}",
    ]
    (HERE.parent.parent / "src" / "agl.rs").write_text("\n".join(lines) + "\n")


# Python codecs fontTools uses for legacy name records (encodingTools), other
# than mac_roman, latin1, ascii and UTF-16.
SINGLE_BYTE_CODECS = [
    "mac_iceland", "mac_turkish", "mac_croatian", "mac_latin2", "mac_romanian",
    "mac_greek", "mac_cyrillic",
]
DOUBLE_BYTE_CODECS = ["shift_jis", "gb2312", "big5", "euc_kr", "johab"]


def write_codecs():
    """src/codecs/tables.rs and src/codecs/*.bin: Python's decoding tables
    for SINGLE_BYTE_CODECS and DOUBLE_BYTE_CODECS, and the byte mappings of
    fontTools' x_mac_*_ttx codecs.

    A <codec>.bin file lists the byte pairs that decode to one character, as
    runs: u16 first pair, u16 count, then count u16 characters (big-endian).
    """
    import struct

    import fontTools
    from fontTools.encodings.codecs import _extended_encodings

    out = HERE.parent.parent / "src" / "codecs"
    out.mkdir(exist_ok=True)

    def single(codec):
        table = []
        for b in range(256):
            try:
                s = bytes([b]).decode(codec)
            except UnicodeDecodeError:
                table.append(0xFFFF)
                continue
            assert len(s) == 1 and ord(s) < 0xFFFF
            table.append(ord(s))
        return table

    lines = [
        "//! Decoding tables of the Python codecs fontTools %s uses for legacy" % fontTools.version,
        "//! name records. Generated; see tests/data/regenerate.py.",
        "",
        "/// Byte -> character for each byte that decodes on its own (0xFFFF: none).",
        "pub static SINGLE: &[(&str, [u16; 256])] = &[",
    ]
    for codec in SINGLE_BYTE_CODECS + DOUBLE_BYTE_CODECS:
        values = ", ".join("0x%04X" % v for v in single(codec))
        lines.append('    ("%s", [%s]),' % (codec, values))
    lines += ["];", "", "/// Byte pairs that decode to one character (see `pair`)."]
    lines.append("pub static PAIRS: &[(&str, &[u8])] = &[")
    for codec in DOUBLE_BYTE_CODECS:
        lone = single(codec)
        pairs = []
        for lead in range(256):
            if lone[lead] != 0xFFFF:
                continue
            for trail in range(256):
                try:
                    s = bytes([lead, trail]).decode(codec)
                except UnicodeDecodeError:
                    continue
                assert len(s) == 1 and ord(s) <= 0xFFFF
                pairs.append((lead << 8 | trail, ord(s)))
        data = bytearray()
        i = 0
        while i < len(pairs):
            j = i
            while j + 1 < len(pairs) and pairs[j + 1][0] == pairs[j][0] + 1:
                j += 1
            data += struct.pack(">HH", pairs[i][0], j - i + 1)
            data += b"".join(struct.pack(">H", c) for _, c in pairs[i : j + 1])
            i = j + 1
        (out / f"{codec}.bin").write_bytes(bytes(data))
        lines.append('    ("%s", include_bytes!("%s.bin")),' % (codec, codec))
    lines += [
        "];",
        "",
        "/// A fontTools x_mac_*_ttx codec: (name, base codec, extra bytes), the",
        "/// extra bytes used where the base codec fails.",
        "pub type Extended = (&'static str, &'static str, &'static [(u8, char)]);",
        "",
        "pub static EXTENDED: &[Extended] = &[",
    ]
    for name, (base, mapping) in _extended_encodings.items():
        items = []
        for key, value in sorted(mapping.items()):
            assert len(key) == 1 and len(value) == 1
            items.append("(0x%02X, '\\u{%X}')" % (key[0], ord(value)))
        lines.append('    ("%s", "%s", &[%s]),' % (name, base, ", ".join(items)))
    lines.append("];")
    (out / "tables.rs").write_text("\n".join(lines) + "\n")


def write_codec_cases():
    """tests/data/codec_cases.json: random byte strings and how Python
    decodes them (null for a UnicodeDecodeError) with every codec of
    write_codecs, for the unit test in src/codecs/mod.rs."""
    import json
    import random

    import fontTools.encodings.codecs  # noqa: F401 (registers x_mac_*_ttx)
    from fontTools.encodings.codecs import _extended_encodings

    rng = random.Random(1)
    cases = []
    for codec in SINGLE_BYTE_CODECS + DOUBLE_BYTE_CODECS + list(_extended_encodings):
        valid = []
        for lead in range(256):
            for trail in [None] + list(range(256)):
                b = bytes([lead]) if trail is None else bytes([lead, trail])
                try:
                    if len(b.decode(codec)) == 1:
                        valid.append(b)
                except UnicodeDecodeError:
                    pass
        for _ in range(100):
            parts = [rng.choice(valid) for _ in range(rng.randrange(1, 8))]
            if rng.random() < 0.3:  # sometimes a random byte, often invalid
                parts.insert(rng.randrange(len(parts) + 1), bytes([rng.randrange(256)]))
            b = b"".join(parts)
            try:
                text = b.decode(codec)
            except UnicodeDecodeError:
                text = None
            cases.append([codec, b.hex(), text])
    (HERE / "codec_cases.json").write_text(json.dumps(cases) + "\n")


def main():
    print("extractor", extractor.__version__, file=sys.stderr)
    expected = HERE / "expected"
    expected.mkdir(exist_ok=True)
    for font in sorted((HERE / "fonts").glob("*.ttf")):
        out = expected / f"{font.stem}.ufo"
        if out.exists():
            shutil.rmtree(out)
        ufo = ufoLib2.Font()
        extractor.extractUFO(str(font), ufo)
        ufo.save(out, structure="package")
        print("wrote", out, file=sys.stderr)

    for name, asm in STREAM_SAMPLES.items():
        program = Program()
        program.fromAssembly(asm)
        (HERE / "stream" / f"{name}.bin").write_bytes(program.getBytecode())
        (HERE / "stream" / f"{name}.ttxasm").write_text(asm)

    write_filename_cases()
    write_agl()
    write_codecs()
    write_codec_cases()


if __name__ == "__main__":
    main()
