//! Port of ufo-extractor's tests/extractor_test.py.

use std::collections::{BTreeMap, BTreeSet};

use ttf2ufo::plist::Value;

fn getpath(filename: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/fonts").join(filename)
}

#[test]
fn test_extract_cmap_with_uvs() {
    let ufo = ttf2ufo::extract(&std::fs::read(getpath("UVSTest.ttf")).unwrap()).unwrap();

    let unicodes: BTreeMap<&str, BTreeSet<u32>> = ufo
        .glyphs
        .iter()
        .filter(|g| !g.unicodes.is_empty())
        .map(|g| (g.name.as_str(), g.unicodes.iter().copied().collect()))
        .collect();
    assert_eq!(
        unicodes,
        BTreeMap::from([("zero", BTreeSet::from([0x030])), ("Anegativesquared", BTreeSet::from([0x1F170]))])
    );

    let dict = |pairs: &[(&str, &str)]| {
        Value::Dict(pairs.iter().map(|(k, v)| (k.to_string(), Value::from(*v))).collect())
    };
    let expected = Value::Dict(
        [
            ("FE00".to_string(), dict(&[("0030", "zero.slash")])),
            ("FE0E".to_string(), dict(&[("1F170", "Anegativesquared.text")])),
            ("FE0F".to_string(), dict(&[("1F170", "Anegativesquared")])),
        ]
        .into_iter()
        .collect(),
    );
    assert_eq!(ufo.lib.get("public.unicodeVariationSequences"), Some(&expected));
}
