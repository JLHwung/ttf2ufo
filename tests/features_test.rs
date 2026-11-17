//! features.fea must match ufo-extractor's (fontFeatures `unparse(font).asFea()`)
//! byte for byte.

use std::path::PathBuf;

use read_fonts::FontRef;
use ttf2ufo::features::extract_features;

fn data(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(path)
}

/// Panics with the first differing line and some context around it.
fn assert_same(name: &str, got: &str, expected: &str) {
    if got == expected {
        return;
    }
    let got_lines: Vec<&str> = got.split('\n').collect();
    let exp_lines: Vec<&str> = expected.split('\n').collect();
    let n = got_lines.len().max(exp_lines.len());
    let first = (0..n)
        .find(|&i| got_lines.get(i) != exp_lines.get(i))
        .unwrap_or(n);
    let lo = first.saturating_sub(5);
    let hi = (first + 6).min(n);
    let show = |lines: &[&str]| {
        (lo..hi)
            .map(|i| {
                let mark = if i == first { ">" } else { " " };
                let line = lines.get(i).map_or("<EOF>".to_string(), |l| {
                    if l.len() > 300 {
                        format!(
                            "{}...",
                            &l[..l.char_indices().nth(300).map_or(l.len(), |c| c.0)]
                        )
                    } else {
                        l.to_string()
                    }
                });
                format!("{mark}{:6} {line}", i + 1)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    panic!(
        "{name}: features.fea differs at line {} ({} vs {} bytes)\n--- expected\n{}\n--- got\n{}",
        first + 1,
        expected.len(),
        got.len(),
        show(&exp_lines),
        show(&got_lines)
    );
}

fn check(font: &str) {
    let bytes = std::fs::read(data(&format!("fonts/{font}.ttf"))).unwrap();
    let font_ref = FontRef::new(&bytes).unwrap();
    let got = extract_features(&font_ref).unwrap();
    let expected =
        std::fs::read_to_string(data(&format!("expected/{font}.ufo/features.fea"))).unwrap();
    assert_same(font, &got, &expected);
}

#[test]
fn uvs_test() {
    check("UVSTest");
}

#[test]
fn plex_serif_fl() {
    check("IBM Plex Serif-Text-FL");
}

#[test]
fn plex_serif_ufo2ft() {
    check("IBM Plex Serif-Text-ufo2ft");
}
