//! Extract a UFO from a TrueType font, byte-for-byte as ufo-extractor's
//! `extractufo` (fontTools / ufoLib2) does.

pub mod cmap;
pub mod features;
pub mod glyphs;
pub mod info;
pub mod instructions;
pub mod layout;
pub mod names;
pub mod plist;
pub mod pyfmt;
pub mod ufo;

/// Error type used throughout the crate.
pub type Error = Box<dyn std::error::Error + Send + Sync>;

use read_fonts::types::Tag;
use read_fonts::{FontRef, TableProvider};

use plist::{Dict, Value};
use ufo::Ufo;

/// Extract a UFO from TrueType font data, as `extractufo` does.
pub fn extract(data: &[u8]) -> Result<Ufo, Error> {
    let font = FontRef::new(data)?;
    if font.table_data(Tag::new(b"glyf")).is_none() {
        return Err("only TrueType-flavored fonts (with a 'glyf' table) are supported".into());
    }
    let names = names::glyph_order(&font)?;

    // extractOpenTypeInfo
    let (info, mut lib) = info::extract_info(&font)?;

    // extractOpenTypeGlyphs + extractUnicodeVariationSequences
    let char_map = cmap::char_map(&font, names.len())?;
    let mut glyphs = glyphs::extract_glyphs(&font, &names, &char_map.unicodes)?;
    if !char_map.uvs.is_empty() {
        let mut sequences = Dict::new();
        for (selector, entries) in &char_map.uvs {
            let mut map = Dict::new();
            for (base, glyph) in entries {
                let gid = match glyph {
                    Some(gid) => *gid,
                    None => *char_map
                        .best
                        .as_ref()
                        .and_then(|best| best.get(base))
                        .ok_or_else(|| format!("default UVS U+{base:04X} has no cmap entry"))?,
                };
                let name = names.get(gid as usize).cloned().unwrap_or_else(|| format!("glyph{gid:05}"));
                map.insert(format!("{base:04X}"), name.into());
            }
            sequences.insert(format!("{selector:04X}"), Value::Dict(map));
        }
        lib.insert("public.unicodeVariationSequences".into(), Value::Dict(sequences));
    }

    // extractGlyphOrder
    if !names.is_empty() {
        lib.insert("public.glyphOrder".into(), names.clone().into());
    }

    // extractOpenTypeKerning
    let (kerning, groups) = layout::extract_kerning(&font, &names)?;

    // extractOpenTypeFeatures
    let features = features::extract_features(&font)?;

    // extractInstructions (glyph programs are handled with the glyphs)
    let mut tt = Dict::new();
    tt.insert("formatVersion".into(), "1".into());
    if let Some(cvt) = font.table_data(Tag::new(b"cvt ")) {
        let values: Dict = cvt
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .enumerate()
            .map(|(i, c)| (i.to_string(), Value::Int(i16::from_be_bytes([c[0], c[1]]) as i64)))
            .collect();
        tt.insert("controlValue".into(), Value::Dict(values));
    }
    if let Some(fpgm) = font.table_data(Tag::new(b"fpgm")) {
        tt.insert("fontProgram".into(), instructions::lib_assembly(fpgm.as_bytes())?.into());
    }
    let maxp = font.maxp()?;
    for (key, value) in [
        ("maxFunctionDefs", maxp.max_function_defs()),
        ("maxInstructionDefs", maxp.max_instruction_defs()),
        ("maxStackElements", maxp.max_stack_elements()),
        ("maxStorage", maxp.max_storage()),
        ("maxTwilightPoints", maxp.max_twilight_points()),
        ("maxZones", maxp.max_zones()),
    ] {
        tt.insert(key.into(), Value::Int(value.ok_or("'maxp' version 0.5 is not supported")? as i64));
    }
    if let Some(prep) = font.table_data(Tag::new(b"prep")) {
        tt.insert("controlValueProgram".into(), instructions::lib_assembly(prep.as_bytes())?.into());
    }
    lib.insert("public.truetype.instructions".into(), Value::Dict(tt));

    // extractAnchors
    let index: std::collections::HashMap<&str, usize> =
        names.iter().enumerate().map(|(i, n)| (n.as_str(), i)).collect();
    for (glyph, anchors) in layout::extract_anchors(&font, &names)? {
        let gid = *index.get(glyph.as_str()).ok_or_else(|| format!("anchor for missing glyph {glyph}"))?;
        glyphs[gid].anchors.extend(anchors);
    }

    Ok(Ufo { info, lib, groups, kerning, features, glyphs })
}
