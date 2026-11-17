//! ttf2ufo: extract a UFO from a TrueType font, like ufo-extractor's `extractufo`.

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: ttf2ufo [--update] FONT.ttf [OUT.ufo]

Writes OUT.ufo (default: FONT.ttf.ufo, like extractufo). Without --update the
output must not exist. With --update an existing UFO is updated in place:
only files whose content changed are rewritten, and files the font no longer
produces are removed.";

fn main() -> ExitCode {
    let mut update = false;
    let mut paths = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--update" => update = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    let (font, out) = match paths.as_slice() {
        [font] => (font.clone(), PathBuf::from(format!("{}.ufo", font.display()))),
        [font, out] => (font.clone(), out.clone()),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let result = std::fs::read(&font)
        .map_err(ttf2ufo::Error::from)
        .and_then(|data| ttf2ufo::extract(&data))
        .and_then(|ufo| {
            if update {
                let (written, removed) = ufo.sync(&out)?;
                println!("{}: {} glyphs, {written} files written, {removed} removed", out.display(), ufo.glyphs.len());
            } else {
                ufo.save(&out)?;
                println!("{}: {} glyphs", out.display(), ufo.glyphs.len());
            }
            Ok(())
        });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ttf2ufo: {}: {e}", font.display());
            ExitCode::FAILURE
        }
    }
}
