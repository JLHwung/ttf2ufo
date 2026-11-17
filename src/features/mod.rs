//! features.fea: a port of fontFeatures' `unparse(font).asFea()`, which is what
//! ufo-extractor uses to decompile GSUB/GPOS into feature code.
//!
//! - `unparse`: `fontFeatures.ttLib` (binary lookups to routines and rules)
//! - `fea`: `FontFeatures.asFeaAST()` and the feaLib AST serialisation
//! - `model`: the fontFeatures objects both work on
//! - `tables`: fontTools' views of coverage, class definitions and GDEF

mod fea;
mod model;
mod tables;
mod unparse;

use read_fonts::types::Tag;
use read_fonts::{FontRef, TableProvider};

use crate::Error;
use model::FontFeatures;
use tables::{GdefInfo, GlyphNames, read_err};
use unparse::Unparser;

/// Decompile the font's GSUB and GPOS tables into feature code, byte-for-byte
/// as `fontFeatures.ttLib.unparse(font).asFea()` does.
pub fn extract_features(font: &FontRef) -> Result<String, Error> {
    let has_gsub = font.table_data(Tag::new(b"GSUB")).is_some();
    let has_gpos = font.table_data(Tag::new(b"GPOS")).is_some();
    let mut ff = FontFeatures::default();
    let names = GlyphNames {
        names: if has_gsub || has_gpos {
            crate::names::glyph_order(font)?
        } else {
            Vec::new()
        },
    };
    let gdef = GdefInfo::load(font, &names);
    // fontTools parses `Debg` lazily; a parse error only matters when read.
    let debg = font
        .table_data(Tag::new(b"Debg"))
        .map(|d| serde_json::from_slice(d.as_bytes()).map_err(|e| e.to_string()));
    if has_gsub {
        let gsub = font.gsub().map_err(read_err)?;
        Unparser::new(&mut ff, &names, &gdef, "GSUB", debg.as_ref()).unparse_gsub(&gsub)?;
    }
    if has_gpos {
        let gpos = font.gpos().map_err(read_err)?;
        Unparser::new(&mut ff, &names, &gdef, "GPOS", debg.as_ref()).unparse_gpos(&gpos)?;
    }
    fea::as_fea(&mut ff, &gdef)
}
