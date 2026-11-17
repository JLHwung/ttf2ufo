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
pub mod otl;
pub mod plist;
pub mod pyfmt;
pub mod ufo;

/// Error type used throughout the crate.
pub type Error = Box<dyn std::error::Error + Send + Sync>;

use read_fonts::FontRef;
use read_fonts::types::Tag;

use plist::Value;
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
    if let Some(sequences) = char_map.uvs_lib(&names)? {
        lib.insert("public.unicodeVariationSequences".into(), sequences);
    }

    // extractGlyphOrder
    if !names.is_empty() {
        lib.insert("public.glyphOrder".into(), names.names().to_vec().into());
    }

    // extractOpenTypeKerning
    let (kerning, groups) = layout::extract_kerning(&font, &names)?;
    timer.lap("kerning");

    // extractOpenTypeFeatures
    let features = features::extract_features(&font, &names)?;
    timer.lap("features");

    // extractInstructions (glyph programs are handled with the glyphs)
    lib.insert(
        "public.truetype.instructions".into(),
        Value::Dict(instructions::font_lib(&font)?),
    );

    // extractAnchors
    for (glyph, anchors) in layout::extract_anchors(&font, &names)? {
        let gid = names
            .gid(&glyph)
            .ok_or_else(|| format!("anchor for missing glyph {glyph}"))?;
        glyphs[gid as usize].anchors.extend(anchors);
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
