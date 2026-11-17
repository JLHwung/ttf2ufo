//! GPOS-derived data: kerning and groups (extractor's
//! _extractOpenTypeKerningFromGPOS / _extractOpenTypeKerningFromKern) and mark
//! anchors (extractAnchors).

use std::collections::{BTreeSet, HashMap, HashSet};

use indexmap::{IndexMap, IndexSet};
use read_fonts::tables::gpos::{
    AnchorTable, Gpos, MarkBasePos, MarkMarkPos, PairPos, PositionLookup, PositionSubtables,
};
use read_fonts::types::Tag;
use read_fonts::{FontRef, TableProvider};

use crate::Error;
use crate::names::GlyphOrder;
use crate::otl::{MarkAttachment, class_defs, coverage_glyphs, tag_str};
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
        // the default LangSys, then each LangSys, in order
        let mut lang_systems = Vec::new();
        if let Some(default) = script.default_lang_sys() {
            lang_systems.push(default?);
        }
        for lang in script.lang_sys_records() {
            lang_systems.push(lang.lang_sys(script.offset_data())?);
        }
        let features = lang_systems.iter().flat_map(|ls| {
            ls.feature_indices()
                .iter()
                .map(|i| i.get() as usize)
                .filter(|i| desired.contains(i))
        });
        let mut lookups = Vec::new();
        for f in features {
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

/// The lookups of the given features, script by script in `_makeScriptOrder`
/// order, each lookup only the first time it is seen.
fn lookups_by_script(gpos: &Gpos, tags: &[&str]) -> Result<Vec<Vec<usize>>, Error> {
    let lookup_indexes = gather_lookup_indexes(gpos, tags)?;
    let mut seen = HashSet::new();
    Ok(script_order(gpos)?
        .iter()
        .map(|script| {
            lookup_indexes
                .get(script)
                .into_iter()
                .flatten()
                .copied()
                .filter(|&index| seen.insert(index))
                .collect()
        })
        .collect())
}

/// `gpos.LookupList.Lookup[index]`. extractor only reaches the LookupList
/// through a referenced lookup, so a null LookupList is fine as long as no
/// wanted feature uses one.
fn gpos_lookup<'a>(gpos: &Gpos<'a>, index: usize) -> Result<PositionLookup<'a>, Error> {
    Ok(gpos.lookup_list()?.lookups().get(index)?)
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

fn x_advance(record: &read_fonts::tables::gpos::ValueRecord) -> Option<i64> {
    record.x_advance().map(|v| v as i64)
}

/// `_gatherKerningForLookup`
#[allow(clippy::type_complexity)]
fn kerning_for_lookup(
    gpos: &Gpos,
    index: usize,
    names: &GlyphOrder,
) -> Result<
    Option<(
        Pairs,
        IndexMap<ClassId, Members>,
        IndexMap<ClassId, Members>,
    )>,
    Error,
> {
    let lookup = gpos_lookup(gpos, index)?;
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
                let coverage = coverage_glyphs(names, &t.coverage()?)?;
                let use_first = !t.value_format1().is_empty();
                if t.pair_sets().len() < coverage.len() {
                    // `pairSets[index]` for every coverage glyph: IndexError
                    return Err("PairPos format 1 has fewer PairSets than coverage glyphs".into());
                }
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
                        if let Some(v) = x_advance(value) {
                            let right_name = names.name(record.second_glyph().to_u32());
                            kerning.insert(
                                (Side::Glyph(left_name.clone()), Side::Glyph(right_name)),
                                v,
                            );
                        }
                    }
                }
            }
            PairPos::Format2(t) => {
                let coverage = coverage_glyphs(names, &t.coverage()?)?;
                let l = feature_classes(
                    index,
                    si,
                    &class_defs(names, &t.class_def1()?)?,
                    Some(&coverage),
                );
                let r = feature_classes(index, si, &class_defs(names, &t.class_def2()?)?, None);
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
                        if let Some(v) = x_advance(value).filter(|v| *v != 0) {
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
pub fn extract_kerning(font: &FontRef, names: &GlyphOrder) -> Result<(Dict, Dict), Error> {
    let mut kerning = Pairs::new();
    let mut groups = Dict::new();
    if let Ok(gpos) = font.gpos() {
        let mut kerning_dicts: Vec<Vec<Pairs>> = Vec::new();
        let mut left_dicts: Vec<Vec<IndexMap<ClassId, Members>>> = Vec::new();
        let mut right_dicts: Vec<Vec<IndexMap<ClassId, Members>>> = Vec::new();
        for lookups in lookups_by_script(&gpos, &["kern"])? {
            let (mut k, mut l, mut r) = (Vec::new(), Vec::new(), Vec::new());
            for index in lookups {
                if let Some((pairs, lc, rc)) = kerning_for_lookup(&gpos, index, names)? {
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
        kerning = kern_table(data.as_bytes(), names)?;
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

/// `_extractOpenTypeKerningFromKern`, over fontTools' `kern` decompile: the
/// OpenType (version 0) and Apple (version 1.0) layouts. Every subtable must
/// be format 0: other formats have no `version`, so extractor fails on them.
fn kern_table(d: &[u8], names: &GlyphOrder) -> Result<Pairs, Error> {
    fn be(d: &[u8], at: usize, n: usize) -> Result<u64, Error> {
        let bytes = d.get(at..at + n).ok_or("bad 'kern' table")?;
        Ok(bytes.iter().fold(0u64, |v, b| v << 8 | *b as u64))
    }
    let version = be(d, 0, 2)?;
    // `self.version == 1.0` picks the Apple subtable header; `apple` is what
    // fontTools passes on to the format 0 subtable.
    let (apple, apple_header, n_tables, mut data) = if d.len() >= 8 && version == 1 {
        let v = be(d, 0, 4)?;
        (true, v == 0x0001_0000, be(d, 4, 4)?, &d[8..])
    } else {
        (false, version == 1, be(d, 2, 2)?, &d[4..])
    };
    let mut pairs = Pairs::new();
    for _ in 0..n_tables {
        let (mut length, format) = if apple_header {
            (be(data, 0, 4)? as usize, be(data, 5, 1)?)
        } else {
            let length = be(data, 2, 2)? as usize;
            let format = be(data, 4, 1)?;
            if n_tables == 1 && format == 0 {
                // fontTools ignores the length of a lone format 0 subtable
                (be(data, 6, 2)? as usize * 6 + 14, format)
            } else {
                (length, format)
            }
        };
        if format != 0 {
            return Err(format!("'kern' subtable format {format} has no version").into());
        }
        length = length.min(data.len());
        let sub = &data[..length];
        let body = if apple {
            &sub[8.min(sub.len())..]
        } else {
            if be(sub, 0, 2)? != 0 {
                return Err(
                    format!("unsupported kern subtable version: {}", be(sub, 0, 2)?).into(),
                );
            }
            &sub[6.min(sub.len())..]
        };
        let n = be(body, 0, 2)? as usize;
        for i in 0..n {
            let p = 8 + 6 * i;
            let value = be(body, p + 4, 2)? as u16 as i16 as i64;
            pairs.insert(
                (
                    Side::Glyph(names.name(be(body, p, 2)? as u32)),
                    Side::Glyph(names.name(be(body, p + 2, 2)? as u32)),
                ),
                value,
            );
        }
        data = &data[length..];
    }
    Ok(pairs)
}

fn anchor_xy(anchor: &AnchorTable) -> (i64, i64) {
    (anchor.x_coordinate() as i64, anchor.y_coordinate() as i64)
}

/// Per anchor group: (base glyph -> position, mark glyph -> position).
type AnchorGroup = (IndexMap<String, (i64, i64)>, IndexMap<String, (i64, i64)>);

fn grow(groups: &mut Vec<AnchorGroup>, len: usize) {
    while groups.len() < len {
        groups.push((IndexMap::new(), IndexMap::new()));
    }
}

/// The anchor groups of one mark-to-base or mark-to-mark subtable, one per
/// base anchor index and mark class.
fn anchor_groups(t: &MarkAttachment, names: &GlyphOrder) -> Result<Vec<AnchorGroup>, Error> {
    let base_cov = coverage_glyphs(names, &t.base_coverage)?;
    let mark_cov = coverage_glyphs(names, &t.mark_coverage)?;
    let mut groups: Vec<AnchorGroup> = Vec::new();
    for (ri, row) in t.base_anchors.iter().enumerate() {
        for (ai, anchor) in row.iter().enumerate() {
            grow(&mut groups, ai + 1);
            if let Some(anchor) = anchor {
                let glyph = base_cov.get(ri).ok_or("base coverage too short")?;
                groups[ai]
                    .0
                    .insert(glyph.clone(), anchor_xy(&anchor.clone()?));
            }
        }
    }
    for (mi, record) in t.marks.mark_records().iter().enumerate() {
        let class = record.mark_class() as usize;
        grow(&mut groups, class + 1);
        let anchor = record.mark_anchor(t.marks.offset_data())?;
        if let AnchorTable::Format1(_) = anchor {
            let glyph = mark_cov.get(mi).ok_or("mark coverage too short")?;
            groups[class].1.insert(glyph.clone(), anchor_xy(&anchor));
        } else {
            eprintln!("  Unexpected markAnchor format.");
        }
    }
    Ok(groups)
}

/// `extractAnchors`: glyph name -> anchors to append, in order.
pub fn extract_anchors(
    font: &FontRef,
    names: &GlyphOrder,
) -> Result<IndexMap<String, Vec<Anchor>>, Error> {
    let mut out: IndexMap<String, Vec<Anchor>> = IndexMap::new();
    let Ok(gpos) = font.gpos() else {
        return Ok(out);
    };
    let mut all: Vec<AnchorGroup> = Vec::new();
    for index in lookups_by_script(&gpos, &["mark", "mkmk"])?
        .into_iter()
        .flatten()
    {
        let lookup = gpos_lookup(&gpos, index)?;
        if !matches!(lookup.lookup_type(), 4 | 6 | 9) {
            continue;
        }
        let subtables = match lookup.subtables()? {
            PositionSubtables::MarkToBase(subtables) => subtables
                .iter()
                .map(|t| match t? {
                    MarkBasePos::Format1(t) => Ok(Some(MarkAttachment::mark_to_base(&t)?)),
                    _ => Ok(None),
                })
                .collect::<Result<Vec<_>, Error>>()?,
            PositionSubtables::MarkToMark(subtables) => subtables
                .iter()
                .map(|t| match t? {
                    MarkMarkPos::Format1(t) => Ok(Some(MarkAttachment::mark_to_mark(&t)?)),
                    _ => Ok(None),
                })
                .collect::<Result<Vec<_>, Error>>()?,
            _ => Vec::new(),
        };
        for subtable in subtables {
            match subtable {
                Some(t) => all.extend(anchor_groups(&t, names)?),
                None => eprintln!("  Skipping Anchor lookup subtable of unknown format."),
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
