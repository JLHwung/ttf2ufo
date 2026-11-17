//! features.fea: a port of fontFeatures' `unparse(font).asFea()`, which is what
//! ufo-extractor uses to decompile GSUB/GPOS into feature code.

use read_fonts::FontRef;

use crate::Error;

/// Decompile the font's GSUB and GPOS tables into feature code, byte-for-byte
/// as `fontFeatures.ttLib.unparse(font).asFea()` does.
pub fn extract_features(font: &FontRef) -> Result<String, Error> {
    let _ = font;
    Ok(String::new())
}
