//! An in-memory UFO and its serialization, matching what ufoLib2's
//! `Font.save(path, structure="package")` writes.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::Error;
use crate::plist::{self, Dict, Value, escape_attr};
use crate::pyfmt::Num;

#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    pub x: i64,
    pub y: i64,
    /// Segment type; `None` for off-curve points.
    pub typ: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    pub base: String,
    /// (xScale, xyScale, yxScale, yScale, xOffset, yOffset)
    pub transform: [Num; 6],
    pub identifier: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Anchor {
    pub x: i64,
    pub y: i64,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    pub name: String,
    pub width: i64,
    pub height: i64,
    pub unicodes: Vec<u32>,
    pub anchors: Vec<Anchor>,
    pub contours: Vec<Vec<Point>>,
    pub components: Vec<Component>,
    pub lib: Dict,
}

impl Glyph {
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
    const ILLEGAL: &[char] = &[
        '"', '*', '+', '/', ':', '<', '>', '?', '[', '\\', ']', '|', '\0',
    ];
    const RESERVED: &[&str] = &[
        "con", "prn", "aux", "clock$", "nul", "a:-z:", "com1", "lpt1", "lpt2", "lpt3", "com2",
        "com3", "com4",
    ];
    let mut user = name.to_string();
    if user.starts_with('.') {
        user = format!("_{}", &user[1..]);
    }
    let mut filtered = String::new();
    for c in user.chars() {
        if ILLEGAL.contains(&c) || (c as u32) < 32 || c as u32 == 0x7F {
            filtered.push('_');
        } else {
            filtered.push(c);
            if c.to_lowercase().collect::<String>() != c.to_string() {
                filtered.push('_');
            }
        }
    }
    let base: String = filtered
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
    let base: String = base.chars().take(255 - ".glif".len()).collect();
    let mut file = format!("{base}.glif");
    if existing_lower.contains(&file.to_lowercase()) {
        // handleClash1: 15-digit counter between name and suffix
        let mut n = 1u64;
        loop {
            let cand = format!("{base}{n:015}.glif");
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

/// A whole UFO in memory.
#[derive(Clone, Debug, Default)]
pub struct Ufo {
    pub info: Dict,
    pub lib: Dict,
    pub groups: Dict,
    pub kerning: Dict,
    pub features: String,
    /// In glyph order.
    pub glyphs: Vec<Glyph>,
}

impl Ufo {
    pub fn glyph(&self, name: &str) -> Option<&Glyph> {
        self.glyphs.iter().find(|g| g.name == name)
    }

    /// Every file of the UFO package: relative path -> content.
    pub fn to_files(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        let mut put = |path: &str, content: String| {
            files.insert(PathBuf::from(path), content.into_bytes());
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
        let file_names: Vec<String> = self
            .glyphs
            .iter()
            .map(|g| glyph_file_name(&g.name, &mut existing))
            .collect();
        let mut contents = Dict::new();
        for (g, f) in self.glyphs.iter().zip(&file_names) {
            contents.insert(g.name.clone(), f.clone().into());
        }
        put(
            "glyphs/contents.plist",
            plist::to_file(&Value::Dict(contents)),
        );
        let rendered: Vec<(PathBuf, Vec<u8>)> = self
            .glyphs
            .par_iter()
            .zip(file_names.par_iter())
            .map(|(g, f)| (Path::new("glyphs").join(f), g.to_glif().into_bytes()))
            .collect();
        files.extend(rendered);
        files
    }

    /// Write the UFO to `path`, which must not exist yet.
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        if path.exists() {
            return Err(format!("{} already exists", path.display()).into());
        }
        let timer = crate::Timer::new();
        let files = self.to_files();
        timer.lap("serialize");
        write_files(path, &files)?;
        timer.lap("write");
        Ok(())
    }

    /// Update the UFO at `path` in place: rewrite only files whose content
    /// changed and remove files the UFO no longer has. Returns
    /// (files written, files removed).
    pub fn sync(&self, path: &Path) -> Result<(usize, usize), Error> {
        let timer = crate::Timer::new();
        let files = self.to_files();
        timer.lap("serialize");
        std::fs::create_dir_all(path.join("glyphs"))?;
        let written = files
            .par_iter()
            .map(|(rel, content)| -> Result<bool, Error> {
                let full = path.join(rel);
                match std::fs::read(&full) {
                    Ok(old) if old == *content => Ok(false),
                    _ => {
                        std::fs::write(&full, content)?;
                        Ok(true)
                    }
                }
            })
            .collect::<Result<Vec<bool>, Error>>()?
            .into_iter()
            .filter(|w| *w)
            .count();
        timer.lap("compare/write");
        let mut removed = 0;
        for dir in [path.to_path_buf(), path.join("glyphs")] {
            for entry in std::fs::read_dir(&dir)? {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    continue;
                }
                let rel = entry.path().strip_prefix(path)?.to_path_buf();
                if !files.contains_key(&rel) {
                    std::fs::remove_file(entry.path())?;
                    removed += 1;
                }
            }
        }
        Ok((written, removed))
    }
}

fn write_files(root: &Path, files: &BTreeMap<PathBuf, Vec<u8>>) -> Result<(), Error> {
    std::fs::create_dir_all(root.join("glyphs"))?;
    files
        .par_iter()
        .map(|(rel, content)| std::fs::write(root.join(rel), content).map_err(Error::from))
        .collect::<Result<Vec<()>, Error>>()?;
    Ok(())
}
