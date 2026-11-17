//! GPOS-derived data: kerning and groups (extractor's
//! _extractOpenTypeKerningFromGPOS / _extractOpenTypeKerningFromKern) and mark
//! anchors (extractAnchors).

use std::collections::{BTreeSet, HashMap};

use indexmap::{IndexMap, IndexSet};
use read_fonts::tables::gpos::{AnchorTable, Gpos, PairPos, PositionSubtables};
use read_fonts::tables::layout::ClassDef;
use read_fonts::types::Tag;
use read_fonts::{FontRef, TableProvider};

use crate::Error;
use crate::plist::{Dict, Value};
use crate::pyfmt::{py_set_order, tuple_hash};
use crate::ufo::Anchor;

/// (lookup index, subtable index, class index)
type ClassId = (usize, usize, usize);
type Members = BTreeSet<String>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Side {
    Glyph(String),
    Class(ClassId),
    Name(String),
}

type Pairs = IndexMap<(Side, Side), i64>;

struct Names<'a>(&'a [String]);

impl Names<'_> {
    /// fontTools' getGlyphName: out-of-range ids get "glyph%05d".
    fn get(&self, gid: u32) -> String {
        self.0
            .get(gid as usize)
            .cloned()
            .unwrap_or_else(|| format!("glyph{gid:05}"))
    }
}

fn tag_str(tag: Tag) -> String {
    String::from_utf8_lossy(&tag.to_be_bytes()).into_owned()
}

/// `_makeScriptOrder`: script tags, sorted.
fn script_order(gpos: &Gpos) -> Result<Vec<String>, Error> {
    let mut scripts: Vec<String> = gpos
        .script_list()?
        .script_records()
        .iter()
        .map(|r| tag_str(r.script_tag()))
        .collect();
    scripts.sort();
    Ok(scripts)
}

/// `_gatherLookupIndexes`: script -> lookup indexes of the given features.
fn gather_lookup_indexes(gpos: &Gpos, tags: &[&str]) -> Result<HashMap<String, Vec<usize>>, Error> {
    let feature_list = gpos.feature_list()?;
    let records = feature_list.feature_records();
    let desired: Vec<usize> = records
        .iter()
        .enumerate()
        .filter(|(_, r)| tags.contains(&tag_str(r.feature_tag()).as_str()))
        .map(|(i, _)| i)
        .collect();
    let script_list = gpos.script_list()?;
    let mut result = HashMap::new();
    for record in script_list.script_records() {
        let script = record.script(script_list.offset_data())?;
        let mut feature_sets: Vec<Vec<usize>> = Vec::new();
        if let Some(default) = script.default_lang_sys() {
            let f: Vec<usize> = default?
                .feature_indices()
                .iter()
                .map(|i| i.get() as usize)
                .filter(|i| desired.contains(i))
                .collect();
            if !f.is_empty() {
                feature_sets.push(f);
            }
        }
        for lang in script.lang_sys_records() {
            let lang_sys = lang.lang_sys(script.offset_data())?;
            let f: Vec<usize> = lang_sys
                .feature_indices()
                .iter()
                .map(|i| i.get() as usize)
                .filter(|i| desired.contains(i))
                .collect();
            if !f.is_empty() {
                feature_sets.push(f);
            }
        }
        let mut lookups = Vec::new();
        for f in feature_sets.into_iter().flatten() {
            let feature = records[f].feature(feature_list.offset_data())?;
            for l in feature.lookup_list_indices() {
                let l = l.get() as usize;
                if !lookups.contains(&l) {
                    lookups.push(l);
                }
            }
        }
        result.insert(tag_str(record.script_tag()), lookups);
    }
    Ok(result)
}

/// fontTools' ClassDef.classDefs: glyph -> class, class 0 omitted.
fn class_defs(class_def: &ClassDef, names: &Names) -> IndexMap<String, u16> {
    #[allow(clippy::unnecessary_cast)]
    let mut defs = IndexMap::new();
    for (gid, cls) in class_def.iter() {
        if cls != 0 {
            defs.insert(names.get(gid.to_u32()), cls as u16);
        }
    }
    defs
}

/// `_extractFeatureClasses`
fn feature_classes(
    lookup: usize,
    subtable: usize,
    defs: &IndexMap<String, u16>,
    coverage: Option<&[String]>,
) -> IndexMap<ClassId, Members> {
    let mut class_dict: IndexMap<u16, Members> = IndexMap::new();
    for (glyph, cls) in defs {
        class_dict.entry(*cls).or_default().insert(glyph.clone());
    }
    let mut class0 = Members::new();
    if let Some(coverage) = coverage {
        if let Some(existing) = class_dict.get(&0) {
            class0 = existing
                .iter()
                .filter(|g| coverage.contains(g))
                .cloned()
                .collect();
        } else {
            class0 = coverage.iter().cloned().collect();
            for members in class_dict.values() {
                class0.retain(|g| !members.contains(g));
            }
        }
    }
    class_dict.insert(0, class0);
    class_dict
        .into_iter()
        .map(|(cls, m)| ((lookup, subtable, cls as usize), m))
        .collect()
}

fn x_advance(record: &read_fonts::tables::gpos::ValueRecord, format_nonzero: bool) -> Option<i64> {
    let _ = format_nonzero;
    record.x_advance().map(|v| v as i64)
}

/// `_gatherKerningForLookup`
#[allow(clippy::type_complexity)]
fn kerning_for_lookup(
    gpos: &Gpos,
    index: usize,
    names: &Names,
) -> Result<
    Option<(
        Pairs,
        IndexMap<ClassId, Members>,
        IndexMap<ClassId, Members>,
    )>,
    Error,
> {
    let lookup = gpos.lookup_list()?.lookups().get(index)?;
    if !matches!(lookup.lookup_type(), 2 | 9) {
        return Ok(None);
    }
    let mut kerning = Pairs::new();
    let mut left = IndexMap::new();
    let mut right = IndexMap::new();
    let PositionSubtables::Pair(subtables) = lookup.subtables()? else {
        return Ok(Some((kerning, left, right)));
    };
    for (si, subtable) in subtables.iter().enumerate() {
        match subtable? {
            PairPos::Format1(t) => {
                let coverage: Vec<String> = t
                    .coverage()?
                    .iter()
                    .map(|g| names.get(g.to_u32()))
                    .collect();
                let use_first = !t.value_format1().is_empty();
                for (i, pair_set) in t.pair_sets().iter().enumerate() {
                    let Some(left_name) = coverage.get(i) else {
                        break;
                    };
                    for record in pair_set?.pair_value_records().iter() {
                        let record = record?;
                        let value = if use_first {
                            record.value_record1()
                        } else {
                            record.value_record2()
                        };
                        if let Some(v) = x_advance(value, use_first) {
                            let right_name = names.get(record.second_glyph().to_u32());
                            kerning.insert(
                                (Side::Glyph(left_name.clone()), Side::Glyph(right_name)),
                                v,
                            );
                        }
                    }
                }
            }
            PairPos::Format2(t) => {
                let coverage: Vec<String> = t
                    .coverage()?
                    .iter()
                    .map(|g| names.get(g.to_u32()))
                    .collect();
                let l = feature_classes(
                    index,
                    si,
                    &class_defs(&t.class_def1()?, names),
                    Some(&coverage),
                );
                let r = feature_classes(index, si, &class_defs(&t.class_def2()?, names), None);
                let use_first = !t.value_format1().is_empty();
                for (c1, class1) in t.class1_records().iter().enumerate() {
                    let class1 = class1?;
                    for (c2, class2) in class1.class2_records().iter().enumerate() {
                        let class2 = class2?;
                        let value = if use_first {
                            class2.value_record1()
                        } else {
                            class2.value_record2()
                        };
                        if let Some(v) = x_advance(value, use_first).filter(|v| *v != 0) {
                            kerning.insert(
                                (Side::Class((index, si, c1)), Side::Class((index, si, c2))),
                                v,
                            );
                        }
                    }
                }
                left.extend(l);
                right.extend(r);
            }
            // extractor only handles pair positioning formats 1 and 2
            _ => {}
        }
    }
    Ok(Some((kerning, left, right)))
}

/// `_findSingleMemberGroups`: removes one-glyph classes, returning them.
fn find_single_member_groups(
    groups: &mut [Vec<IndexMap<ClassId, Members>>],
) -> HashMap<ClassId, String> {
    let mut singles = HashMap::new();
    for group in groups.iter_mut() {
        for dict in group.iter_mut() {
            dict.retain(|id, members| {
                if members.len() == 1 {
                    singles.insert(*id, members.iter().next().unwrap().clone());
                    false
                } else {
                    true
                }
            });
        }
    }
    singles
}

fn rename_pairs(
    kerning: Pairs,
    left: &HashMap<ClassId, Side>,
    right: &HashMap<ClassId, Side>,
) -> Pairs {
    let map = |side: Side, rename: &HashMap<ClassId, Side>| match side {
        Side::Class(id) => rename.get(&id).cloned().unwrap_or(Side::Class(id)),
        other => other,
    };
    let mut out = Pairs::new();
    for ((l, r), v) in kerning {
        out.insert((map(l, left), map(r, right)), v);
    }
    out
}

/// `_mergeClasses`: classes in first-seen order, and the rename map of
/// classes merged into an identical one.
fn merge_classes(
    groups: &[Vec<IndexMap<ClassId, Members>>],
) -> (IndexMap<ClassId, Members>, HashMap<ClassId, Side>) {
    let mut tree: IndexMap<Members, Vec<ClassId>> = IndexMap::new();
    for group in groups {
        for dict in group {
            for (id, members) in dict {
                let ids = tree.entry(members.clone()).or_default();
                if !ids.contains(id) {
                    ids.push(*id);
                }
            }
        }
    }
    let mut classes = IndexMap::new();
    let mut rename = HashMap::new();
    for (members, ids) in tree {
        // names.pop() on a Python set of tuples
        let order = py_set_order(
            ids.iter()
                .map(|id| (tuple_hash(&[id.0 as u64, id.1 as u64, id.2 as u64]), *id)),
        );
        let keep = order[0];
        for other in &order[1..] {
            rename.insert(*other, Side::Class(keep));
        }
        classes.insert(keep, members);
    }
    (classes, rename)
}

/// `_renameClasses`
fn rename_classes(classes: &IndexMap<ClassId, Members>, prefix: &str) -> HashMap<ClassId, Side> {
    let mut rename: HashMap<ClassId, Side> = HashMap::new();
    let mut used: IndexSet<String> = IndexSet::new();
    for (id, glyphs) in classes {
        let name = match glyphs.len() {
            0 => format!("{prefix}_empty_lu.{}_st.{}_cl.{}", id.0, id.1, id.2),
            1 => glyphs.iter().next().unwrap().clone(),
            _ => format!("{prefix}{}", glyphs.iter().next().unwrap()),
        };
        if used.insert(name.clone()) {
            rename.insert(*id, Side::Name(name));
        } else {
            eprintln!("  Dropping duplicate group rename target:\n    {id:?} -> {name}");
        }
    }
    rename
}

fn side_name(side: &Side) -> Result<String, Error> {
    match side {
        Side::Glyph(n) | Side::Name(n) => Ok(n.clone()),
        Side::Class(id) => Err(format!("kerning class {id:?} could not be named").into()),
    }
}

/// (kerning.plist, groups.plist) contents.
pub fn extract_kerning(font: &FontRef, glyph_names: &[String]) -> Result<(Dict, Dict), Error> {
    let names = Names(glyph_names);
    let mut kerning = Pairs::new();
    let mut groups = Dict::new();
    if let Ok(gpos) = font.gpos() {
        let lookup_indexes = gather_lookup_indexes(&gpos, &["kern"])?;
        let mut seen = std::collections::HashSet::new();
        let mut kerning_dicts: Vec<Vec<Pairs>> = Vec::new();
        let mut left_dicts: Vec<Vec<IndexMap<ClassId, Members>>> = Vec::new();
        let mut right_dicts: Vec<Vec<IndexMap<ClassId, Members>>> = Vec::new();
        for script in script_order(&gpos)? {
            let (mut k, mut l, mut r) = (Vec::new(), Vec::new(), Vec::new());
            for &index in lookup_indexes
                .get(&script)
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                if !seen.insert(index) {
                    continue;
                }
                if let Some((pairs, lc, rc)) = kerning_for_lookup(&gpos, index, &names)? {
                    k.push(pairs);
                    l.push(lc);
                    r.push(rc);
                }
            }
            if !k.is_empty() {
                kerning_dicts.push(k);
                left_dicts.push(l);
                right_dicts.push(r);
            }
        }
        for group in kerning_dicts.iter().rev() {
            for dict in group {
                for (key, value) in dict {
                    kerning.insert(key.clone(), *value);
                }
            }
        }
        let left_singles = find_single_member_groups(&mut left_dicts);
        let right_singles = find_single_member_groups(&mut right_dicts);
        let as_glyph = |m: HashMap<ClassId, String>| -> HashMap<ClassId, Side> {
            m.into_iter().map(|(k, v)| (k, Side::Glyph(v))).collect()
        };
        kerning = rename_pairs(kerning, &as_glyph(left_singles), &as_glyph(right_singles));
        let (left_classes, left_merge) = merge_classes(&left_dicts);
        let (right_classes, right_merge) = merge_classes(&right_dicts);
        for classes in [&left_classes, &right_classes] {
            let mut owner: HashMap<&String, usize> = HashMap::new();
            for members in classes.values() {
                for g in members {
                    *owner.entry(g).or_default() += 1;
                }
            }
            if owner.values().any(|n| *n > 1) {
                return Err("Kerning classes are in an conflicting state.".into());
            }
        }
        kerning = rename_pairs(kerning, &left_merge, &right_merge);
        let left_rename = rename_classes(&left_classes, "public.kern1.");
        let right_rename = rename_classes(&right_classes, "public.kern2.");
        kerning = rename_pairs(kerning, &left_rename, &right_rename);
        for (classes, rename) in [
            (&left_classes, &left_rename),
            (&right_classes, &right_rename),
        ] {
            for (id, glyphs) in classes {
                if glyphs.len() == 1 {
                    continue;
                }
                let name = side_name(&rename.get(id).cloned().unwrap_or(Side::Class(*id)))?;
                groups.insert(
                    name,
                    Value::Array(glyphs.iter().map(|g| g.clone().into()).collect()),
                );
            }
        }
    }
    if kerning.is_empty()
        && let Some(data) = font.table_data(Tag::new(b"kern"))
    {
        kerning = kern_table(data.as_bytes(), &names)?;
        groups = Dict::new();
    }

    let mut nested = Dict::new();
    for ((l, r), v) in kerning {
        let (l, r) = (side_name(&l)?, side_name(&r)?);
        let entry = nested.entry(l).or_insert_with(|| Value::Dict(Dict::new()));
        if let Value::Dict(d) = entry {
            d.insert(r, Value::Int(v));
        }
    }
    Ok((nested, groups))
}

/// `_extractOpenTypeKerningFromKern`: format 0 subtables of a version 0 'kern' table.
fn kern_table(d: &[u8], names: &Names) -> Result<Pairs, Error> {
    let u16_at = |at: usize| -> Result<u16, Error> {
        Ok(u16::from_be_bytes(
            d.get(at..at + 2).ok_or("bad 'kern' table")?.try_into()?,
        ))
    };
    if u16_at(0)? != 0 {
        return Err("only version 0 'kern' tables are supported".into());
    }
    let mut pairs = Pairs::new();
    let mut at = 4usize;
    for _ in 0..u16_at(2)? {
        let (version, length, coverage) = (u16_at(at)?, u16_at(at + 2)? as usize, u16_at(at + 4)?);
        if version != 0 || coverage >> 8 != 0 {
            return Err("Unknown kern table formst".into());
        }
        let n = u16_at(at + 6)? as usize;
        for i in 0..n {
            let p = at + 14 + 6 * i;
            let value = u16_at(p + 4)? as i16 as i64;
            pairs.insert(
                (
                    Side::Glyph(names.get(u16_at(p)? as u32)),
                    Side::Glyph(names.get(u16_at(p + 2)? as u32)),
                ),
                value,
            );
        }
        at += length;
    }
    Ok(pairs)
}

fn anchor_xy(anchor: &AnchorTable) -> (i64, i64) {
    match anchor {
        AnchorTable::Format1(a) => (a.x_coordinate() as i64, a.y_coordinate() as i64),
        AnchorTable::Format2(a) => (a.x_coordinate() as i64, a.y_coordinate() as i64),
        AnchorTable::Format3(a) => (a.x_coordinate() as i64, a.y_coordinate() as i64),
    }
}

type AnchorGroup = (IndexMap<String, (i64, i64)>, IndexMap<String, (i64, i64)>);

fn grow(groups: &mut Vec<AnchorGroup>, len: usize) {
    while groups.len() < len {
        groups.push((IndexMap::new(), IndexMap::new()));
    }
}

/// `extractAnchors`: glyph name -> anchors to append, in order.
pub fn extract_anchors(
    font: &FontRef,
    glyph_names: &[String],
) -> Result<IndexMap<String, Vec<Anchor>>, Error> {
    let names = Names(glyph_names);
    let mut out: IndexMap<String, Vec<Anchor>> = IndexMap::new();
    let Ok(gpos) = font.gpos() else {
        return Ok(out);
    };
    let lookup_indexes = gather_lookup_indexes(&gpos, &["mark", "mkmk"])?;
    let lookups = gpos.lookup_list()?.lookups();
    let mut seen = std::collections::HashSet::new();
    let mut all: Vec<AnchorGroup> = Vec::new();
    for script in script_order(&gpos)? {
        for &index in lookup_indexes
            .get(&script)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            if !seen.insert(index) {
                continue;
            }
            let lookup = lookups.get(index)?;
            if !matches!(lookup.lookup_type(), 4 | 6 | 9) {
                continue;
            }
            match lookup.subtables()? {
                PositionSubtables::MarkToBase(subtables) => {
                    for subtable in subtables.iter() {
                        let read_fonts::tables::gpos::MarkBasePos::Format1(t) = subtable? else {
                            eprintln!("  Skipping Anchor lookup subtable of unknown format.");
                            continue;
                        };
                        let mut groups: Vec<AnchorGroup> = Vec::new();
                        let base_cov: Vec<String> = t
                            .base_coverage()?
                            .iter()
                            .map(|g| names.get(g.to_u32()))
                            .collect();
                        let mark_cov: Vec<String> = t
                            .mark_coverage()?
                            .iter()
                            .map(|g| names.get(g.to_u32()))
                            .collect();
                        let base_array = t.base_array()?;
                        for (ri, record) in base_array.base_records().iter().enumerate() {
                            let record = record?;
                            for (ai, anchor) in record
                                .base_anchors(base_array.offset_data())
                                .iter()
                                .enumerate()
                            {
                                grow(&mut groups, ai + 1);
                                if let Some(anchor) = anchor {
                                    groups[ai]
                                        .0
                                        .insert(base_cov[ri].clone(), anchor_xy(&anchor?));
                                }
                            }
                        }
                        let mark_array = t.mark_array()?;
                        for (mi, record) in mark_array.mark_records().iter().enumerate() {
                            let class = record.mark_class() as usize;
                            grow(&mut groups, class + 1);
                            let anchor = record.mark_anchor(mark_array.offset_data())?;
                            if let AnchorTable::Format1(_) = anchor {
                                groups[class]
                                    .1
                                    .insert(mark_cov[mi].clone(), anchor_xy(&anchor));
                            } else {
                                eprintln!("  Unexpected markAnchor format.");
                            }
                        }
                        all.extend(groups);
                    }
                }
                PositionSubtables::MarkToMark(subtables) => {
                    for subtable in subtables.iter() {
                        let read_fonts::tables::gpos::MarkMarkPos::Format1(t) = subtable? else {
                            eprintln!("  Skipping Anchor lookup subtable of unknown format.");
                            continue;
                        };
                        let mut groups: Vec<AnchorGroup> = Vec::new();
                        let base_cov: Vec<String> = t
                            .mark2_coverage()?
                            .iter()
                            .map(|g| names.get(g.to_u32()))
                            .collect();
                        let mark_cov: Vec<String> = t
                            .mark1_coverage()?
                            .iter()
                            .map(|g| names.get(g.to_u32()))
                            .collect();
                        let base_array = t.mark2_array()?;
                        for (ri, record) in base_array.mark2_records().iter().enumerate() {
                            let record = record?;
                            for (ai, anchor) in record
                                .mark2_anchors(base_array.offset_data())
                                .iter()
                                .enumerate()
                            {
                                grow(&mut groups, ai + 1);
                                if let Some(anchor) = anchor {
                                    groups[ai]
                                        .0
                                        .insert(base_cov[ri].clone(), anchor_xy(&anchor?));
                                }
                            }
                        }
                        let mark_array = t.mark1_array()?;
                        for (mi, record) in mark_array.mark_records().iter().enumerate() {
                            let class = record.mark_class() as usize;
                            grow(&mut groups, class + 1);
                            let anchor = record.mark_anchor(mark_array.offset_data())?;
                            if let AnchorTable::Format1(_) = anchor {
                                groups[class]
                                    .1
                                    .insert(mark_cov[mi].clone(), anchor_xy(&anchor));
                            } else {
                                eprintln!("  Unexpected markAnchor format.");
                            }
                        }
                        all.extend(groups);
                    }
                }
                _ => {}
            }
        }
    }
    for (gi, (bases, marks)) in all.iter().enumerate() {
        for (glyph, (x, y)) in bases {
            out.entry(glyph.clone()).or_default().push(Anchor {
                x: *x,
                y: *y,
                name: format!("Anchor-{gi}"),
            });
        }
        for (glyph, (x, y)) in marks {
            out.entry(glyph.clone()).or_default().push(Anchor {
                x: *x,
                y: *y,
                name: format!("_Anchor-{gi}"),
            });
        }
    }
    Ok(out)
}
