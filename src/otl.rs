//! fontTools' views of the OpenType layout structures that both the kerning
//! and anchor extraction (`layout`) and the feature unparser (`features`)
//! read: tags, coverage tables, class definitions and mark attachment
//! subtables.

use indexmap::IndexMap;
use read_fonts::ReadError;
use read_fonts::tables::gpos::{AnchorTable, MarkArray, MarkBasePosFormat1, MarkMarkPosFormat1};
use read_fonts::tables::layout::{ClassDef, CoverageTable};
use read_fonts::types::Tag;

use crate::Error;
use crate::names::GlyphOrder;

/// fontTools reads tags as Latin-1 strings.
pub fn tag_str(tag: Tag) -> String {
    tag.to_be_bytes().iter().map(|&b| b as char).collect()
}

/// The glyph IDs of a coverage table, in the order of fontTools'
/// `Coverage.glyphs`: format 2 ranges are re-sorted by StartCoverageIndex
/// (a stable sort).
pub fn coverage_gids(cov: &CoverageTable) -> Result<Vec<u32>, Error> {
    match cov {
        CoverageTable::Format1(t) => Ok(t.glyph_array().iter().map(|g| g.get().to_u32()).collect()),
        CoverageTable::Format2(t) => {
            let mut ranges: Vec<_> = t.range_records().iter().collect();
            ranges.sort_by_key(|r| r.start_coverage_index());
            let mut gids = Vec::new();
            for r in ranges {
                gids.extend(r.start_glyph_id().to_u32()..r.end_glyph_id().to_u32() + 1);
            }
            Ok(gids)
        }
        other => Err(format!("unsupported Coverage format {}", other.coverage_format()).into()),
    }
}

/// `Coverage.glyphs` as fontTools' `Coverage.postRead` builds it.
pub fn coverage_glyphs(names: &GlyphOrder, cov: &CoverageTable) -> Result<Vec<String>, Error> {
    Ok(coverage_gids(cov)?
        .into_iter()
        .map(|gid| names.name(gid))
        .collect())
}

/// `ClassDef.classDefs` as fontTools' `ClassDef.postRead` builds it (class 0
/// entries are omitted).
pub fn class_defs(names: &GlyphOrder, cd: &ClassDef) -> Result<IndexMap<String, u16>, Error> {
    let mut defs = IndexMap::new();
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

/// A mark-to-base or mark-to-mark subtable (format 1), with the "base" side
/// being the base glyphs or the mark2 glyphs.
pub struct MarkAttachment<'a> {
    /// `MarkClassCount`.
    pub class_count: u16,
    /// The (mark1) mark coverage.
    pub mark_coverage: CoverageTable<'a>,
    /// The (mark1) mark array.
    pub marks: MarkArray<'a>,
    /// The base (or mark2) coverage.
    pub base_coverage: CoverageTable<'a>,
    /// For each base (or mark2) record, its anchor per mark class; null
    /// offsets are `None`. Anchors are read when used, as in fontTools.
    pub base_anchors: Vec<Vec<Option<Result<AnchorTable<'a>, ReadError>>>>,
}

impl<'a> MarkAttachment<'a> {
    /// A mark-to-base subtable.
    pub fn mark_to_base(t: &MarkBasePosFormat1<'a>) -> Result<Self, ReadError> {
        let bases = t.base_array()?;
        let base_anchors = bases
            .base_records()
            .iter()
            .map(|r| Ok(r?.base_anchors(bases.offset_data()).iter().collect()))
            .collect::<Result<_, ReadError>>()?;
        Ok(MarkAttachment {
            class_count: t.mark_class_count(),
            mark_coverage: t.mark_coverage()?,
            marks: t.mark_array()?,
            base_coverage: t.base_coverage()?,
            base_anchors,
        })
    }

    /// A mark-to-mark subtable: mark2 glyphs take the place of bases.
    pub fn mark_to_mark(t: &MarkMarkPosFormat1<'a>) -> Result<Self, ReadError> {
        let bases = t.mark2_array()?;
        let base_anchors = bases
            .mark2_records()
            .iter()
            .map(|r| Ok(r?.mark2_anchors(bases.offset_data()).iter().collect()))
            .collect::<Result<_, ReadError>>()?;
        Ok(MarkAttachment {
            class_count: t.mark_class_count(),
            mark_coverage: t.mark1_coverage()?,
            marks: t.mark1_array()?,
            base_coverage: t.mark2_coverage()?,
            base_anchors,
        })
    }
}
