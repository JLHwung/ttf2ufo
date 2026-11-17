//! Glyph order and names, as fontTools' `TTFont.getGlyphOrder()` builds them
//! from the `post` table.

use read_fonts::tables::post::DEFAULT_GLYPH_NAMES;
use read_fonts::types::Tag;
use read_fonts::{FontRef, TableProvider};
use std::collections::{HashMap, HashSet};

use crate::Error;

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

/// The font's glyph order, with fontTools' handling of empty and duplicate
/// names (`glyph%05d`, `name.N`).
pub fn glyph_order(font: &FontRef) -> Result<Vec<String>, Error> {
    let num_glyphs = font.maxp()?.num_glyphs() as usize;
    let post = font
        .table_data(Tag::new(b"post"))
        .ok_or("fonts without a 'post' table are not supported")?;
    let post = post.as_bytes();
    let version = u32::from_be_bytes(post.get(0..4).ok_or("bad 'post' table")?.try_into()?);
    let mut order: Vec<String> = match version {
        0x0001_0000 => {
            if num_glyphs > DEFAULT_GLYPH_NAMES.len() {
                return Err(
                    "'post' format 1 has too few names; cmap-derived glyph names are not supported"
                        .into(),
                );
            }
            DEFAULT_GLYPH_NAMES[..num_glyphs]
                .iter()
                .map(|s| s.to_string())
                .collect()
        }
        0x0002_0000 => {
            let declared = be16(post, 32).ok_or("bad 'post' table")? as usize;
            let count = declared.min(num_glyphs);
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
        _ => {
            return Err(format!(
                "'post' version {:#010x} has no glyph names; cmap-derived glyph names are not supported",
                version
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
