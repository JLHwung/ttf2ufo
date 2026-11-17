//! The unparser's views of class definitions (`_invertClassDef`) and of the
//! GDEF data it looks at. Coverage and ClassDef reading is shared with the
//! kerning extraction, in [`crate::otl`].

use std::collections::HashMap;

use indexmap::IndexMap;
use read_fonts::tables::gdef::Gdef;
use read_fonts::{FontRef, TableProvider};

use crate::Error;
use crate::names::GlyphOrder;
use crate::otl::{class_defs, coverage_glyphs};

/// `GTableUnparser._invertClassDef`: class -> glyphs, in first-seen order,
/// plus class 0 holding every other glyph of the font.
///
/// In Python class 0 is a `set` of glyph names, whose iteration order depends
/// on PYTHONHASHSEED; here it is in glyph order. Positioning rules sort their
/// classes before output, so this only shows through in contextual format 2
/// rules whose class 0 has 2 to 5 glyphs (larger classes are sorted into
/// named classes).
pub fn invert_class_def(
    names: &GlyphOrder,
    defs: &IndexMap<String, u16>,
) -> IndexMap<u16, Vec<String>> {
    let mut classes: IndexMap<u16, Vec<String>> = IndexMap::new();
    for (glyph, cls) in defs {
        classes.entry(*cls).or_default().push(glyph.clone());
    }
    let class0: Vec<String> = names
        .names()
        .iter()
        .filter(|g| !defs.contains_key(g.as_str()))
        .cloned()
        .collect();
    classes.insert(0, class0);
    classes
}

/// The parts of GDEF that the unparser and `categorize_glyph` read. Each is
/// `Err` when reading it would fail in Python (missing table or subtable).
pub struct GdefInfo {
    pub glyph_class: Result<HashMap<String, u16>, String>,
    pub mark_attach: Result<IndexMap<String, u16>, String>,
    pub mark_glyph_sets: Result<Vec<Vec<String>>, String>,
    pub present: bool,
}

impl GdefInfo {
    pub fn load(font: &FontRef, names: &GlyphOrder) -> Self {
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
                .map(|d| d.into_iter().collect()),
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
                    .map(|c| {
                        c.map_err(Error::from)
                            .and_then(|c| coverage_glyphs(names, &c))
                            .map_err(|e| e.to_string())
                    })
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
