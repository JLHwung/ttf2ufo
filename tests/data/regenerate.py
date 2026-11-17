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


if __name__ == "__main__":
    main()
