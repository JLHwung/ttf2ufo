//! Port of `fontFeatures.ttLib` (GTableUnparser, GSUBUnparser, GPOSUnparser):
//! binary GSUB/GPOS lookups to fontFeatures routines.

use std::collections::HashSet;

use indexmap::IndexMap;
use read_fonts::ReadError;
use read_fonts::tables::gpos::{
    AnchorTable, CursivePos, Gpos, MarkBasePos, MarkLigPos, MarkMarkPos, PairPos, PositionLookup,
    PositionSubtables, SinglePos,
};
use read_fonts::tables::gsub::{
    AlternateSubst, Gsub, LigatureSubst, MultipleSubst, ReverseChainSingleSubst, SingleSubst,
    SubstitutionLookup, SubstitutionSubtables,
};
use read_fonts::tables::layout::{
    ChainedSequenceContext, ClassDef, CoverageTable, FeatureList, LangSys, ScriptList,
    SequenceContext, SequenceLookupRecord,
};
use read_fonts::types::{BigEndian, GlyphId16};

use super::model::{
    Attachment, FeatRef, FontFeatures, Ref, Routine, Rule, RuleKind, Slot, ValueRecord,
};
use super::tables::{GdefInfo, invert_class_def};
use crate::Error;
use crate::names::GlyphOrder;
use crate::otl::{MarkAttachment, class_defs, coverage_gids, coverage_glyphs, tag_str};

/// Collects an iterator of read results, failing on the first error.
fn all<T>(items: impl Iterator<Item = Result<T, ReadError>>) -> Result<Vec<T>, ReadError> {
    items.collect()
}

fn single(g: String) -> Slot {
    vec![g]
}

fn anchor_xy(a: &AnchorTable) -> (i32, i32) {
    (a.x_coordinate() as i32, a.y_coordinate() as i32)
}

fn value_record(vr: &read_fonts::tables::gpos::ValueRecord) -> ValueRecord {
    let nz = |v: Option<i16>| v.filter(|&v| v != 0).map(|v| v as i32);
    ValueRecord {
        x_placement: nz(vr.x_placement()),
        y_placement: nz(vr.y_placement()),
        x_advance: nz(vr.x_advance()),
        y_advance: nz(vr.y_advance()),
    }
}

/// Python's `repr` of a list of ints.
fn py_int_list(v: &[u16]) -> String {
    let items: Vec<String> = v.iter().map(|i| i.to_string()).collect();
    format!("[{}]", items.join(", "))
}

pub struct Unparser<'a> {
    pub ff: &'a mut FontFeatures,
    pub names: &'a GlyphOrder,
    pub gdef: &'a GdefInfo,
    /// `_table`: "GSUB" or "GPOS".
    pub table: &'static str,
    /// The font's `Debg` table, parsed as JSON (or the parse error).
    debg: Option<&'a Result<serde_json::Value, String>>,
    index: usize,
    /// Arena indices of this table's routines, by lookup index.
    lookups: Vec<usize>,
    /// The lookup being unparsed: its index, flag, mark filtering set, and
    /// whether it is an extension lookup.
    current_lookup: usize,
    flag: u16,
    mark_filtering_set: Option<u16>,
    is_ext: bool,
}

impl<'a> Unparser<'a> {
    pub fn new(
        ff: &'a mut FontFeatures,
        names: &'a GlyphOrder,
        gdef: &'a GdefInfo,
        table: &'static str,
        debg: Option<&'a Result<serde_json::Value, String>>,
    ) -> Self {
        Unparser {
            ff,
            names,
            gdef,
            table,
            debg,
            index: 0,
            lookups: Vec::new(),
            current_lookup: 0,
            flag: 0,
            mark_filtering_set: None,
            is_ext: false,
        }
    }

    fn gensym(&mut self) -> String {
        self.index += 1;
        self.index.to_string()
    }

    fn name(&self, gid: u32) -> String {
        self.names.name(gid)
    }

    /// A routine named `prefix` plus the next symbol, with the current
    /// lookup's flags and mark sets.
    fn start(&mut self, prefix: &str) -> Result<Routine, Error> {
        let name = format!("{prefix}{}", self.gensym());
        self.routine(name)
    }

    /// `_fix_flags`: a routine with the current lookup's flags and mark sets.
    fn routine(&self, name: String) -> Result<Routine, Error> {
        let flag = self.flag;
        let mut r = Routine {
            name,
            flags: flag,
            ..Default::default()
        };
        let mat = flag & 0xFF00;
        if mat != 0 {
            let defs = self
                .gdef
                .mark_attach
                .as_ref()
                .map_err(|e| Error::from(e.clone()))?;
            let wanted = mat >> 8;
            r.mark_attachment_set = Some(
                defs.iter()
                    .filter(|(_, c)| **c == wanted)
                    .map(|(g, _)| g.clone())
                    .collect(),
            );
        }
        if flag & 0x10 != 0 {
            if let Some(idx) = self.mark_filtering_set {
                let sets = self
                    .gdef
                    .mark_glyph_sets
                    .as_ref()
                    .map_err(|e| Error::from(e.clone()))?;
                let set = sets
                    .get(idx as usize)
                    .ok_or("MarkFilteringSet index out of range")?;
                r.mark_filtering_set = Some(set.clone());
            }
            if r.mark_filtering_set.is_none() {
                return Err("lookup uses a mark filtering set but has none".into());
            }
        }
        Ok(r)
    }

    fn rule(&self, kind: RuleKind) -> Rule {
        Rule {
            kind,
            address: Some(self.current_lookup),
        }
    }

    /// A substitution rule without context.
    fn subst(&self, input: Vec<Slot>, replacement: Vec<Slot>, force_alt: bool) -> Rule {
        self.rule(RuleKind::Substitution {
            input,
            replacement,
            precontext: vec![],
            postcontext: vec![],
            reverse: false,
            force_alt,
        })
    }

    fn glyph_names(&self, gids: &[BigEndian<GlyphId16>]) -> Vec<String> {
        gids.iter().map(|g| self.name(g.get().to_u32())).collect()
    }

    fn names_of(&self, gids: &[BigEndian<GlyphId16>]) -> Vec<Slot> {
        self.glyph_names(gids).into_iter().map(single).collect()
    }

    /// One slot per coverage table.
    fn coverage_slots<'b>(
        &self,
        covs: impl Iterator<Item = Result<CoverageTable<'b>, ReadError>>,
    ) -> Result<Vec<Slot>, Error> {
        all(covs)?
            .iter()
            .map(|cov| coverage_glyphs(self.names, cov))
            .collect()
    }

    /// `_invertClassDef(classDef.classDefs)`
    fn classes(
        &self,
        cd: Result<ClassDef, ReadError>,
    ) -> Result<IndexMap<u16, Vec<String>>, Error> {
        Ok(invert_class_def(self.names, &class_defs(self.names, &cd?)?))
    }

    // ------------------------------------------------------------------
    // Table level
    // ------------------------------------------------------------------

    /// `GTableUnparser.unparse` for GSUB.
    pub fn unparse_gsub(&mut self, gsub: &Gsub) -> Result<(), Error> {
        let Some(script_list) = gsub.original_script_list() else {
            return Ok(());
        };
        let script_list = script_list?;
        if let Some(lookup_list) = gsub.original_lookup_list() {
            let lookup_list = lookup_list?;
            let lookups = all(lookup_list.lookups().iter())?;
            self.unparse_lookup_list(&lookups, Self::unparse_gsub_lookup)?;
        }
        self.collect_features(&script_list, gsub.original_feature_list())
    }

    /// `GTableUnparser.unparse` for GPOS.
    pub fn unparse_gpos(&mut self, gpos: &Gpos) -> Result<(), Error> {
        let Some(script_list) = gpos.original_script_list() else {
            return Ok(());
        };
        let script_list = script_list?;
        if let Some(lookup_list) = gpos.original_lookup_list() {
            let lookup_list = lookup_list?;
            let lookups = all(lookup_list.lookups().iter())?;
            self.unparse_lookup_list(&lookups, Self::unparse_gpos_lookup)?;
        }
        self.collect_features(&script_list, gpos.original_feature_list())
    }

    /// `unparseLookups`
    fn unparse_lookup_list<L>(
        &mut self,
        lookups: &[L],
        unparse: fn(&mut Self, &L) -> Result<Routine, Error>,
    ) -> Result<(), Error> {
        self.make_dummies(lookups.len());
        for (idx, lookup) in lookups.iter().enumerate() {
            self.current_lookup = idx;
            let mut routine = unparse(self, lookup)?;
            self.apply_debug_info(&mut routine, idx)?;
            self.copy_routine(routine, idx);
        }
        Ok(())
    }

    fn make_dummies(&mut self, n: usize) {
        for _ in 0..n {
            let id = self.ff.new_routine(Routine::default());
            self.lookups.push(id);
            self.ff.routines.push(id);
        }
    }

    /// The `getDebugInfo` part of `unparseLookups`: with feaLib debug data in
    /// a `Debg` table, a lookup gets its source address and original name.
    fn apply_debug_info(&self, routine: &mut Routine, idx: usize) -> Result<(), Error> {
        use serde_json::Value as J;
        const KEY: &str = "com.github.fonttools.feaLib";
        let Some(debg) = self.debg else {
            return Ok(());
        };
        let data = debg
            .as_ref()
            .map_err(|e| format!("bad 'Debg' table: {e}"))?;
        // `KEY in data`, then `data[KEY][table][str(ix)]`
        let present = match data {
            J::Object(m) => m.contains_key(KEY),
            J::Array(a) => a.iter().any(|v| v.as_str() == Some(KEY)),
            J::String(s) => s.contains(KEY),
            _ => true,
        };
        if !present {
            return Ok(());
        }
        let bad = || {
            Error::from(format!(
                "'Debg' has no usable feaLib entry for {} lookup {idx}",
                self.table
            ))
        };
        let entry = data
            .get(KEY)
            .and_then(|d| d.get(self.table))
            .and_then(|d| d.get(idx.to_string()))
            .and_then(J::as_array)
            .filter(|a| a.len() >= 2)
            .ok_or_else(bad)?;
        // `debug_data[1].replace("-", "_")`: only a string has `replace`
        let name = entry[1].as_str().ok_or_else(bad)?.replace('-', "_");
        let location = match &entry[0] {
            J::String(s) => s.clone(),
            J::Null => "None".to_string(),
            J::Bool(b) => if *b { "True" } else { "False" }.to_string(),
            J::Number(n) if n.is_i64() || n.is_u64() => n.to_string(),
            _ => return Err("unsupported 'Debg' source location".into()),
        };
        routine.address = Some(format!("{} {idx} {location} {name}", self.table));
        if !name.is_empty() {
            routine.name = name;
        }
        Ok(())
    }

    /// `_copyRoutineToRoutine`
    fn copy_routine(&mut self, src: Routine, idx: usize) {
        let dst = &mut self.ff.arena[self.lookups[idx]];
        let usecount = dst.usecount;
        *dst = Routine { usecount, ..src };
    }

    /// `collectFeatures` / `_prepareFeatureLangSys`
    fn collect_features(
        &mut self,
        script_list: &ScriptList,
        feature_list: Option<Result<FeatureList, ReadError>>,
    ) -> Result<(), Error> {
        let feature_list = feature_list.transpose()?;
        for record in script_list.script_records() {
            let script_tag = tag_str(record.script_tag());
            let script = record.script(script_list.offset_data())?;
            if let Some(dls) = script.default_lang_sys() {
                let dls = dls?;
                self.prepare_feature_lang_sys("dflt", &dls, feature_list.as_ref(), &script_tag)?;
            }
            for lsr in script.lang_sys_records() {
                let lang_tag = tag_str(lsr.lang_sys_tag());
                let ls = lsr.lang_sys(script.offset_data())?;
                self.prepare_feature_lang_sys(&lang_tag, &ls, feature_list.as_ref(), &script_tag)?;
            }
        }
        self.ff.resolve_all_routines();
        Ok(())
    }

    fn prepare_feature_lang_sys(
        &mut self,
        lang_tag: &str,
        lang_sys: &LangSys,
        feature_list: Option<&FeatureList>,
        script_tag: &str,
    ) -> Result<(), Error> {
        for fi in lang_sys.feature_indices() {
            let fi = fi.get() as usize;
            let feature_list = feature_list.ok_or("FeatureList is missing")?;
            let record = feature_list
                .feature_records()
                .get(fi)
                .ok_or("feature index out of range")?;
            let feature_tag = tag_str(record.feature_tag());
            let feature = record.feature(feature_list.offset_data())?;
            for li in feature.lookup_list_indices() {
                let li = li.get() as usize;
                let routine = *self.lookups.get(li).ok_or("lookup index out of range")?;
                let pair = (script_tag.to_string(), lang_tag.to_string());
                let langs = &mut self.ff.arena[routine].languages;
                if !langs.contains(&pair) {
                    langs.push(pair);
                }
                let refs = self.ff.feature_mut(&feature_tag);
                if !refs.iter().any(|r| r.routine == routine) {
                    refs.push(FeatRef {
                        routine,
                        languages: Vec::new(),
                    });
                    self.ff.arena[routine].usecount += 1;
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Lookup dispatch
    // ------------------------------------------------------------------

    /// Runs `f` on the subtables, or for an extension lookup, on each
    /// subtable separately and wraps the results into an "Extension" routine
    /// (`unparseExtension`).
    fn run<T>(
        &mut self,
        subtables: impl Iterator<Item = Result<T, ReadError>>,
        f: fn(&mut Self, Vec<T>) -> Result<Routine, Error>,
    ) -> Result<Routine, Error> {
        let subtables = all(subtables)?;
        if !self.is_ext {
            return f(self, subtables);
        }
        let mut rules = Vec::new();
        for sub in subtables {
            rules.extend(f(self, vec![sub])?.rules);
        }
        self.extension(rules)
    }

    fn extension(&mut self, rules: Vec<Rule>) -> Result<Routine, Error> {
        let mut r = self.start("Extension")?;
        r.rules = rules;
        Ok(r)
    }

    fn unparse_gsub_lookup(&mut self, lookup: &SubstitutionLookup) -> Result<Routine, Error> {
        self.flag = lookup.lookup_flag().to_bits();
        self.mark_filtering_set = lookup.mark_filtering_set();
        self.is_ext = lookup.lookup_type() == 7;
        match lookup.subtables()? {
            SubstitutionSubtables::Single(s) => self.run(s.iter(), Self::single_subst),
            SubstitutionSubtables::Multiple(s) => self.run(s.iter(), Self::multiple_subst),
            SubstitutionSubtables::Alternate(s) => self.run(s.iter(), Self::alternate_subst),
            SubstitutionSubtables::Ligature(s) => self.run(s.iter(), Self::ligature_subst),
            SubstitutionSubtables::Contextual(s) => self.run(s.iter(), Self::contextual),
            SubstitutionSubtables::ChainContextual(s) => {
                self.run(s.iter(), Self::chained_contextual)
            }
            SubstitutionSubtables::Reverse(s) => self.run(s.iter(), Self::reverse_subst),
            SubstitutionSubtables::EmptyExtension => self.extension(Vec::new()),
        }
    }

    fn unparse_gpos_lookup(&mut self, lookup: &PositionLookup) -> Result<Routine, Error> {
        self.flag = lookup.lookup_flag().to_bits();
        self.mark_filtering_set = lookup.mark_filtering_set();
        self.is_ext = lookup.lookup_type() == 9;
        match lookup.subtables()? {
            PositionSubtables::Single(s) => self.run(s.iter(), Self::single_pos),
            PositionSubtables::Pair(s) => self.run(s.iter(), Self::pair_pos),
            PositionSubtables::Cursive(s) => self.run(s.iter(), Self::cursive_pos),
            PositionSubtables::MarkToBase(s) => self.run(s.iter(), Self::mark_to_base),
            PositionSubtables::MarkToLig(s) => self.run(s.iter(), Self::mark_to_ligature),
            PositionSubtables::MarkToMark(s) => self.run(s.iter(), Self::mark_to_mark),
            PositionSubtables::Contextual(s) => self.run(s.iter(), Self::contextual),
            PositionSubtables::ChainContextual(s) => self.run(s.iter(), Self::chained_contextual),
            PositionSubtables::EmptyExtension => self.extension(Vec::new()),
        }
    }

    // ------------------------------------------------------------------
    // GSUB
    // ------------------------------------------------------------------

    fn single_subst(&mut self, subs: Vec<SingleSubst>) -> Result<Routine, Error> {
        let mut b = self.start("SingleSubstitution")?;
        for sub in subs {
            let mut mapping: IndexMap<String, String> = IndexMap::new();
            match &sub {
                SingleSubst::Format1(t) => {
                    let cov = t.coverage()?;
                    let input = coverage_glyphs(self.names, &cov)?;
                    let gids = coverage_gids(&cov)?;
                    let delta = t.delta_glyph_id() as i32;
                    for (inp, gid) in input.into_iter().zip(gids) {
                        let out = (gid as i32 + delta).rem_euclid(65536) as u32;
                        mapping.insert(inp, self.name(out));
                    }
                }
                SingleSubst::Format2(t) => {
                    let input = coverage_glyphs(self.names, &t.coverage()?)?;
                    if input.len() != t.glyph_count() as usize {
                        return Err("invalid SingleSubstFormat2 table".into());
                    }
                    for (inp, out) in input.into_iter().zip(t.substitute_glyph_ids()) {
                        mapping.insert(inp, self.name(out.get().to_u32()));
                    }
                }
                _ => return Err("unsupported SingleSubst format".into()),
            }
            for (k, v) in mapping {
                b.rules
                    .push(self.subst(vec![single(k)], vec![single(v)], false));
            }
        }
        Ok(b)
    }

    fn multiple_subst(&mut self, subs: Vec<MultipleSubst>) -> Result<Routine, Error> {
        let mut b = self.start("MultipleSubstitution")?;
        for sub in subs {
            let MultipleSubst::Format1(t) = &sub else {
                return Err("unsupported MultipleSubst format".into());
            };
            let glyphs = coverage_glyphs(self.names, &t.coverage()?)?;
            let seqs = all(t.sequences().iter())?;
            let mut mapping: IndexMap<String, Vec<String>> = IndexMap::new();
            for (g, seq) in glyphs.into_iter().zip(seqs) {
                let out = self.glyph_names(seq.substitute_glyph_ids());
                mapping.insert(g, out);
            }
            for (k, outs) in mapping {
                b.rules.push(self.subst(
                    vec![single(k)],
                    outs.into_iter().map(single).collect(),
                    false,
                ));
            }
        }
        Ok(b)
    }

    fn alternate_subst(&mut self, subs: Vec<AlternateSubst>) -> Result<Routine, Error> {
        let mut b = self.start("AlternateSubstitution")?;
        for sub in subs {
            let AlternateSubst::Format1(t) = &sub else {
                return Err("unsupported AlternateSubst format".into());
            };
            let input = coverage_glyphs(self.names, &t.coverage()?)?;
            let sets = all(t.alternate_sets().iter())?;
            if input.len() != sets.len() {
                return Err("AlternateSubst coverage and AlternateSet counts differ".into());
            }
            let mut alternates: IndexMap<String, Vec<String>> = IndexMap::new();
            for (g, set) in input.into_iter().zip(sets) {
                let alts = self.glyph_names(set.alternate_glyph_ids());
                alternates.insert(g, alts);
            }
            for (k, alts) in alternates {
                b.rules.push(self.subst(vec![single(k)], vec![alts], true));
            }
        }
        Ok(b)
    }

    fn ligature_subst(&mut self, subs: Vec<LigatureSubst>) -> Result<Routine, Error> {
        let mut b = self.start("LigatureSubstitution")?;
        for sub in subs {
            let LigatureSubst::Format1(t) = &sub else {
                return Err("unsupported LigatureSubst format".into());
            };
            let input = coverage_glyphs(self.names, &t.coverage()?)?;
            let sets = all(t.ligature_sets().iter())?;
            if input.len() != sets.len() {
                return Err("LigatureSubst coverage and LigatureSet counts differ".into());
            }
            let mut ligatures: IndexMap<String, Vec<(Vec<String>, String)>> = IndexMap::new();
            for (g, set) in input.into_iter().zip(sets) {
                let ligs = all(set.ligatures().iter())?;
                let ligs = ligs
                    .iter()
                    .map(|lig| {
                        let comps = self.glyph_names(lig.component_glyph_ids());
                        (comps, self.name(lig.ligature_glyph().to_u32()))
                    })
                    .collect();
                ligatures.insert(g, ligs);
            }
            for (first, ligs) in ligatures {
                for (comps, lig) in ligs {
                    let mut input = vec![single(first.clone())];
                    input.extend(comps.into_iter().map(single));
                    b.rules.push(self.subst(input, vec![single(lig)], false));
                }
            }
        }
        Ok(b)
    }

    fn reverse_subst(&mut self, subs: Vec<ReverseChainSingleSubst>) -> Result<Routine, Error> {
        let mut b = self.start("ReverseContextualSubstitution")?;
        for sub in subs {
            let ReverseChainSingleSubst::Format1(t) = &sub else {
                return Err("unsupported ReverseChainSingleSubst format".into());
            };
            let mut prefix = self.coverage_slots(t.backtrack_coverages().iter())?;
            prefix.reverse();
            let suffix = self.coverage_slots(t.lookahead_coverages().iter())?;
            let outputs: Vec<String> = self.glyph_names(t.substitute_glyph_ids());
            let inputs = coverage_glyphs(self.names, &t.coverage()?)?;
            let kind = RuleKind::Substitution {
                input: vec![inputs],
                replacement: vec![outputs],
                precontext: prefix,
                postcontext: suffix,
                reverse: true,
                force_alt: false,
            };
            // This is the one rule the unparser creates without an address.
            b.rules.push(Rule {
                kind,
                address: None,
            });
        }
        Ok(b)
    }

    // ------------------------------------------------------------------
    // GPOS
    // ------------------------------------------------------------------

    fn single_pos(&mut self, subs: Vec<SinglePos>) -> Result<Routine, Error> {
        let mut b = self.start("SinglePositioning")?;
        for sub in subs {
            match &sub {
                SinglePos::Format1(t) => {
                    let glyphs = coverage_glyphs(self.names, &t.coverage()?)?;
                    let kind = RuleKind::Positioning {
                        glyphs: vec![glyphs],
                        valuerecords: vec![value_record(&t.value_record())],
                    };
                    b.rules.push(self.rule(kind));
                }
                SinglePos::Format2(t) => {
                    let glyphs = coverage_glyphs(self.names, &t.coverage()?)?;
                    let values = all(t.value_records().iter())?;
                    for (g, v) in glyphs.into_iter().zip(values.iter()) {
                        let kind = RuleKind::Positioning {
                            glyphs: vec![single(g)],
                            valuerecords: vec![value_record(v)],
                        };
                        b.rules.push(self.rule(kind));
                    }
                }
                _ => return Err("unsupported SinglePos format".into()),
            }
        }
        Ok(b)
    }

    fn pair_pos(&mut self, subs: Vec<PairPos>) -> Result<Routine, Error> {
        let mut b = self.start("PairPositioning")?;
        for sub in subs {
            match &sub {
                PairPos::Format1(t) => {
                    let glyphs = coverage_glyphs(self.names, &t.coverage()?)?;
                    let sets = all(t.pair_sets().iter())?;
                    for (g, set) in glyphs.into_iter().zip(sets) {
                        for vr in all(set.pair_value_records().iter())? {
                            let second = self.name(vr.second_glyph().to_u32());
                            let kind = RuleKind::Positioning {
                                glyphs: vec![single(g.clone()), single(second)],
                                valuerecords: vec![
                                    value_record(vr.value_record1()),
                                    value_record(vr.value_record2()),
                                ],
                            };
                            b.rules.push(self.rule(kind));
                        }
                    }
                }
                PairPos::Format2(t) => {
                    let class1 = self.classes(t.class_def1())?;
                    let class2 = self.classes(t.class_def2())?;
                    let coverage: HashSet<String> = coverage_glyphs(self.names, &t.coverage()?)?
                        .into_iter()
                        .collect();
                    for (ix1, c1) in t.class1_records().iter().enumerate() {
                        let c1 = c1?;
                        let Some(first_all) = class1.get(&(ix1 as u16)) else {
                            continue;
                        };
                        for (ix2, c2) in c1.class2_records().iter().enumerate() {
                            let c2 = c2?;
                            let Some(second) = class2.get(&(ix2 as u16)) else {
                                continue;
                            };
                            let vr1 = value_record(c2.value_record1());
                            let vr2 = value_record(c2.value_record2());
                            if !vr1.truthy() && !vr2.truthy() {
                                continue;
                            }
                            // `list(set(class1[ix1]) & set(coverage))`: its order
                            // doesn't matter, positioning classes are sorted on output.
                            let mut seen = HashSet::new();
                            let first: Vec<String> = first_all
                                .iter()
                                .filter(|g| coverage.contains(*g) && seen.insert((*g).clone()))
                                .cloned()
                                .collect();
                            if ix2 == 0 && second.len() == 1 {
                                // Python indexes the class-0 set: TypeError.
                                return Err(
                                    "PairPos class 2 #0 with a single glyph cannot be unparsed"
                                        .into(),
                                );
                            }
                            let kind = RuleKind::Positioning {
                                glyphs: vec![first, second.clone()],
                                valuerecords: vec![vr1, vr2],
                            };
                            b.rules.push(self.rule(kind));
                        }
                    }
                }
                _ => return Err("unsupported PairPos format".into()),
            }
        }
        Ok(b)
    }

    fn cursive_pos(&mut self, subs: Vec<CursivePos>) -> Result<Routine, Error> {
        let mut b = self.start("CursiveAttachment")?;
        let mut entries: IndexMap<String, (i32, i32)> = IndexMap::new();
        let mut exits: IndexMap<String, (i32, i32)> = IndexMap::new();
        for sub in subs {
            let CursivePos::Format1(t) = &sub else {
                return Err("unsupported CursivePos format".into());
            };
            let glyphs = coverage_glyphs(self.names, &t.coverage()?)?;
            for (g, rec) in glyphs.into_iter().zip(t.entry_exit_record()) {
                if let Some(a) = rec.entry_anchor(t.offset_data()) {
                    entries.insert(g.clone(), anchor_xy(&a?));
                }
                if let Some(a) = rec.exit_anchor(t.offset_data()) {
                    exits.insert(g, anchor_xy(&a?));
                }
            }
        }
        let att = Attachment {
            base_name: "cursive_entry".into(),
            fullname: "cursive_entry_cursive_exit".into(),
            bases: entries,
            marks: exits,
            has_font: false,
            force_markmark: false,
        };
        b.rules.push(self.rule(RuleKind::Attachment(att)));
        Ok(b)
    }

    /// `_formatMarkArray`
    fn mark_array(
        &self,
        array: &read_fonts::tables::gpos::MarkArray,
        coverage: &[String],
        class_id: u16,
    ) -> Result<IndexMap<String, (i32, i32)>, Error> {
        let mut marks = IndexMap::new();
        for (i, rec) in array.mark_records().iter().enumerate() {
            if rec.mark_class() == class_id {
                let anchor = rec.mark_anchor(array.offset_data())?;
                let g = coverage.get(i).ok_or("mark coverage too short")?;
                marks.insert(g.clone(), anchor_xy(&anchor));
            }
        }
        Ok(marks)
    }

    /// The rules of `unparseMarkToBase` and `unparseMarkToMark` for one
    /// subtable: an attachment per mark class.
    fn mark_attachments(
        &mut self,
        b: &mut Routine,
        t: &MarkAttachment,
        mark_to_mark: bool,
    ) -> Result<(), Error> {
        let mark_cov = coverage_glyphs(self.names, &t.mark_coverage)?;
        let base_cov = coverage_glyphs(self.names, &t.base_coverage)?;
        for class_id in 0..t.class_count {
            let prefix = format!("Anchor{}", self.gensym());
            let marks = self.mark_array(&t.marks, &mark_cov, class_id)?;
            let mut bases = IndexMap::new();
            for (i, row) in t.base_anchors.iter().enumerate() {
                let Some(anchor) = row.get(class_id as usize).cloned().flatten() else {
                    continue;
                };
                let anchor = anchor?;
                let g = base_cov.get(i).ok_or(if mark_to_mark {
                    "mark2 coverage too short"
                } else {
                    "base coverage too short"
                })?;
                bases.insert(g.clone(), anchor_xy(&anchor));
            }
            let fullname = if mark_to_mark {
                format!("{prefix}_{prefix}_")
            } else {
                format!("{prefix}__{prefix}")
            };
            let att = Attachment {
                fullname,
                base_name: prefix,
                bases,
                marks,
                has_font: true,
                force_markmark: mark_to_mark,
            };
            b.rules.push(self.rule(RuleKind::Attachment(att)));
        }
        Ok(())
    }

    fn mark_to_base(&mut self, subs: Vec<MarkBasePos>) -> Result<Routine, Error> {
        let mut b = self.start("MarkToBase")?;
        for sub in subs {
            let MarkBasePos::Format1(t) = &sub else {
                return Err("unsupported MarkBasePos format".into());
            };
            self.mark_attachments(&mut b, &MarkAttachment::mark_to_base(t)?, false)?;
        }
        Ok(b)
    }

    /// `unparseMarkToLigature`: fontFeatures only dumps the subtables as XML
    /// comments ("unparsable"), which `asFea` never prints, so the routine
    /// has no rules and only its name and flags reach the output.
    fn mark_to_ligature(&mut self, _subs: Vec<MarkLigPos>) -> Result<Routine, Error> {
        self.start("MarkToLigature")
    }

    fn mark_to_mark(&mut self, subs: Vec<MarkMarkPos>) -> Result<Routine, Error> {
        let mut b = self.start("MarkToMark")?;
        for sub in subs {
            let MarkMarkPos::Format1(t) = &sub else {
                return Err("unsupported MarkMarkPos format".into());
            };
            self.mark_attachments(&mut b, &MarkAttachment::mark_to_mark(t)?, true)?;
        }
        Ok(b)
    }

    // ------------------------------------------------------------------
    // Contextual (GSUB 5/6, GPOS 7/8)
    // ------------------------------------------------------------------

    /// `_unparse_lookups`
    #[allow(clippy::type_complexity)]
    fn unparse_lookups(
        &self,
        records: &[SequenceLookupRecord],
        n_inputs: usize,
    ) -> Result<(Vec<Option<Vec<Ref>>>, Option<String>), Error> {
        let indices: Vec<u16> = records.iter().map(|r| r.sequence_index()).collect();
        let mut sorted = indices.clone();
        sorted.sort();
        let note = (indices != sorted).then(|| {
            format!(
                "Out-of-order lookup application in lookup {}\n A chaining rule calls lookups in the order {}\n \
                 This cannot be directly expressed in Adobe FEA syntax;\n  asFea() output will be incorrect",
                self.current_lookup,
                py_int_list(&indices)
            )
        });
        let mut lookups: Vec<Option<Vec<Ref>>> = Vec::new();
        for r in records {
            let si = r.sequence_index() as usize;
            if lookups.len() <= si {
                let extra = 1 + si - lookups.len();
                lookups.extend(std::iter::repeat_n(None, extra));
            }
            if lookups[si].as_ref().is_none_or(|l| l.is_empty()) {
                lookups[si] = Some(Vec::new());
            }
            let li = r.lookup_list_index() as usize;
            let Some(&routine) = self.lookups.get(li) else {
                return Err(format!("Lookups unparsed out of order: unparse {li} first!").into());
            };
            let name = self.ff.arena[routine].name.clone();
            lookups[si].as_mut().unwrap().push(Ref { routine, name });
        }
        if lookups.len() < n_inputs {
            lookups.extend(std::iter::repeat_n(None, n_inputs - lookups.len()));
        }
        if lookups.len() != n_inputs {
            return Err(format!(
                "a contextual rule in lookup {} applies a lookup beyond its input sequence",
                self.current_lookup
            )
            .into());
        }
        Ok((lookups, note))
    }

    fn chain_rule(
        &self,
        input: Vec<Slot>,
        precontext: Vec<Slot>,
        postcontext: Vec<Slot>,
        records: &[SequenceLookupRecord],
    ) -> Result<Rule, Error> {
        let (lookups, note) = self.unparse_lookups(records, input.len())?;
        Ok(self.rule(RuleKind::Chaining {
            input,
            precontext,
            postcontext,
            lookups,
            note,
        }))
    }

    fn contextual(&mut self, subs: Vec<SequenceContext>) -> Result<Routine, Error> {
        let mut b = self.start(&format!("Contextual{}", self.table))?;
        for sub in subs {
            match &sub {
                SequenceContext::Format1(t) => {
                    let cov = coverage_glyphs(self.names, &t.coverage()?)?;
                    for (ruleset, first) in t.seq_rule_sets().iter().zip(cov) {
                        let ruleset = ruleset.ok_or("null SubRuleSet")??;
                        for rule in all(ruleset.seq_rules().iter())? {
                            let mut input = vec![single(first.clone())];
                            input.extend(self.names_of(rule.input_sequence()));
                            let r =
                                self.chain_rule(input, vec![], vec![], rule.seq_lookup_records())?;
                            b.rules.push(r);
                        }
                    }
                }
                SequenceContext::Format2(t) => {
                    let cov: HashSet<String> = coverage_glyphs(self.names, &t.coverage()?)?
                        .into_iter()
                        .collect();
                    let inputs = self.classes(t.class_def())?;
                    for (class_id, ruleset) in t.class_seq_rule_sets().iter().enumerate() {
                        let Some(ruleset) = ruleset else { continue };
                        let ruleset = ruleset?;
                        let inputclass = input_class(&inputs, class_id, &cov);
                        for rule in all(ruleset.class_seq_rules().iter())? {
                            let mut input = vec![inputclass.clone()];
                            for c in rule.input_sequence() {
                                input.push(class_of(&inputs, c.get())?);
                            }
                            let r =
                                self.chain_rule(input, vec![], vec![], rule.seq_lookup_records())?;
                            b.rules.push(r);
                        }
                    }
                }
                SequenceContext::Format3(t) => {
                    let input = self.coverage_slots(t.coverages().iter())?;
                    let r = self.chain_rule(input, vec![], vec![], t.seq_lookup_records())?;
                    b.rules.push(r);
                }
                _ => return Err("unsupported SequenceContext format".into()),
            }
        }
        Ok(b)
    }

    fn chained_contextual(&mut self, subs: Vec<ChainedSequenceContext>) -> Result<Routine, Error> {
        let mut b = self.start(&format!("ChainedContextual{}", self.table))?;
        for sub in subs {
            match &sub {
                ChainedSequenceContext::Format1(t) => {
                    let cov = coverage_glyphs(self.names, &t.coverage()?)?;
                    for (ruleset, first) in t.chained_seq_rule_sets().iter().zip(cov) {
                        let ruleset = ruleset.ok_or("null ChainSubRuleSet")??;
                        for rule in all(ruleset.chained_seq_rules().iter())? {
                            let mut input = vec![single(first.clone())];
                            input.extend(self.names_of(rule.input_sequence()));
                            let mut prefix = self.names_of(rule.backtrack_sequence());
                            prefix.reverse();
                            let suffix = self.names_of(rule.lookahead_sequence());
                            let r =
                                self.chain_rule(input, prefix, suffix, rule.seq_lookup_records())?;
                            b.rules.push(r);
                        }
                    }
                }
                ChainedSequenceContext::Format2(t) => {
                    let backtrack = if t.backtrack_class_def_offset().is_null() {
                        IndexMap::new()
                    } else {
                        self.classes(t.backtrack_class_def())?
                    };
                    let lookahead = if t.lookahead_class_def_offset().is_null() {
                        IndexMap::new()
                    } else {
                        self.classes(t.lookahead_class_def())?
                    };
                    let inputs = self.classes(t.input_class_def())?;
                    let cov: HashSet<String> = coverage_glyphs(self.names, &t.coverage()?)?
                        .into_iter()
                        .collect();
                    for (class_id, ruleset) in t.chained_class_seq_rule_sets().iter().enumerate() {
                        let Some(ruleset) = ruleset else { continue };
                        let ruleset = ruleset?;
                        let inputclass = input_class(&inputs, class_id, &cov);
                        for rule in all(ruleset.chained_class_seq_rules().iter())? {
                            let mut prefix = Vec::new();
                            for c in rule.backtrack_sequence() {
                                prefix.push(class_of(&backtrack, c.get())?);
                            }
                            prefix.reverse();
                            let mut suffix = Vec::new();
                            for c in rule.lookahead_sequence() {
                                suffix.push(class_of(&lookahead, c.get())?);
                            }
                            let mut input = vec![inputclass.clone()];
                            for c in rule.input_sequence() {
                                input.push(class_of(&inputs, c.get())?);
                            }
                            let r =
                                self.chain_rule(input, prefix, suffix, rule.seq_lookup_records())?;
                            b.rules.push(r);
                        }
                    }
                }
                ChainedSequenceContext::Format3(t) => {
                    let mut prefix = self.coverage_slots(t.backtrack_coverages().iter())?;
                    prefix.reverse();
                    let suffix = self.coverage_slots(t.lookahead_coverages().iter())?;
                    let input = self.coverage_slots(t.input_coverages().iter())?;
                    let r = self.chain_rule(input, prefix, suffix, t.seq_lookup_records())?;
                    b.rules.push(r);
                }
                _ => return Err("unsupported ChainedSequenceContext format".into()),
            }
        }
        Ok(b)
    }
}

/// `[g for g in inputs.get(classId, []) if g in sub.Coverage.glyphs]`
fn input_class(
    inputs: &IndexMap<u16, Vec<String>>,
    class_id: usize,
    cov: &HashSet<String>,
) -> Vec<String> {
    let Ok(class_id) = u16::try_from(class_id) else {
        return Vec::new();
    };
    inputs
        .get(&class_id)
        .map(|v| v.iter().filter(|g| cov.contains(*g)).cloned().collect())
        .unwrap_or_default()
}

/// `classes[c]` for a backtrack, lookahead or later input slot of a
/// contextual format 2 rule.
fn class_of(classes: &IndexMap<u16, Vec<String>>, c: u16) -> Result<Vec<String>, Error> {
    let glyphs = classes
        .get(&c)
        .cloned()
        .ok_or_else(|| Error::from(format!("class {c} is not defined in the ClassDef")))?;
    if c == 0 && glyphs.len() == 1 {
        // Class 0 is a Python set here, and Chaining's glyphref indexes a
        // one-glyph class: TypeError.
        return Err("contextual rule with a one-glyph class 0 cannot be unparsed".into());
    }
    Ok(glyphs)
}
