//! Whole-UFO comparison against ufo-extractor 0.8.1's output for the same fonts
//! (tests/data/expected/, produced by tests/data/regenerate.py).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name)
}

fn read_tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(&path).unwrap(),
                );
            }
        }
    }
    files
}

fn first_difference(actual: &[u8], expected: &[u8]) -> String {
    let a = String::from_utf8_lossy(actual);
    let e = String::from_utf8_lossy(expected);
    let (al, el): (Vec<&str>, Vec<&str>) = (a.lines().collect(), e.lines().collect());
    for i in 0..al.len().max(el.len()) {
        if al.get(i) != el.get(i) {
            return format!(
                "line {}:\n  expected: {:?}\n  actual:   {:?}",
                i + 1,
                el.get(i).unwrap_or(&"<eof>"),
                al.get(i).unwrap_or(&"<eof>")
            );
        }
    }
    "difference in line endings".into()
}

/// Compare every file; `skip` lists paths not compared.
fn check(font: &str, skip: &[&str]) {
    let bytes = std::fs::read(data(&format!("fonts/{font}.ttf"))).unwrap();
    let ufo = ttf2ufo::extract(&bytes).unwrap();
    let actual = ufo.to_files();
    let expected = read_tree(&data(&format!("expected/{font}.ufo")));
    let skip: Vec<PathBuf> = skip.iter().map(PathBuf::from).collect();

    let mut problems = Vec::new();
    for path in expected.keys().filter(|p| !skip.contains(p)) {
        if !actual.contains_key(path) {
            problems.push(format!("missing: {}", path.display()));
        }
    }
    for (path, content) in actual.iter().filter(|(p, _)| !skip.contains(p)) {
        match expected.get(path) {
            None => problems.push(format!("unexpected: {}", path.display())),
            Some(e) if e != content => problems.push(format!(
                "differs: {}\n{}",
                path.display(),
                first_difference(content, e)
            )),
            Some(_) => {}
        }
    }
    let total = problems.len();
    problems.truncate(10);
    assert!(
        total == 0,
        "{font}: {total} problems, first {}:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

const FEATURES: &[&str] = &["features.fea"];

#[test]
fn uvs_test() {
    check("UVSTest", FEATURES);
}

#[test]
fn ibm_plex_fl() {
    check("IBM Plex Serif-Text-FL", FEATURES);
}

#[test]
fn ibm_plex_ufo2ft() {
    check("IBM Plex Serif-Text-ufo2ft", FEATURES);
}
