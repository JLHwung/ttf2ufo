//! fontTools' views of the common layout structures: coverage tables as glyph
//! name lists, class definitions as `{glyph: class}` dicts, and the GDEF data
//! the unparser looks at.

use std::collections::HashMap;

use read_fonts::tables::gdef::Gdef;
use read_fonts::tables::layout::{ClassDef, CoverageTable};
use read_fonts::{FontRef, ReadError, TableProvider};

use super::model::ODict;
use crate::Error;

/// The glyph order, with fontTools' `getGlyphName` fallback for glyph IDs
/// beyond it.
pub struct GlyphNames {
    pub names: Vec<String>,
}

impl GlyphNames {
    pub fn name(&self, gid: u32) -> String {
        match self.names.get(gid as usize) {
            Some(n) => n.clone(),
            None => format!("glyph{gid:05}"),
        }
    }
}

pub fn read_err(e: ReadError) -> Error {
    format!("{e}").into()
}

/// `Coverage.glyphs` as fontTools' `Coverage.postRead` builds it.
pub fn coverage_glyphs(names: &GlyphNames, cov: &CoverageTable) -> Result<Vec<String>, Error> {
    match cov {
        CoverageTable::Format1(t) => Ok(t
            .glyph_array()
            .iter()
            .map(|g| names.name(g.get().to_u32()))
            .collect()),
        CoverageTable::Format2(t) => {
            let mut ranges: Vec<_> = t.range_records().iter().collect();
            // fontTools re-sorts ranges by StartCoverageIndex (stable sort).
            ranges.sort_by_key(|r| r.start_coverage_index());
            let mut glyphs = Vec::new();
            for r in ranges {
                let start = r.start_glyph_id().to_u32();
                let end = r.end_glyph_id().to_u32();
                for gid in start..end + 1 {
                    glyphs.push(names.name(gid));
                }
            }
            Ok(glyphs)
        }
        other => Err(format!("unsupported Coverage format {}", other.coverage_format()).into()),
    }
}

pub fn opt_coverage_glyphs(
    names: &GlyphNames,
    cov: Result<CoverageTable, ReadError>,
) -> Result<Vec<String>, Error> {
    coverage_glyphs(names, &cov.map_err(read_err)?)
}

/// `ClassDef.classDefs` as fontTools' `ClassDef.postRead` builds it (class 0
/// entries are omitted).
pub fn class_defs(names: &GlyphNames, cd: &ClassDef) -> Result<ODict<String, u16>, Error> {
    let mut defs = ODict::new();
    match cd {
        ClassDef::Format1(t) => {
            let start = t.start_glyph_id().to_u32();
            for (i, cls) in t.class_value_array().iter().enumerate() {
                let cls = cls.get();
                if cls != 0 {
                    defs.insert(names.name(start + i as u32), cls);
                }
            }
        }
        ClassDef::Format2(t) => {
            for rec in t.class_range_records() {
                let cls = rec.class();
                if cls == 0 {
                    continue;
                }
                let start = rec.start_glyph_id().to_u32();
                let end = rec.end_glyph_id().to_u32();
                for gid in start..end + 1 {
                    defs.insert(names.name(gid), cls);
                }
            }
        }
        other => {
            return Err(format!("unsupported ClassDef format {}", other.class_format()).into());
        }
    }
    Ok(defs)
}

/// `GTableUnparser._invertClassDef`: class -> glyphs, in first-seen order,
/// plus class 0 holding every other glyph of the font.
///
/// In Python class 0 is a `set` of glyph names, whose iteration order depends
/// on PYTHONHASHSEED; here it is in glyph order. Positioning rules sort their
/// classes before output, so this only shows through in contextual format 2
/// rules whose class 0 has 2 to 5 glyphs (larger classes are sorted into
/// named classes).
pub fn invert_class_def(names: &GlyphNames, defs: &ODict<String, u16>) -> ODict<u16, Vec<String>> {
    let mut classes: ODict<u16, Vec<String>> = ODict::new();
    for (glyph, cls) in &defs.items {
        if let Some(v) = classes.get_mut(cls) {
            v.push(glyph.clone());
        } else {
            classes.insert(*cls, vec![glyph.clone()]);
        }
    }
    let class0: Vec<String> = names
        .names
        .iter()
        .filter(|g| !defs.contains_key(g))
        .cloned()
        .collect();
    classes.insert(0, class0);
    classes
}

/// The parts of GDEF that the unparser and `categorize_glyph` read. Each is
/// `Err` when reading it would fail in Python (missing table or subtable).
pub struct GdefInfo {
    pub glyph_class: Result<HashMap<String, u16>, String>,
    pub mark_attach: Result<ODict<String, u16>, String>,
    pub mark_glyph_sets: Result<Vec<Vec<String>>, String>,
    pub present: bool,
}

impl GdefInfo {
    pub fn load(font: &FontRef, names: &GlyphNames) -> Self {
        let gdef: Option<Gdef> = font.gdef().ok();
        let Some(gdef) = gdef else {
            let missing = "the font has no GDEF table".to_string();
            return GdefInfo {
                glyph_class: Err(missing.clone()),
                mark_attach: Err(missing.clone()),
                mark_glyph_sets: Err(missing),
                present: false,
            };
        };
        let glyph_class = match gdef.original_glyph_class_def() {
            None => Err("GDEF has no GlyphClassDef".to_string()),
            Some(cd) => cd
                .map_err(|e| e.to_string())
                .and_then(|cd| class_defs(names, &cd).map_err(|e| e.to_string()))
                .map(|d| d.items.into_iter().collect()),
        };
        let mark_attach = match gdef.original_mark_attach_class_def() {
            None => Err("GDEF has no MarkAttachClassDef".to_string()),
            Some(cd) => cd
                .map_err(|e| e.to_string())
                .and_then(|cd| class_defs(names, &cd).map_err(|e| e.to_string())),
        };
        let mark_glyph_sets = match gdef.original_mark_glyph_sets_def() {
            None => Err("GDEF has no MarkGlyphSetsDef".to_string()),
            Some(sets) => sets.map_err(|e| e.to_string()).and_then(|sets| {
                sets.coverages()
                    .iter()
                    .map(|c| opt_coverage_glyphs(names, c).map_err(|e| e.to_string()))
                    .collect()
            }),
        };
        GdefInfo {
            glyph_class,
            mark_attach,
            mark_glyph_sets,
            present: true,
        }
    }

    /// `fontFeatures.utils.categorize_glyph(font, name)[0] == "mark"`
    pub fn is_mark(&self, glyph: &str) -> Result<bool, Error> {
        if !self.present {
            return Ok(false);
        }
        let classes = self
            .glyph_class
            .as_ref()
            .map_err(|e| Error::from(e.clone()))?;
        Ok(classes.get(glyph) == Some(&3))
    }
}
