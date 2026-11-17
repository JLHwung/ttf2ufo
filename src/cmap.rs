//! Character mappings, as fontTools decodes the `cmap` table.

use indexmap::IndexMap;
use read_fonts::tables::cmap::{Cmap, CmapSubtable, PlatformId};
use read_fonts::{FontRef, TableProvider};

use crate::Error;
use crate::pyfmt::int_set_order;

/// One decoded subtable: fontTools' `subtable.cmap` dict, code -> glyph id,
/// in dict (insertion) order. Mappings to glyph 0 are dropped, as
/// fontTools' `_make_map` does.
struct Subtable {
    platform: u16,
    encoding: u16,
    map: IndexMap<u32, u32>,
}

impl Subtable {
    /// `CmapSubtable.isUnicode()`
    fn is_unicode(&self) -> bool {
        self.platform == 0 || (self.platform == 3 && matches!(self.encoding, 0 | 1 | 10))
    }
}

fn insert(map: &mut IndexMap<u32, u32>, code: u32, gid: u32) {
    if gid != 0 {
        map.insert(code, gid);
    }
}

fn decode_subtables(cmap: &Cmap) -> Result<Vec<Subtable>, Error> {
    let mut out = Vec::new();
    for (i, record) in cmap.encoding_records().iter().enumerate() {
        let platform = match record.platform_id() {
            PlatformId::Unicode => 0,
            PlatformId::Macintosh => 1,
            PlatformId::ISO => 2,
            PlatformId::Windows => 3,
            PlatformId::Custom => 4,
            _ => 0xFFFF,
        };
        let encoding = record.encoding_id();
        let mut map = IndexMap::new();
        match cmap.subtable(i as u16)? {
            CmapSubtable::Format0(t) => {
                for (code, gid) in t.glyph_id_array().iter().enumerate() {
                    insert(&mut map, code as u32, *gid as u32);
                }
            }
            CmapSubtable::Format4(t) => {
                let start = t.start_code();
                let end = t.end_code();
                let delta = t.id_delta();
                let range_offset = t.id_range_offsets();
                let glyph_ids = t.glyph_id_array();
                let seg_count = start.len();
                // fontTools skips the last segment ("don't do 0xffff!")
                for i in 0..seg_count.saturating_sub(1) {
                    let (s, e) = (start[i].get() as i64, end[i].get() as i64);
                    let d = delta[i].get() as i64;
                    let ro = range_offset[i].get() as i64;
                    let partial = ro / 2 - s + i as i64 - seg_count as i64;
                    for code in s..=e {
                        let gid = if ro == 0 {
                            (code + d) & 0xFFFF
                        } else {
                            let index = code + partial;
                            let g = glyph_ids
                                .get(
                                    usize::try_from(index)
                                        .map_err(|_| "bad cmap format 4 index")?,
                                )
                                .ok_or("bad cmap format 4 index")?
                                .get() as i64;
                            if g != 0 { (g + d) & 0xFFFF } else { 0 }
                        };
                        insert(&mut map, code as u32, gid as u32);
                    }
                }
            }
            CmapSubtable::Format6(t) => {
                let first = t.first_code() as u32;
                for (i, gid) in t.glyph_id_array().iter().enumerate() {
                    insert(&mut map, first + i as u32, gid.get() as u32);
                }
            }
            CmapSubtable::Format12(t) => {
                for g in t.groups() {
                    for code in g.start_char_code()..=g.end_char_code() {
                        insert(
                            &mut map,
                            code,
                            g.start_glyph_id() + (code - g.start_char_code()),
                        );
                    }
                }
            }
            CmapSubtable::Format13(t) => {
                for g in t.groups() {
                    for code in g.start_char_code()..=g.end_char_code() {
                        insert(&mut map, code, g.glyph_id());
                    }
                }
            }
            CmapSubtable::Format14(_) => {}
            other => {
                let unicode = platform == 0 || (platform == 3 && matches!(encoding, 0 | 1 | 10));
                if unicode {
                    return Err(format!(
                        "cmap subtable format {} is not supported",
                        format_of(&other)
                    )
                    .into());
                }
            }
        }
        out.push(Subtable {
            platform,
            encoding,
            map,
        });
    }
    Ok(out)
}

fn format_of(t: &CmapSubtable) -> u16 {
    match t {
        CmapSubtable::Format0(_) => 0,
        CmapSubtable::Format2(_) => 2,
        CmapSubtable::Format4(_) => 4,
        CmapSubtable::Format6(_) => 6,
        CmapSubtable::Format8(_) => 8,
        CmapSubtable::Format10(_) => 10,
        CmapSubtable::Format12(_) => 12,
        CmapSubtable::Format13(_) => 13,
        CmapSubtable::Format14(_) => 14,
        CmapSubtable::Format15(_) => 15,
    }
}

/// A variation selector's entries: (base char, Some(glyph id) | None for default UVS).
pub type UvsRecord = (u32, Vec<(u32, Option<u32>)>);

/// Character mapping data extractor needs.
pub struct CharMap {
    /// Per glyph id: its code points in `list(cmap.buildReversed()[name])` order.
    pub unicodes: Vec<Vec<u32>>,
    /// `cmap.getBestCmap()`, code -> glyph id.
    pub best: Option<IndexMap<u32, u32>>,
    /// Format 14 subtables: variation selector -> [(base char, Some(glyph) | None for default)].
    pub uvs: Vec<UvsRecord>,
}

pub fn char_map(font: &FontRef, num_glyphs: usize) -> Result<CharMap, Error> {
    let Ok(cmap) = font.cmap() else {
        return Ok(CharMap {
            unicodes: vec![Vec::new(); num_glyphs],
            best: None,
            uvs: Vec::new(),
        });
    };
    let subtables = decode_subtables(&cmap)?;

    // buildReversed: name -> set of code points, over all Unicode subtables
    let mut inserted: Vec<Vec<u64>> = vec![Vec::new(); num_glyphs];
    for t in subtables.iter().filter(|t| t.is_unicode()) {
        for (&code, &gid) in &t.map {
            if let Some(v) = inserted.get_mut(gid as usize) {
                v.push(code as u64);
            }
        }
    }
    let unicodes = inserted
        .into_iter()
        .map(|codes| int_set_order(codes).into_iter().map(|c| c as u32).collect())
        .collect();

    // getBestCmap preference order
    const PREFS: [(u16, u16); 8] = [
        (3, 10),
        (0, 6),
        (0, 4),
        (3, 1),
        (0, 3),
        (0, 2),
        (0, 1),
        (0, 0),
    ];
    let best = PREFS.iter().find_map(|&(p, e)| {
        subtables
            .iter()
            .find(|t| t.platform == p && t.encoding == e)
            .map(|t| t.map.clone())
    });

    let mut uvs = Vec::new();
    for (i, record) in cmap.encoding_records().iter().enumerate() {
        let _ = record;
        if let CmapSubtable::Format14(t) = cmap.subtable(i as u16)? {
            let data = t.offset_data();
            for selector in t.var_selector() {
                let mut entries = Vec::new();
                if let Some(default) = selector.default_uvs(data) {
                    for range in default?.ranges() {
                        let first = range.start_unicode_value().to_u32();
                        for uv in first..=first + range.additional_count() as u32 {
                            entries.push((uv, None));
                        }
                    }
                }
                if let Some(non_default) = selector.non_default_uvs(data) {
                    for mapping in non_default?.uvs_mapping() {
                        entries.push((
                            mapping.unicode_value().to_u32(),
                            Some(mapping.glyph_id() as u32),
                        ));
                    }
                }
                uvs.push((selector.var_selector().to_u32(), entries));
            }
        }
    }
    Ok(CharMap {
        unicodes,
        best,
        uvs,
    })
}
