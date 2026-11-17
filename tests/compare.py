"""Compare ttf2ufo's output with extractufo's, caching extractufo's.

Run with a Python that has the reference packages (ufo-extractor 0.8.1,
fontFeatures 1.9.0, fontTools 4.61.1, ufoLib2 0.18.1):

    python tests/compare.py [options] FONT... [--list FILE]

For every font it prints SAME, DIFF (with the files that differ), REFFAIL
(only extractufo failed), RSFAIL (only ttf2ufo failed) or BOTHFAIL, then a
summary.

extractufo is slow, so its output is reduced to a sha256 per file and
cached in ~/.cache/ttf2ufo/extractufo/ (or $TTF2UFO_CACHE), keyed by the
font's content and by the reference: package versions, extractor's and
fontFeatures' patched sources and the patch set. A font is extracted once
per reference; a changed reference starts a new cache.

By default extractufo is patched with upstream fixes:
robotools/extractor#80 (mark anchors of any format), #78 (skip undecodable
name records) and simoncozens/fontFeatures#75 (sets of glyph names in glyph
order). ttf2ufo on this branch does not make them; it matches unpatched
extractufo, so use --plain, which leaves them out. Fonts where fontFeatures
iterates a set of glyph names still differ (see the README). Either way,
extractor's versionMinor (robotools/extractor#81) is fixed, and
PYTHONHASHSEED is 0.
"""

import argparse
import concurrent.futures
import hashlib
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent

# (module file, original text, replacement): source patches applied to the
# reference before it runs. FIXES are what ttf2ufo changes on purpose.
ALWAYS = [
    (
        "extractor/formats/opentype.py",
        "    version = str(round(head.fontRevision, 3))\n",
        '    version = f"{round(head.fontRevision, 6):.3f}"\n',
    ),
]
FIXES = [
    (  # robotools/extractor#80
        "extractor/formats/opentype.py",
        "        if (markAnchor.Format != 1):\n",
        "        if False:\n",
    ),
    (  # robotools/extractor#78, as on extractor's master
        "extractor/formats/opentype.py",
        "        string = record.toUnicode()\n",
        "        try:\n"
        "            string = record.toUnicode()\n"
        "        except UnicodeDecodeError:\n"
        "            continue\n",
    ),
    (  # simoncozens/fontFeatures#75
        "fontFeatures/feaLib/Attachment.py",
        "        allglyphs = set(self.bases.keys()) | set(self.marks.keys())\n",
        "        allglyphs = list(dict.fromkeys([*self.bases, *self.marks]))\n",
    ),
    (  # simoncozens/fontFeatures#75
        "fontFeatures/ttLib/GTableUnparser.py",
        "        classes[0] = glyphset - set(a.keys())\n",
        "        classes[0] = [g for g in font.getGlyphOrder() if g not in a]\n",
    ),
]
MODULES = {
    "extractor/formats/opentype.py": "extractor.formats.opentype",
    "fontFeatures/feaLib/Attachment.py": "fontFeatures.feaLib.Attachment",
    "fontFeatures/ttLib/GTableUnparser.py": "fontFeatures.ttLib.GTableUnparser",
}


def patched_sources(plain):
    """Module name -> (file, patched source)."""
    import importlib

    patches = ALWAYS + ([] if plain else FIXES)
    out = {}
    for rel, name in MODULES.items():
        module = importlib.import_module(name)
        src = Path(module.__file__).read_text()
        for file, old, new in patches:
            if file != rel:
                continue
            if new in src and old not in src:
                continue  # already applied, e.g. a locally patched install
            if src.count(old) != 1:
                raise SystemExit(f"{rel}: patch target not found: {old.strip()!r}")
            src = src.replace(old, new)
        out[name] = (module.__file__, src)
    return out


def fingerprint(plain):
    from importlib.metadata import version

    h = hashlib.sha256()
    for pkg in ["ufo-extractor", "fontFeatures", "fonttools", "ufoLib2", "lxml"]:
        h.update(f"{pkg}={version(pkg)}\n".encode())
    for name, (_, src) in sorted(patched_sources(plain).items()):
        h.update(f"{name}\n{src}\n".encode())
    return h.hexdigest()[:16]


def manifest(ufo):
    """Relative path -> sha256 of every file in a UFO package."""
    return {
        str(p.relative_to(ufo)): hashlib.sha256(p.read_bytes()).hexdigest()
        for p in sorted(Path(ufo).rglob("*"))
        if p.is_file()
    }


def reference_extract(font, out, plain):
    """Run extractufo, patched, on font into the UFO package out."""
    import extractor
    import fontFeatures
    import ufoLib2

    for name, (file, src) in patched_sources(plain).items():
        module = sys.modules[name]
        namespace = dict(module.__dict__)
        exec(compile(src, file, "exec"), namespace)
        for key, new in namespace.items():
            old = module.__dict__.get(key)
            if isinstance(old, type) and isinstance(new, type) and old is not new:
                # keep the class (subclasses elsewhere derive from it), take
                # its new methods
                for attr, value in vars(new).items():
                    if callable(value) and not attr.startswith("__"):
                        setattr(old, attr, value)
            else:
                module.__dict__[key] = new
    # rebind functions imported by name before patching
    from extractor.formats import opentype
    from fontFeatures.feaLib import Attachment

    extractor._extractFunctions["OTF"] = opentype.extractFontFromOpenType
    fontFeatures.Attachment.asFeaAST = Attachment.asFeaAST
    ufo = ufoLib2.Font()
    extractor.extractUFO(font, ufo)
    ufo.save(out, structure="package")


def run_reference(font, plain, timeout, save=None):
    """{"files": manifest} or {"error": message}, from a fresh process."""
    with tempfile.TemporaryDirectory() as tmp:
        out = save or os.path.join(tmp, "ref.ufo")
        cmd = [sys.executable, "-I", __file__, "--reference-worker", font, out]
        if plain:
            cmd.append("--plain")
        env = dict(os.environ, PYTHONHASHSEED="0")
        try:
            r = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            return {"error": f"timed out after {timeout}s"}
        if r.returncode:
            # extractor prints the traceback of the cause, then raises its own
            # ExtractorError ("There was an error reading the OTF file")
            lines = [l for l in (r.stdout + r.stderr).splitlines() if l.strip()]
            errors = [
                l for l in lines
                if not l.startswith(" ") and ("Error" in l or "Exception" in l) and ":" in l
            ]
            return {"error": (errors or lines or ["failed"])[0].strip()}
        return {"files": manifest(out)}


def run_ttf2ufo(binary, font, save=None):
    with tempfile.TemporaryDirectory() as tmp:
        out = save or os.path.join(tmp, "rs.ufo")
        r = subprocess.run([binary, font, out], capture_output=True, text=True)
        if r.returncode:
            return {"error": (r.stderr.strip().splitlines() or ["failed"])[-1]}
        return {"files": manifest(out)}


def compare(ref, rs):
    if "error" in ref and "error" in rs:
        return "BOTHFAIL", f"extractufo: {ref['error']} | ttf2ufo: {rs['error']}"
    if "error" in ref:
        return "REFFAIL", ref["error"]
    if "error" in rs:
        return "RSFAIL", rs["error"]
    a, b = ref["files"], rs["files"]
    differ = sorted(p for p in set(a) | set(b) if a.get(p) != b.get(p))
    return ("SAME", "") if not differ else ("DIFF", " ".join(differ))


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("fonts", nargs="*")
    parser.add_argument("--list", help="file with one font path per line")
    parser.add_argument("--ttf2ufo", default=str(HERE.parent / "target/release/ttf2ufo"))
    parser.add_argument("--plain", action="store_true", help="unfixed extractufo")
    parser.add_argument("-j", "--jobs", type=int, default=os.cpu_count())
    parser.add_argument("--timeout", type=int, default=1800, help="seconds per extractufo run")
    parser.add_argument("--keep", help="save both UFOs of differing fonts here")
    parser.add_argument("--reference-worker", nargs=2, help=argparse.SUPPRESS)
    args = parser.parse_args()

    if args.reference_worker:
        reference_extract(*args.reference_worker, args.plain)
        return

    fonts = list(args.fonts)
    if args.list:
        fonts += [l for l in Path(args.list).read_text().splitlines() if l.strip()]
    if not fonts:
        parser.error("no fonts given")
    if not os.path.exists(args.ttf2ufo):
        parser.error(f"{args.ttf2ufo} not found; run cargo build --release")

    cache_root = Path(os.environ.get("TTF2UFO_CACHE", Path.home() / ".cache/ttf2ufo"))
    cache = cache_root / "extractufo" / fingerprint(args.plain)
    cache.mkdir(parents=True, exist_ok=True)

    def one(font):
        key = hashlib.sha256(Path(font).read_bytes()).hexdigest()
        entry = cache / f"{key}.json"
        if entry.exists():
            ref = json.loads(entry.read_text())
        else:
            ref = run_reference(font, args.plain, args.timeout)
            if not ref.get("error", "").startswith("timed out"):
                entry.write_text(json.dumps(ref, indent=0, sort_keys=True))
        status, detail = compare(ref, run_ttf2ufo(args.ttf2ufo, font))
        if status == "DIFF" and args.keep:
            dest = Path(args.keep) / key[:12]
            dest.mkdir(parents=True, exist_ok=True)
            (dest / "font.txt").write_text(font + "\n")
            for name in ["ref.ufo", "rs.ufo"]:
                subprocess.run(["rm", "-rf", str(dest / name)])
            run_reference(font, args.plain, args.timeout, save=str(dest / "ref.ufo"))
            run_ttf2ufo(args.ttf2ufo, font, save=str(dest / "rs.ufo"))
            detail += f" (kept in {dest})"
        return font, status, detail

    counts = {}
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        for font, status, detail in pool.map(one, fonts):
            counts[status] = counts.get(status, 0) + 1
            print(f"{status} {font}" + (f": {detail}" if detail else ""), flush=True)
    print(", ".join(f"{n} {s}" for s, n in sorted(counts.items())), f"(cache: {cache})")


if __name__ == "__main__":
    main()
