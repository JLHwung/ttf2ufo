//! An in-memory UFO and its serialization, matching what ufoLib2's
//! `Font.save(path, structure="package")` writes.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;

use crate::Error;
use crate::plist::{self, Dict, Value, escape_attr};
use crate::pyfmt::Num;

/// A contour point in font units.
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    /// Horizontal position.
    pub x: i64,
    /// Vertical position.
    pub y: i64,
    /// Segment type; `None` for off-curve points.
    pub typ: Option<&'static str>,
}

/// A reference to another glyph, placed with an affine transformation.
#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    /// Name of the referenced glyph.
    pub base: String,
    /// (xScale, xyScale, yxScale, yScale, xOffset, yOffset)
    pub transform: [Num; 6],
    /// The component's identifier, the key of its entry in the glyph lib's
    /// `public.objectLibs`.
    pub identifier: Option<String>,
}

/// A named anchor point, taken from GPOS mark attachment.
#[derive(Clone, Debug, PartialEq)]
pub struct Anchor {
    /// Horizontal position.
    pub x: i64,
    /// Vertical position.
    pub y: i64,
    /// Anchor name: `Anchor-N` on base glyphs and `_Anchor-N` on marks, where
    /// N numbers the anchor groups, as extractor names them.
    pub name: String,
}

/// A glyph of the default layer, written as one `.glif` file.
#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    /// Glyph name.
    pub name: String,
    /// Advance width.
    pub width: i64,
    /// Advance height, from `vmtx`; 0 when the font has none.
    pub height: i64,
    /// Code points mapped to this glyph, in the order extractor lists them.
    pub unicodes: Vec<u32>,
    /// Anchors, in the order they are written.
    pub anchors: Vec<Anchor>,
    /// Closed contours; each is a list of points.
    pub contours: Vec<Vec<Point>>,
    /// Components, written after the contours.
    pub components: Vec<Component>,
    /// The glyph's `lib`, such as `public.objectLibs`, `public.truetype.overlap`
    /// and `public.truetype.instructions`.
    pub lib: Dict,
}

impl Glyph {
    /// An empty glyph with the given name and advance width.
    pub fn new(name: String, width: i64) -> Self {
        Glyph {
            name,
            width,
            height: 0,
            unicodes: Vec::new(),
            anchors: Vec::new(),
            contours: Vec::new(),
            components: Vec::new(),
            lib: Dict::new(),
        }
    }

    /// The .glif file, as fontTools' glifLib writes format 2.
    pub fn to_glif(&self) -> String {
        let mut s = String::with_capacity(4096);
        s.push_str("<?xml version='1.0' encoding='UTF-8'?>\n");
        writeln!(
            s,
            "<glyph name=\"{}\" format=\"2\">",
            escape_attr(&self.name)
        )
        .unwrap();
        match (self.width, self.height) {
            (0, 0) => {}
            (w, 0) => writeln!(s, "  <advance width=\"{w}\"/>").unwrap(),
            (0, h) => writeln!(s, "  <advance height=\"{h}\"/>").unwrap(),
            (w, h) => writeln!(s, "  <advance height=\"{h}\" width=\"{w}\"/>").unwrap(),
        }
        let mut seen = HashSet::new();
        for u in &self.unicodes {
            if seen.insert(*u) {
                writeln!(s, "  <unicode hex=\"{u:04X}\"/>").unwrap();
            }
        }
        for a in &self.anchors {
            writeln!(
                s,
                "  <anchor x=\"{}\" y=\"{}\" name=\"{}\"/>",
                a.x,
                a.y,
                escape_attr(&a.name)
            )
            .unwrap();
        }
        s.push_str("  <outline>\n");
        for contour in &self.contours {
            s.push_str("    <contour>\n");
            // hot path: written by hand instead of through `write!`
            let mut num = itoa::Buffer::new();
            for p in contour {
                s.push_str("      <point x=\"");
                s.push_str(num.format(p.x));
                s.push_str("\" y=\"");
                s.push_str(num.format(p.y));
                match p.typ {
                    Some(t) => {
                        s.push_str("\" type=\"");
                        s.push_str(t);
                        s.push_str("\"/>\n");
                    }
                    None => s.push_str("\"/>\n"),
                }
            }
            s.push_str("    </contour>\n");
        }
        const NAMES: [&str; 6] = [
            "xScale", "xyScale", "yxScale", "yScale", "xOffset", "yOffset",
        ];
        const DEFAULTS: [f64; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        for c in &self.components {
            write!(s, "    <component base=\"{}\"", escape_attr(&c.base)).unwrap();
            for i in 0..6 {
                if c.transform[i].as_f64() != DEFAULTS[i] {
                    write!(s, " {}=\"{}\"", NAMES[i], c.transform[i].repr()).unwrap();
                }
            }
            if let Some(id) = &c.identifier {
                write!(s, " identifier=\"{}\"", escape_attr(id)).unwrap();
            }
            s.push_str("/>\n");
        }
        s.push_str("  </outline>\n");
        if !self.lib.is_empty() {
            s.push_str("  <lib>\n");
            plist::write_dict(&mut s, &self.lib, 2);
            s.push_str("  </lib>\n");
        }
        s.push_str("</glyph>\n");
        s
    }
}

/// fontTools.misc.filenames.userNameToFileName with suffix ".glif".
pub fn glyph_file_name(name: &str, existing_lower: &mut HashSet<String>) -> String {
    // fontTools' list has r"\0" (backslash, zero), so NUL is not replaced
    const ILLEGAL: &[char] = &['"', '*', '+', '/', ':', '<', '>', '?', '[', '\\', ']', '|'];
    const RESERVED: &[&str] = &[
        "con", "prn", "aux", "clock$", "nul", "a:-z:", "com1", "lpt1", "lpt2", "lpt3", "com2",
        "com3", "com4",
    ];
    const SUFFIX: &str = ".glif";
    let mut user = name.to_string();
    if user.starts_with('.') {
        user = format!("_{}", &user[1..]);
    }
    let mut filtered = String::new();
    for c in user.chars() {
        if ILLEGAL.contains(&c) || (1..32).contains(&(c as u32)) || c as u32 == 0x7F {
            filtered.push('_');
        } else {
            filtered.push(c);
            if c.to_lowercase().collect::<String>() != c.to_string() {
                filtered.push('_');
            }
        }
    }
    // truncated before the reserved-name prefixes are added
    let filtered: String = filtered.chars().take(255 - SUFFIX.len()).collect();
    let mut base: String = filtered
        .split('.')
        .map(|part| {
            if RESERVED.contains(&part.to_lowercase().as_str()) {
                format!("_{part}")
            } else {
                part.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(".");
    let mut file = format!("{base}{SUFFIX}");
    if existing_lower.contains(&file.to_lowercase()) {
        // handleClash1: room for a 15-digit counter between name and suffix
        let len = base.chars().count();
        if len + SUFFIX.len() + 15 > 255 {
            base = base.chars().take(255 - SUFFIX.len() - 15).collect();
        }
        let mut n = 1u64;
        loop {
            let cand = format!("{base}{n:015}{SUFFIX}");
            if !existing_lower.contains(&cand.to_lowercase()) {
                file = cand;
                break;
            }
            n += 1;
        }
    }
    existing_lower.insert(file.to_lowercase());
    file
}

/// glifLib's `GLIFPointPen` validation that TrueType outlines can fail: a
/// curve point after more than two off-curve points (a cubic segment with
/// several off-curve pairs). The count restarts with each contour.
fn check_outline(glyph: &Glyph) -> Result<(), Error> {
    for contour in &glyph.contours {
        let mut off_curves = 0;
        for p in contour {
            match p.typ {
                None => off_curves += 1,
                Some(t) => {
                    if t == "curve" && off_curves > 2 {
                        return Err("too many offcurve points before curve point.".into());
                    }
                    off_curves = 0;
                }
            }
        }
    }
    Ok(())
}

/// A whole UFO in memory.
#[derive(Clone, Debug, Default)]
pub struct Ufo {
    /// `fontinfo.plist`.
    pub info: Dict,
    /// `lib.plist`; not written when empty.
    pub lib: Dict,
    /// `groups.plist`; not written when empty.
    pub groups: Dict,
    /// `kerning.plist`; not written when empty.
    pub kerning: Dict,
    /// `features.fea`; not written when empty.
    pub features: String,
    /// In glyph order.
    pub glyphs: Vec<Glyph>,
}

impl Ufo {
    /// The glyph with the given name, if any.
    pub fn glyph(&self, name: &str) -> Option<&Glyph> {
        self.glyphs.iter().find(|g| g.name == name)
    }

    /// Fail where ufoLib2's save fails: a string that goes into a plist or
    /// .glif is not XML compatible (see [`plist::check_xml_str`]; features.fea
    /// is plain text and is not checked), or glifLib's point pen rejects an
    /// outline.
    pub fn validate(&self) -> Result<(), Error> {
        for (file, dict) in [
            ("fontinfo.plist", &self.info),
            ("groups.plist", &self.groups),
            ("kerning.plist", &self.kerning),
            ("lib.plist", &self.lib),
        ] {
            plist::check_xml_dict(dict)
                .map_err(|e| format!("'{file}' could not be written: {e}"))?;
        }
        self.glyphs
            .par_iter()
            .try_for_each(|g| -> Result<(), Error> {
                let strings = std::iter::once(&g.name)
                    .chain(g.anchors.iter().map(|a| &a.name))
                    .chain(g.components.iter().map(|c| &c.base))
                    .chain(g.components.iter().filter_map(|c| c.identifier.as_ref()));
                strings
                    .map(|s| plist::check_xml_str(s))
                    .chain(std::iter::once(plist::check_xml_dict(&g.lib)))
                    .chain(std::iter::once(check_outline(g)))
                    .collect::<Result<(), Error>>()
                    .map_err(|e| format!("glyph {:?} could not be written: {e}", g.name).into())
            })
    }

    /// Every file of the UFO package: relative path -> content. Fails where
    /// ufoLib2's save fails (see [`Ufo::validate`]).
    ///
    /// This holds the whole package in memory; [`Ufo::save`] and [`Ufo::sync`]
    /// render each glyph only while writing it.
    pub fn to_files(&self) -> Result<BTreeMap<PathBuf, Vec<u8>>, Error> {
        let package = self.package()?;
        let mut files: BTreeMap<PathBuf, Vec<u8>> = package.files.into_iter().collect();
        let rendered: Vec<(PathBuf, Vec<u8>)> = package
            .glyphs
            .into_par_iter()
            .map(|(path, g)| (path, g.to_glif().into_bytes()))
            .collect();
        files.extend(rendered);
        Ok(files)
    }

    /// The package's files, with the .glif files left to render: validates
    /// first, so nothing is written for a UFO that ufoLib2 would refuse.
    fn package(&self) -> Result<Package<'_>, Error> {
        self.validate()?;
        let mut files = Vec::new();
        let mut put = |path: &str, content: String| {
            files.push((PathBuf::from(path), content.into_bytes()));
        };
        let mut meta = Dict::new();
        meta.insert("creator".into(), "com.github.fonttools.ufoLib".into());
        meta.insert("formatVersion".into(), 3.into());
        put("metainfo.plist", plist::dict_file(&meta));
        put("fontinfo.plist", plist::dict_file(&self.info));
        if !self.groups.is_empty() {
            put("groups.plist", plist::dict_file(&self.groups));
        }
        if !self.kerning.is_empty() {
            put("kerning.plist", plist::dict_file(&self.kerning));
        }
        if !self.features.is_empty() {
            put("features.fea", self.features.clone());
        }
        if !self.lib.is_empty() {
            put("lib.plist", plist::dict_file(&self.lib));
        }
        put(
            "layercontents.plist",
            plist::to_file(&Value::Array(vec![Value::Array(vec![
                "public.default".into(),
                "glyphs".into(),
            ])])),
        );
        let mut existing = HashSet::new();
        let mut contents = Dict::new();
        let mut glyphs = Vec::with_capacity(self.glyphs.len());
        for g in &self.glyphs {
            let file = glyph_file_name(&g.name, &mut existing);
            glyphs.push((Path::new("glyphs").join(&file), g));
            contents.insert(g.name.clone(), file.into());
        }
        put(
            "glyphs/contents.plist",
            plist::to_file(&Value::Dict(contents)),
        );
        Ok(Package { files, glyphs })
    }

    /// Write the UFO to `path`, which must not exist yet.
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        if path.exists() {
            return Err(format!("{} already exists", path.display()).into());
        }
        let timer = crate::Timer::new();
        let package = self.package()?;
        timer.lap("serialize");
        std::fs::create_dir_all(path.join("glyphs"))?;
        package.for_each_file(|rel, content| Ok(std::fs::write(path.join(rel), content)?))?;
        timer.lap("write");
        Ok(())
    }

    /// Update the UFO at `path` in place: rewrite only files whose content
    /// changed and remove files the UFO no longer has. Returns
    /// (files written, files removed).
    pub fn sync(&self, path: &Path) -> Result<(usize, usize), Error> {
        let timer = crate::Timer::new();
        let package = self.package()?;
        timer.lap("serialize");
        std::fs::create_dir_all(path.join("glyphs"))?;
        let written = AtomicUsize::new(0);
        package.for_each_file(|rel, content| {
            let full = path.join(rel);
            if !same_content(&full, content) {
                std::fs::write(&full, content)?;
                written.fetch_add(1, Ordering::Relaxed);
            }
            Ok(())
        })?;
        timer.lap("compare/write");
        let produced: HashSet<&Path> = package.paths().collect();
        let mut removed = 0;
        for dir in [path.to_path_buf(), path.join("glyphs")] {
            for entry in std::fs::read_dir(&dir)? {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    continue;
                }
                if !produced.contains(entry.path().strip_prefix(path)?) {
                    std::fs::remove_file(entry.path())?;
                    removed += 1;
                }
            }
        }
        Ok((written.into_inner(), removed))
    }
}

/// A UFO package ready to write: the top-level files and `contents.plist`
/// rendered, the .glif files as the glyphs to render.
struct Package<'a> {
    files: Vec<(PathBuf, Vec<u8>)>,
    glyphs: Vec<(PathBuf, &'a Glyph)>,
}

impl Package<'_> {
    /// Every file's path relative to the package.
    fn paths(&self) -> impl Iterator<Item = &Path> {
        self.files
            .iter()
            .map(|(p, _)| p.as_path())
            .chain(self.glyphs.iter().map(|(p, _)| p.as_path()))
    }

    /// Calls `f` on every file, in parallel. Each .glif is rendered just
    /// before `f` sees it and dropped right after, so the package's glyph
    /// files are never all in memory at once.
    fn for_each_file(
        &self,
        f: impl Fn(&Path, &[u8]) -> Result<(), Error> + Sync,
    ) -> Result<(), Error> {
        self.files
            .par_iter()
            .try_for_each(|(rel, content)| f(rel, content))?;
        self.glyphs
            .par_iter()
            .try_for_each(|(rel, g)| f(rel, g.to_glif().as_bytes()))
    }
}

/// Whether the file at `path` already holds `content`. The length is
/// checked first, so a changed file is usually not read.
fn same_content(path: &Path, content: &[u8]) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() && meta.len() == content.len() as u64 => {
            std::fs::read(path).is_ok_and(|old| old == content)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// File names from fontTools' `userNameToFileName(name, existing,
    /// suffix=".glif")`, each added to `existing` in turn.
    #[test]
    fn glyph_file_names_match_fonttools() {
        let cases: Vec<(String, String)> =
            serde_json::from_str(include_str!("../tests/data/filename_cases.json")).unwrap();
        let mut existing = HashSet::new();
        for (name, expected) in cases {
            assert_eq!(glyph_file_name(&name, &mut existing), expected, "{name:?}");
        }
    }
}
