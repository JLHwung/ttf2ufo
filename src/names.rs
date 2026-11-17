//! Glyph order and names, as fontTools' `TTFont.getGlyphOrder()` builds them
//! from the `post` table.

use read_fonts::tables::post::DEFAULT_GLYPH_NAMES;
use read_fonts::types::Tag;
use read_fonts::{FontRef, TableProvider};
use std::collections::{HashMap, HashSet};

use crate::Error;
use crate::agl::uv2agl;

/// `TTFont._getGlyphNamesFromCmap`: names made from each glyph's lowest
/// Unicode code point (`_makeGlyphName`), `glyph%05d` for the rest.
fn names_from_cmap(font: &FontRef, num_glyphs: usize) -> Result<Vec<String>, Error> {
    if num_glyphs == 0 {
        return Err("fonts without glyphs are not supported".into());
    }
    let mut order: Vec<String> = (0..num_glyphs).map(|i| format!("glyph{i:05}")).collect();
    order[0] = ".notdef".to_string();
    let reversed = crate::cmap::reversed_min(font)?;
    let mut use_count: HashMap<String, usize> = HashMap::new();
    for (gid, slot) in order.iter_mut().enumerate() {
        let Some(&uv) = reversed.get(&(gid as u32)) else {
            continue;
        };
        let mut name = match uv2agl(uv) {
            Some(name) => name.to_string(),
            None if uv <= 0xFFFF => format!("uni{uv:04X}"),
            None => format!("u{uv:X}"),
        };
        let uses = use_count.entry(name.clone()).or_insert(0);
        *uses += 1;
        if *uses > 1 {
            name = format!("{name}.alt{}", *uses - 1);
        }
        *slot = name;
    }
    Ok(order)
}

fn be16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

/// fontTools' `unpackPStrings`: `n` Pascal strings, decoded as Latin-1.
fn unpack_pstrings(data: &[u8], n: usize) -> Vec<String> {
    let mut strings = Vec::with_capacity(n);
    let mut index = 0usize;
    for _ in 0..n {
        let length = data.get(index).map_or(0, |b| *b as usize);
        index += 1;
        let name = match data.get(index..index + length) {
            Some(bytes) if index + length <= data.len() => {
                bytes.iter().map(|b| *b as char).collect()
            }
            _ => String::new(),
        };
        strings.push(name);
        index += length;
    }
    strings
}

/// The font's glyph order, as fontTools' `TTFont.getGlyphOrder()` builds it,
/// indexed by name.
#[derive(Clone, Debug, Default)]
pub struct GlyphOrder {
    names: Vec<String>,
    index: HashMap<String, u32>,
}

impl GlyphOrder {
    /// A glyph order made of `names`.
    pub fn new(names: Vec<String>) -> Self {
        let index = names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.clone(), i as u32))
            .collect();
        GlyphOrder { names, index }
    }

    /// The names, by glyph ID.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// The number of glyphs.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether the font has no glyphs.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// fontTools' `getGlyphName`: glyph IDs beyond the order are named
    /// `glyph%05d`.
    pub fn name(&self, gid: u32) -> String {
        match self.names.get(gid as usize) {
            Some(name) => name.clone(),
            None => format!("glyph{gid:05}"),
        }
    }

    /// The glyph ID of a name in the order.
    pub fn gid(&self, name: &str) -> Option<u32> {
        self.index.get(name).copied()
    }
}

/// The font's glyph order, with fontTools' handling of empty and duplicate
/// names (`glyph%05d`, `name.N`).
pub fn glyph_order(font: &FontRef) -> Result<GlyphOrder, Error> {
    glyph_names(font).map(GlyphOrder::new)
}

fn glyph_names(font: &FontRef) -> Result<Vec<String>, Error> {
    let num_glyphs = font.maxp()?.num_glyphs() as usize;
    let post = font
        .table_data(Tag::new(b"post"))
        .ok_or("fonts without a 'post' table are not supported")?;
    let post = post.as_bytes();
    let version = u32::from_be_bytes(post.get(0..4).ok_or("bad 'post' table")?.try_into()?);
    let mut order: Vec<String> = match version {
        0x0001_0000 => {
            if num_glyphs > DEFAULT_GLYPH_NAMES.len() {
                // "Not enough names found in the 'post' table"
                return names_from_cmap(font, num_glyphs);
            }
            DEFAULT_GLYPH_NAMES[..num_glyphs]
                .iter()
                .map(|s| s.to_string())
                .collect()
        }
        0x0002_0000 => {
            let declared = be16(post, 32).ok_or("bad 'post' table")? as usize;
            let count = declared.min(num_glyphs);
            if count == 0 {
                // `max(indices)` of an empty array: ValueError
                return Err("'post' format 2 table without glyphs".into());
            }
            let indices: Vec<usize> = (0..count)
                .map(|i| {
                    be16(post, 34 + 2 * i)
                        .map(|v| v as usize)
                        .ok_or("bad 'post' table")
                })
                .collect::<Result<_, _>>()?;
            let max_index = indices.iter().copied().max().unwrap_or(0);
            let strings = post.get(34 + 2 * count..).unwrap_or_default();
            let extra = unpack_pstrings(strings, max_index.saturating_sub(257));
            let mut order = vec![String::new(); num_glyphs];
            for (gid, &index) in indices.iter().enumerate() {
                order[gid] = if index > 257 {
                    extra.get(index - 258).cloned().unwrap_or_default()
                } else {
                    DEFAULT_GLYPH_NAMES[index].to_string()
                };
            }
            order
        }
        0x0003_0000 => return names_from_cmap(font, num_glyphs),
        0x0004_0000 => {
            // decode_format_4_0: a code point per glyph, named from the AGL
            let data = post.get(32..).unwrap_or_default();
            if data.len() % 2 != 0 {
                return Err("bad 'post' format 4 table".into());
            }
            let mut order = vec![String::new(); num_glyphs];
            for (slot, c) in order.iter_mut().zip(data.as_chunks::<2>().0) {
                let uv = u16::from_be_bytes(*c) as u32;
                *slot = match uv {
                    0xFFFF => String::new(),
                    _ => match uv2agl(uv) {
                        Some(name) => name.to_string(),
                        None => format!("uni{uv:04X}"),
                    },
                };
            }
            order
        }
        _ => {
            return Err(format!(
                "'post' table format {} not supported",
                crate::pyfmt::float_repr(version as f64 / 65536.0)
            )
            .into());
        }
    };

    // post.build_psNameMapping: fill in empty names, make duplicates unique
    let original: HashSet<String> = order.iter().cloned().collect();
    let mut all_names: HashMap<String, usize> = HashMap::new();
    for (i, slot) in order.iter_mut().enumerate() {
        let mut name = if slot.is_empty() {
            format!("glyph{i:05}")
        } else {
            slot.clone()
        };
        if let Some(&n0) = all_names.get(&name) {
            let mut n = n0;
            while original.contains(&format!("{name}.{n}")) {
                n += 1;
            }
            all_names.insert(name.clone(), n + 1);
            name = format!("{name}.{n}");
        }
        all_names.insert(name.clone(), 1);
        *slot = name;
    }
    Ok(order)
}
