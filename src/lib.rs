//! Extract a UFO from a TrueType font, byte-for-byte as ufo-extractor's
//! `extractufo` (fontTools / ufoLib2) does.
//!
//! [`extract`] reads the font into an in-memory [`Ufo`], which can then be
//! written as a new UFO package or used to update an existing one:
//!
//! ```no_run
//! use std::path::Path;
//!
//! # fn main() -> Result<(), ttf2ufo::Error> {
//! let data = std::fs::read("Font.ttf")?;
//! let ufo = ttf2ufo::extract(&data)?;
//! println!("{} glyphs", ufo.glyphs.len());
//!
//! // Write a new UFO; fails if the path exists.
//! ufo.save(Path::new("Font.ttf.ufo"))?;
//!
//! // Or update an existing UFO, rewriting only files that changed.
//! let (written, removed) = ufo.sync(Path::new("Font.ufo"))?;
//! # Ok(())
//! # }
//! ```
//!
//! [`Ufo::to_files`] returns every file of the package without touching the
//! disk.
//!
//! The other modules hold the extraction steps, named after the extractor
//! functions they port, and the formatting helpers that reproduce Python's
//! output exactly. Fonts that would need behavior this crate does not
//! implement, such as CFF outlines or legacy name encodings, make
//! [`extract`] return an error instead of producing different output; the
//! README lists them.

#![warn(missing_docs)]

pub mod agl;
pub mod cmap;
pub mod codecs;
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

/// Print how long each stage took when TTF2UFO_TIMING is set.
pub(crate) struct Timer {
    start: std::time::Instant,
    enabled: bool,
}

impl Timer {
    pub(crate) fn new() -> Self {
        Timer {
            start: std::time::Instant::now(),
            enabled: std::env::var_os("TTF2UFO_TIMING").is_some(),
        }
    }

    pub(crate) fn lap(&self, what: &str) {
        if self.enabled {
            eprintln!("{:>8.3}s {what}", self.start.elapsed().as_secs_f64());
        }
    }
}

/// Extract a UFO from TrueType font data, as `extractufo` does.
pub fn extract(data: &[u8]) -> Result<Ufo, Error> {
    let timer = Timer::new();
    let font = FontRef::new(data)?;
    if font.table_data(Tag::new(b"glyf")).is_none() {
        return Err("only TrueType-flavored fonts (with a 'glyf' table) are supported".into());
    }
    let names = names::glyph_order(&font)?;
    timer.lap("glyph order");

    // extractOpenTypeInfo
    let (info, mut lib) = info::extract_info(&font)?;
    timer.lap("info");

    // extractOpenTypeGlyphs + extractUnicodeVariationSequences
    let char_map = cmap::char_map(&font, names.len())?;
    timer.lap("cmap");
    let mut glyphs = glyphs::extract_glyphs(&font, &names, &char_map.unicodes)?;
    timer.lap("glyphs");
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
                let name = names
                    .get(gid as usize)
                    .cloned()
                    .unwrap_or_else(|| format!("glyph{gid:05}"));
                map.insert(format!("{base:04X}"), name.into());
            }
            sequences.insert(format!("{selector:04X}"), Value::Dict(map));
        }
        lib.insert(
            "public.unicodeVariationSequences".into(),
            Value::Dict(sequences),
        );
    }

    // extractGlyphOrder
    if !names.is_empty() {
        lib.insert("public.glyphOrder".into(), names.clone().into());
    }

    // extractOpenTypeKerning
    let (kerning, groups) = layout::extract_kerning(&font, &names)?;
    timer.lap("kerning");

    // extractOpenTypeFeatures
    let features = features::extract_features(&font)?;
    timer.lap("features");

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
            .map(|(i, c)| {
                (
                    i.to_string(),
                    Value::Int(i16::from_be_bytes([c[0], c[1]]) as i64),
                )
            })
            .collect();
        tt.insert("controlValue".into(), Value::Dict(values));
    }
    if let Some(fpgm) = font.table_data(Tag::new(b"fpgm")) {
        tt.insert(
            "fontProgram".into(),
            instructions::lib_assembly(fpgm.as_bytes())?.into(),
        );
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
        tt.insert(
            key.into(),
            Value::Int(value.ok_or("'maxp' version 0.5 is not supported")? as i64),
        );
    }
    if let Some(prep) = font.table_data(Tag::new(b"prep")) {
        tt.insert(
            "controlValueProgram".into(),
            instructions::lib_assembly(prep.as_bytes())?.into(),
        );
    }
    lib.insert("public.truetype.instructions".into(), Value::Dict(tt));

    // extractAnchors
    let index: std::collections::HashMap<&str, usize> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    for (glyph, anchors) in layout::extract_anchors(&font, &names)? {
        let gid = *index
            .get(glyph.as_str())
            .ok_or_else(|| format!("anchor for missing glyph {glyph}"))?;
        glyphs[gid].anchors.extend(anchors);
    }

    timer.lap("instructions, anchors");
    Ok(Ufo {
        info,
        lib,
        groups,
        kerning,
        features,
        glyphs,
    })
}
