//! Port of `fontFeatures.ttLib` (GTableUnparser, GSUBUnparser, GPOSUnparser):
//! binary GSUB/GPOS lookups to fontFeatures routines.

use std::collections::HashSet;

use read_fonts::ReadError;
use read_fonts::tables::gpos::{
    AnchorTable, CursivePos, Gpos, MarkBasePos, MarkMarkPos, PairPos, PositionLookup,
    PositionSubtables, SinglePos,
};
use read_fonts::tables::gsub::{
    AlternateSubst, Gsub, LigatureSubst, MultipleSubst, ReverseChainSingleSubst, SingleSubst,
    SubstitutionLookup, SubstitutionSubtables,
};
use read_fonts::tables::layout::{
    ChainedSequenceContext, FeatureList, LangSys, ScriptList, SequenceContext, SequenceLookupRecord,
};
use read_fonts::types::Tag;

use super::model::{
    Attachment, FeatRef, FontFeatures, ODict, Ref, Routine, Rule, RuleKind, Slot, ValueRecord,
};
use super::tables::{
    GdefInfo, GlyphNames, class_defs, coverage_glyphs, invert_class_def, opt_coverage_glyphs,
    read_err,
};
use crate::Error;

macro_rules! collect {
    ($iter:expr) => {
        $iter
            .collect::<Result<Vec<_>, ReadError>>()
            .map_err(read_err)?
    };
}

/// fontTools reads tags as Latin-1 strings.
pub fn tag_str(tag: Tag) -> String {
    tag.to_be_bytes().iter().map(|&b| b as char).collect()
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
    pub names: &'a GlyphNames,
    pub gdef: &'a GdefInfo,
    /// `_table`: "GSUB" or "GPOS".
    pub table: &'static str,
    index: usize,
    /// Arena indices of this table's routines, by lookup index.
    lookups: Vec<usize>,
    current_lookup: usize,
}

impl<'a> Unparser<'a> {
    pub fn new(
        ff: &'a mut FontFeatures,
        names: &'a GlyphNames,
        gdef: &'a GdefInfo,
        table: &'static str,
    ) -> Self {
        Unparser {
            ff,
            names,
            gdef,
            table,
            index: 0,
            lookups: Vec::new(),
            current_lookup: 0,
        }
    }

    fn gensym(&mut self) -> String {
        self.index += 1;
        self.index.to_string()
    }

    fn name(&self, gid: u32) -> String {
        self.names.name(gid)
    }

    /// `_fix_flags`: a routine with the lookup's flags and mark sets.
    fn routine(&self, name: String, flag: u16, mfs: Option<u16>) -> Result<Routine, Error> {
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
                defs.items
                    .iter()
                    .filter(|(_, c)| *c == wanted)
                    .map(|(g, _)| g.clone())
                    .collect(),
            );
        }
        if flag & 0x10 != 0 {
            if let Some(idx) = mfs {
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

    // ------------------------------------------------------------------
    // Table level
    // ------------------------------------------------------------------

    /// `GTableUnparser.unparse` for GSUB.
    pub fn unparse_gsub(&mut self, gsub: &Gsub) -> Result<(), Error> {
        let Some(script_list) = gsub.original_script_list() else {
            return Ok(());
        };
        let script_list = script_list.map_err(read_err)?;
        if let Some(lookup_list) = gsub.original_lookup_list() {
            let lookup_list = lookup_list.map_err(read_err)?;
            let lookups: Vec<SubstitutionLookup> = collect!(lookup_list.lookups().iter());
            self.make_dummies(lookups.len());
            for (idx, lookup) in lookups.iter().enumerate() {
                let routine = self.unparse_gsub_lookup(lookup, idx)?;
                self.copy_routine(routine, idx);
            }
        }
        let feature_list = gsub.original_feature_list();
        self.collect_features(&script_list, feature_list)?;
        self.ff.resolve_all_routines();
        Ok(())
    }

    /// `GTableUnparser.unparse` for GPOS.
    pub fn unparse_gpos(&mut self, gpos: &Gpos) -> Result<(), Error> {
        let Some(script_list) = gpos.original_script_list() else {
            return Ok(());
        };
        let script_list = script_list.map_err(read_err)?;
        if let Some(lookup_list) = gpos.original_lookup_list() {
            let lookup_list = lookup_list.map_err(read_err)?;
            let lookups: Vec<PositionLookup> = collect!(lookup_list.lookups().iter());
            self.make_dummies(lookups.len());
            for (idx, lookup) in lookups.iter().enumerate() {
                let routine = self.unparse_gpos_lookup(lookup, idx)?;
                self.copy_routine(routine, idx);
            }
        }
        let feature_list = gpos.original_feature_list();
        self.collect_features(&script_list, feature_list)?;
        self.ff.resolve_all_routines();
        Ok(())
    }

    fn make_dummies(&mut self, n: usize) {
        for _ in 0..n {
            let id = self.ff.new_routine(Routine::default());
            self.lookups.push(id);
            self.ff.routines.push(id);
        }
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
        let feature_list = match feature_list {
            None => None,
            Some(fl) => Some(fl.map_err(read_err)?),
        };
        for record in script_list.script_records() {
            let script_tag = tag_str(record.script_tag());
            let script = record.script(script_list.offset_data()).map_err(read_err)?;
            if let Some(dls) = script.default_lang_sys() {
                let dls = dls.map_err(read_err)?;
                self.prepare_feature_lang_sys("dflt", &dls, feature_list.as_ref(), &script_tag)?;
            }
            for lsr in script.lang_sys_records() {
                let lang_tag = tag_str(lsr.lang_sys_tag());
                let ls = lsr.lang_sys(script.offset_data()).map_err(read_err)?;
                self.prepare_feature_lang_sys(&lang_tag, &ls, feature_list.as_ref(), &script_tag)?;
            }
        }
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
            let feature = record
                .feature(feature_list.offset_data())
                .map_err(read_err)?;
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
    #[allow(clippy::type_complexity)]
    fn run<T>(
        &mut self,
        is_ext: bool,
        subtables: Vec<T>,
        flag: u16,
        mfs: Option<u16>,
        f: fn(&mut Self, Vec<T>, u16, Option<u16>) -> Result<Routine, Error>,
    ) -> Result<Routine, Error> {
        if !is_ext {
            return f(self, subtables, flag, mfs);
        }
        let mut rules = Vec::new();
        for sub in subtables {
            let r = f(self, vec![sub], flag, mfs)?;
            rules.extend(r.rules);
        }
        self.extension(rules, flag, mfs)
    }

    fn extension(
        &mut self,
        rules: Vec<Rule>,
        flag: u16,
        mfs: Option<u16>,
    ) -> Result<Routine, Error> {
        let name = format!("Extension{}", self.gensym());
        let mut r = self.routine(name, flag, mfs)?;
        r.rules = rules;
        Ok(r)
    }

    fn unparse_gsub_lookup(
        &mut self,
        lookup: &SubstitutionLookup,
        idx: usize,
    ) -> Result<Routine, Error> {
        self.current_lookup = idx;
        let flag = lookup.lookup_flag().to_bits();
        let mfs = lookup.mark_filtering_set();
        let is_ext = lookup.lookup_type() == 7;
        match lookup.subtables().map_err(read_err)? {
            SubstitutionSubtables::Single(s) => {
                let v = collect!(s.iter());
                self.run(is_ext, v, flag, mfs, Self::single_subst)
            }
            SubstitutionSubtables::Multiple(s) => {
                let v = collect!(s.iter());
                self.run(is_ext, v, flag, mfs, Self::multiple_subst)
            }
            SubstitutionSubtables::Alternate(s) => {
                let v = collect!(s.iter());
                self.run(is_ext, v, flag, mfs, Self::alternate_subst)
            }
            SubstitutionSubtables::Ligature(s) => {
                let v = collect!(s.iter());
                self.run(is_ext, v, flag, mfs, Self::ligature_subst)
            }
            SubstitutionSubtables::Contextual(s) => {
                let v = collect!(s.iter());
                self.run(is_ext, v, flag, mfs, Self::contextual)
            }
            SubstitutionSubtables::ChainContextual(s) => {
                let v = collect!(s.iter());
                self.run(is_ext, v, flag, mfs, Self::chained_contextual)
            }
            SubstitutionSubtables::Reverse(s) => {
                let v = collect!(s.iter());
                self.run(is_ext, v, flag, mfs, Self::reverse_subst)
            }
            SubstitutionSubtables::EmptyExtension => self.extension(Vec::new(), flag, mfs),
        }
    }

    fn unparse_gpos_lookup(
        &mut self,
        lookup: &PositionLookup,
        idx: usize,
    ) -> Result<Routine, Error> {
        self.current_lookup = idx;
        let flag = lookup.lookup_flag().to_bits();
        let mfs = lookup.mark_filtering_set();
        let is_ext = lookup.lookup_type() == 9;
        match lookup.subtables().map_err(read_err)? {
            PositionSubtables::Single(s) => {
                let v = collect!(s.iter());
                self.run(is_ext, v, flag, mfs, Self::single_pos)
            }
            PositionSubtables::Pair(s) => {
                let v = collect!(s.iter());
                self.run(is_ext, v, flag, mfs, Self::pair_pos)
            }
            PositionSubtables::Cursive(s) => {
                let v = collect!(s.iter());
                self.run(is_ext, v, flag, mfs, Self::cursive_pos)
            }
            PositionSubtables::MarkToBase(s) => {
                let v = collect!(s.iter());
                self.run(is_ext, v, flag, mfs, Self::mark_to_base)
            }
            PositionSubtables::MarkToLig(_) => Err(
                "GPOS lookup type 5 (mark-to-ligature) is not supported: fontFeatures dumps it as \
                 fontTools XML comments"
                    .into(),
            ),
            PositionSubtables::MarkToMark(s) => {
                let v = collect!(s.iter());
                self.run(is_ext, v, flag, mfs, Self::mark_to_mark)
            }
            PositionSubtables::Contextual(s) => {
                let v = collect!(s.iter());
                self.run(is_ext, v, flag, mfs, Self::contextual)
            }
            PositionSubtables::ChainContextual(s) => {
                let v = collect!(s.iter());
                self.run(is_ext, v, flag, mfs, Self::chained_contextual)
            }
            PositionSubtables::EmptyExtension => self.extension(Vec::new(), flag, mfs),
        }
    }

    // ------------------------------------------------------------------
    // GSUB
    // ------------------------------------------------------------------

    fn single_subst(
        &mut self,
        subs: Vec<SingleSubst>,
        flag: u16,
        mfs: Option<u16>,
    ) -> Result<Routine, Error> {
        let name = format!("SingleSubstitution{}", self.gensym());
        let mut b = self.routine(name, flag, mfs)?;
        for sub in subs {
            let mut mapping: ODict<String, String> = ODict::new();
            match &sub {
                SingleSubst::Format1(t) => {
                    let cov = t.coverage().map_err(read_err)?;
                    let input = coverage_glyphs(self.names, &cov)?;
                    let gids = coverage_gids(&cov)?;
                    let delta = t.delta_glyph_id() as i32;
                    for (inp, gid) in input.into_iter().zip(gids) {
                        let out = (gid as i32 + delta).rem_euclid(65536) as u32;
                        mapping.insert(inp, self.name(out));
                    }
                }
                SingleSubst::Format2(t) => {
                    let input = opt_coverage_glyphs(self.names, t.coverage())?;
                    if input.len() != t.glyph_count() as usize {
                        return Err("invalid SingleSubstFormat2 table".into());
                    }
                    for (inp, out) in input.into_iter().zip(t.substitute_glyph_ids()) {
                        mapping.insert(inp, self.name(out.get().to_u32()));
                    }
                }
                _ => return Err("unsupported SingleSubst format".into()),
            }
            for (k, v) in mapping.items {
                let kind = RuleKind::Substitution {
                    input: vec![single(k)],
                    replacement: vec![single(v)],
                    precontext: vec![],
                    postcontext: vec![],
                    reverse: false,
                    force_alt: false,
                };
                b.rules.push(self.rule(kind));
            }
        }
        Ok(b)
    }

    fn multiple_subst(
        &mut self,
        subs: Vec<MultipleSubst>,
        flag: u16,
        mfs: Option<u16>,
    ) -> Result<Routine, Error> {
        let name = format!("MultipleSubstitution{}", self.gensym());
        let mut b = self.routine(name, flag, mfs)?;
        for sub in subs {
            let MultipleSubst::Format1(t) = &sub else {
                return Err("unsupported MultipleSubst format".into());
            };
            let glyphs = opt_coverage_glyphs(self.names, t.coverage())?;
            let seqs = collect!(t.sequences().iter());
            let mut mapping: ODict<String, Vec<String>> = ODict::new();
            for (g, seq) in glyphs.into_iter().zip(seqs) {
                let out = seq
                    .substitute_glyph_ids()
                    .iter()
                    .map(|x| self.name(x.get().to_u32()))
                    .collect();
                mapping.insert(g, out);
            }
            for (k, outs) in mapping.items {
                let kind = RuleKind::Substitution {
                    input: vec![single(k)],
                    replacement: outs.into_iter().map(single).collect(),
                    precontext: vec![],
                    postcontext: vec![],
                    reverse: false,
                    force_alt: false,
                };
                b.rules.push(self.rule(kind));
            }
        }
        Ok(b)
    }

    fn alternate_subst(
        &mut self,
        subs: Vec<AlternateSubst>,
        flag: u16,
        mfs: Option<u16>,
    ) -> Result<Routine, Error> {
        let name = format!("AlternateSubstitution{}", self.gensym());
        let mut b = self.routine(name, flag, mfs)?;
        for sub in subs {
            let AlternateSubst::Format1(t) = &sub else {
                return Err("unsupported AlternateSubst format".into());
            };
            let input = opt_coverage_glyphs(self.names, t.coverage())?;
            let sets = collect!(t.alternate_sets().iter());
            if input.len() != sets.len() {
                return Err("AlternateSubst coverage and AlternateSet counts differ".into());
            }
            let mut alternates: ODict<String, Vec<String>> = ODict::new();
            for (g, set) in input.into_iter().zip(sets) {
                let alts = set
                    .alternate_glyph_ids()
                    .iter()
                    .map(|x| self.name(x.get().to_u32()))
                    .collect();
                alternates.insert(g, alts);
            }
            for (k, alts) in alternates.items {
                let kind = RuleKind::Substitution {
                    input: vec![single(k)],
                    replacement: vec![alts],
                    precontext: vec![],
                    postcontext: vec![],
                    reverse: false,
                    force_alt: true,
                };
                b.rules.push(self.rule(kind));
            }
        }
        Ok(b)
    }

    fn ligature_subst(
        &mut self,
        subs: Vec<LigatureSubst>,
        flag: u16,
        mfs: Option<u16>,
    ) -> Result<Routine, Error> {
        let name = format!("LigatureSubstitution{}", self.gensym());
        let mut b = self.routine(name, flag, mfs)?;
        for sub in subs {
            let LigatureSubst::Format1(t) = &sub else {
                return Err("unsupported LigatureSubst format".into());
            };
            let input = opt_coverage_glyphs(self.names, t.coverage())?;
            let sets = collect!(t.ligature_sets().iter());
            if input.len() != sets.len() {
                return Err("LigatureSubst coverage and LigatureSet counts differ".into());
            }
            let mut ligatures: ODict<String, Vec<(Vec<String>, String)>> = ODict::new();
            for (g, set) in input.into_iter().zip(sets) {
                let ligs = collect!(set.ligatures().iter());
                let ligs = ligs
                    .iter()
                    .map(|lig| {
                        let comps = lig
                            .component_glyph_ids()
                            .iter()
                            .map(|x| self.name(x.get().to_u32()))
                            .collect();
                        (comps, self.name(lig.ligature_glyph().to_u32()))
                    })
                    .collect();
                ligatures.insert(g, ligs);
            }
            for (first, ligs) in ligatures.items {
                for (comps, lig) in ligs {
                    let mut input = vec![single(first.clone())];
                    input.extend(comps.into_iter().map(single));
                    let kind = RuleKind::Substitution {
                        input,
                        replacement: vec![single(lig)],
                        precontext: vec![],
                        postcontext: vec![],
                        reverse: false,
                        force_alt: false,
                    };
                    b.rules.push(self.rule(kind));
                }
            }
        }
        Ok(b)
    }

    fn reverse_subst(
        &mut self,
        subs: Vec<ReverseChainSingleSubst>,
        flag: u16,
        mfs: Option<u16>,
    ) -> Result<Routine, Error> {
        let name = format!("ReverseContextualSubstitution{}", self.gensym());
        let mut b = self.routine(name, flag, mfs)?;
        for sub in subs {
            let ReverseChainSingleSubst::Format1(t) = &sub else {
                return Err("unsupported ReverseChainSingleSubst format".into());
            };
            let mut prefix = Vec::new();
            for cov in collect!(t.backtrack_coverages().iter()).iter().rev() {
                prefix.push(coverage_glyphs(self.names, cov)?);
            }
            let mut suffix = Vec::new();
            for cov in collect!(t.lookahead_coverages().iter()).iter() {
                suffix.push(coverage_glyphs(self.names, cov)?);
            }
            let outputs: Vec<String> = t
                .substitute_glyph_ids()
                .iter()
                .map(|x| self.name(x.get().to_u32()))
                .collect();
            let inputs = opt_coverage_glyphs(self.names, t.coverage())?;
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

    fn single_pos(
        &mut self,
        subs: Vec<SinglePos>,
        flag: u16,
        mfs: Option<u16>,
    ) -> Result<Routine, Error> {
        let name = format!("SinglePositioning{}", self.gensym());
        let mut b = self.routine(name, flag, mfs)?;
        for sub in subs {
            match &sub {
                SinglePos::Format1(t) => {
                    let glyphs = opt_coverage_glyphs(self.names, t.coverage())?;
                    let kind = RuleKind::Positioning {
                        glyphs: vec![glyphs],
                        valuerecords: vec![value_record(&t.value_record())],
                    };
                    b.rules.push(self.rule(kind));
                }
                SinglePos::Format2(t) => {
                    let glyphs = opt_coverage_glyphs(self.names, t.coverage())?;
                    let values = collect!(t.value_records().iter());
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

    fn pair_pos(
        &mut self,
        subs: Vec<PairPos>,
        flag: u16,
        mfs: Option<u16>,
    ) -> Result<Routine, Error> {
        let name = format!("PairPositioning{}", self.gensym());
        let mut b = self.routine(name, flag, mfs)?;
        for sub in subs {
            match &sub {
                PairPos::Format1(t) => {
                    let glyphs = opt_coverage_glyphs(self.names, t.coverage())?;
                    let sets = collect!(t.pair_sets().iter());
                    for (g, set) in glyphs.into_iter().zip(sets) {
                        for vr in collect!(set.pair_value_records().iter()) {
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
                    let cd1 = class_defs(self.names, &t.class_def1().map_err(read_err)?)?;
                    let cd2 = class_defs(self.names, &t.class_def2().map_err(read_err)?)?;
                    let class1 = invert_class_def(self.names, &cd1);
                    let class2 = invert_class_def(self.names, &cd2);
                    let coverage: HashSet<String> = opt_coverage_glyphs(self.names, t.coverage())?
                        .into_iter()
                        .collect();
                    for (ix1, c1) in t.class1_records().iter().enumerate() {
                        let c1 = c1.map_err(read_err)?;
                        let Some(first_all) = class1.get(&(ix1 as u16)) else {
                            continue;
                        };
                        for (ix2, c2) in c1.class2_records().iter().enumerate() {
                            let c2 = c2.map_err(read_err)?;
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

    fn cursive_pos(
        &mut self,
        subs: Vec<CursivePos>,
        flag: u16,
        mfs: Option<u16>,
    ) -> Result<Routine, Error> {
        let name = format!("CursiveAttachment{}", self.gensym());
        let mut b = self.routine(name, flag, mfs)?;
        let mut entries: ODict<String, (i32, i32)> = ODict::new();
        let mut exits: ODict<String, (i32, i32)> = ODict::new();
        for sub in subs {
            let CursivePos::Format1(t) = &sub else {
                return Err("unsupported CursivePos format".into());
            };
            let glyphs = opt_coverage_glyphs(self.names, t.coverage())?;
            for (g, rec) in glyphs.into_iter().zip(t.entry_exit_record()) {
                if let Some(a) = rec.entry_anchor(t.offset_data()) {
                    entries.insert(g.clone(), anchor_xy(&a.map_err(read_err)?));
                }
                if let Some(a) = rec.exit_anchor(t.offset_data()) {
                    exits.insert(g, anchor_xy(&a.map_err(read_err)?));
                }
            }
        }
        let att = Attachment {
            base_name: "cursive_entry".into(),
            mark_name: "cursive_exit".into(),
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
    ) -> Result<ODict<String, (i32, i32)>, Error> {
        let mut marks = ODict::new();
        for (i, rec) in array.mark_records().iter().enumerate() {
            if rec.mark_class() == class_id {
                let anchor = rec.mark_anchor(array.offset_data()).map_err(read_err)?;
                let g = coverage.get(i).ok_or("mark coverage too short")?;
                marks.insert(g.clone(), anchor_xy(&anchor));
            }
        }
        Ok(marks)
    }

    fn mark_to_base(
        &mut self,
        subs: Vec<MarkBasePos>,
        flag: u16,
        mfs: Option<u16>,
    ) -> Result<Routine, Error> {
        let name = format!("MarkToBase{}", self.gensym());
        let mut b = self.routine(name, flag, mfs)?;
        for sub in subs {
            let MarkBasePos::Format1(t) = &sub else {
                return Err("unsupported MarkBasePos format".into());
            };
            let mark_cov = opt_coverage_glyphs(self.names, t.mark_coverage())?;
            let base_cov = opt_coverage_glyphs(self.names, t.base_coverage())?;
            let mark_array = t.mark_array().map_err(read_err)?;
            let base_array = t.base_array().map_err(read_err)?;
            let base_records = collect!(base_array.base_records().iter());
            for class_id in 0..t.mark_class_count() {
                let prefix = format!("Anchor{}", self.gensym());
                let marks = self.mark_array(&mark_array, &mark_cov, class_id)?;
                let mut bases = ODict::new();
                for (i, rec) in base_records.iter().enumerate() {
                    for (c, anchor) in rec
                        .base_anchors(base_array.offset_data())
                        .iter()
                        .enumerate()
                    {
                        if c != class_id as usize {
                            continue;
                        }
                        let Some(anchor) = anchor else { continue };
                        let anchor = anchor.map_err(read_err)?;
                        let g = base_cov.get(i).ok_or("base coverage too short")?;
                        bases.insert(g.clone(), anchor_xy(&anchor));
                    }
                }
                let att = Attachment {
                    fullname: format!("{prefix}__{prefix}"),
                    mark_name: format!("_{prefix}"),
                    base_name: prefix,
                    bases,
                    marks,
                    has_font: true,
                    force_markmark: false,
                };
                b.rules.push(self.rule(RuleKind::Attachment(att)));
            }
        }
        Ok(b)
    }

    fn mark_to_mark(
        &mut self,
        subs: Vec<MarkMarkPos>,
        flag: u16,
        mfs: Option<u16>,
    ) -> Result<Routine, Error> {
        let name = format!("MarkToMark{}", self.gensym());
        let mut b = self.routine(name, flag, mfs)?;
        for sub in subs {
            let MarkMarkPos::Format1(t) = &sub else {
                return Err("unsupported MarkMarkPos format".into());
            };
            let mark1_cov = opt_coverage_glyphs(self.names, t.mark1_coverage())?;
            let mark2_cov = opt_coverage_glyphs(self.names, t.mark2_coverage())?;
            let mark1_array = t.mark1_array().map_err(read_err)?;
            let mark2_array = t.mark2_array().map_err(read_err)?;
            let mark2_records = collect!(mark2_array.mark2_records().iter());
            for class_id in 0..t.mark_class_count() {
                let prefix = format!("Anchor{}", self.gensym());
                let marks = self.mark_array(&mark1_array, &mark1_cov, class_id)?;
                let mut bases = ODict::new();
                for (i, rec) in mark2_records.iter().enumerate() {
                    let anchor = rec
                        .mark2_anchors(mark2_array.offset_data())
                        .get(class_id as usize)
                        .map(|a| a.map_err(read_err))
                        .transpose()?;
                    let Some(anchor) = anchor else { continue };
                    let g = mark2_cov.get(i).ok_or("mark2 coverage too short")?;
                    bases.insert(g.clone(), anchor_xy(&anchor));
                }
                let att = Attachment {
                    fullname: format!("{prefix}_{prefix}_"),
                    mark_name: format!("{prefix}_"),
                    base_name: prefix,
                    bases,
                    marks,
                    has_font: true,
                    force_markmark: true,
                };
                b.rules.push(self.rule(RuleKind::Attachment(att)));
            }
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

    fn names_of(
        &self,
        gids: &[read_fonts::types::BigEndian<read_fonts::types::GlyphId16>],
    ) -> Vec<Slot> {
        gids.iter()
            .map(|g| single(self.name(g.get().to_u32())))
            .collect()
    }

    fn contextual(
        &mut self,
        subs: Vec<SequenceContext>,
        flag: u16,
        mfs: Option<u16>,
    ) -> Result<Routine, Error> {
        let sym = self.gensym();
        let name = format!("Contextual{}{}", self.table, sym);
        let mut b = self.routine(name, flag, mfs)?;
        for sub in subs {
            match &sub {
                SequenceContext::Format1(t) => {
                    let cov = opt_coverage_glyphs(self.names, t.coverage())?;
                    for (ruleset, first) in t.seq_rule_sets().iter().zip(cov) {
                        let ruleset = ruleset.ok_or("null SubRuleSet")?.map_err(read_err)?;
                        for rule in collect!(ruleset.seq_rules().iter()) {
                            let mut input = vec![single(first.clone())];
                            input.extend(self.names_of(rule.input_sequence()));
                            let r =
                                self.chain_rule(input, vec![], vec![], rule.seq_lookup_records())?;
                            b.rules.push(r);
                        }
                    }
                }
                SequenceContext::Format2(t) => {
                    let cov: HashSet<String> = opt_coverage_glyphs(self.names, t.coverage())?
                        .into_iter()
                        .collect();
                    let inputs = invert_class_def(
                        self.names,
                        &class_defs(self.names, &t.class_def().map_err(read_err)?)?,
                    );
                    for (class_id, ruleset) in t.class_seq_rule_sets().iter().enumerate() {
                        let Some(ruleset) = ruleset else { continue };
                        let ruleset = ruleset.map_err(read_err)?;
                        let inputclass = input_class(&inputs, class_id, &cov);
                        for rule in collect!(ruleset.class_seq_rules().iter()) {
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
                    let mut input = Vec::new();
                    for cov in collect!(t.coverages().iter()).iter() {
                        input.push(coverage_glyphs(self.names, cov)?);
                    }
                    let r = self.chain_rule(input, vec![], vec![], t.seq_lookup_records())?;
                    b.rules.push(r);
                }
                _ => return Err("unsupported SequenceContext format".into()),
            }
        }
        Ok(b)
    }

    fn chained_contextual(
        &mut self,
        subs: Vec<ChainedSequenceContext>,
        flag: u16,
        mfs: Option<u16>,
    ) -> Result<Routine, Error> {
        let sym = self.gensym();
        let name = format!("ChainedContextual{}{}", self.table, sym);
        let mut b = self.routine(name, flag, mfs)?;
        for sub in subs {
            match &sub {
                ChainedSequenceContext::Format1(t) => {
                    let cov = opt_coverage_glyphs(self.names, t.coverage())?;
                    for (ruleset, first) in t.chained_seq_rule_sets().iter().zip(cov) {
                        let ruleset = ruleset.ok_or("null ChainSubRuleSet")?.map_err(read_err)?;
                        for rule in collect!(ruleset.chained_seq_rules().iter()) {
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
                        ODict::new()
                    } else {
                        invert_class_def(
                            self.names,
                            &class_defs(self.names, &t.backtrack_class_def().map_err(read_err)?)?,
                        )
                    };
                    let lookahead = if t.lookahead_class_def_offset().is_null() {
                        ODict::new()
                    } else {
                        invert_class_def(
                            self.names,
                            &class_defs(self.names, &t.lookahead_class_def().map_err(read_err)?)?,
                        )
                    };
                    let inputs = invert_class_def(
                        self.names,
                        &class_defs(self.names, &t.input_class_def().map_err(read_err)?)?,
                    );
                    let cov: HashSet<String> = opt_coverage_glyphs(self.names, t.coverage())?
                        .into_iter()
                        .collect();
                    for (class_id, ruleset) in t.chained_class_seq_rule_sets().iter().enumerate() {
                        let Some(ruleset) = ruleset else { continue };
                        let ruleset = ruleset.map_err(read_err)?;
                        let inputclass = input_class(&inputs, class_id, &cov);
                        for rule in collect!(ruleset.chained_class_seq_rules().iter()) {
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
                    let mut prefix = Vec::new();
                    for cov in collect!(t.backtrack_coverages().iter()).iter().rev() {
                        prefix.push(coverage_glyphs(self.names, cov)?);
                    }
                    let mut suffix = Vec::new();
                    for cov in collect!(t.lookahead_coverages().iter()).iter() {
                        suffix.push(coverage_glyphs(self.names, cov)?);
                    }
                    let mut input = Vec::new();
                    for cov in collect!(t.input_coverages().iter()).iter() {
                        input.push(coverage_glyphs(self.names, cov)?);
                    }
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
    inputs: &ODict<u16, Vec<String>>,
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

fn class_of(classes: &ODict<u16, Vec<String>>, c: u16) -> Result<Vec<String>, Error> {
    classes
        .get(&c)
        .cloned()
        .ok_or_else(|| format!("class {c} is not defined in the ClassDef").into())
}

/// The glyph IDs of a coverage table, in the order of `coverage_glyphs`.
fn coverage_gids(cov: &read_fonts::tables::layout::CoverageTable) -> Result<Vec<u32>, Error> {
    use read_fonts::tables::layout::CoverageTable;
    match cov {
        CoverageTable::Format1(t) => Ok(t.glyph_array().iter().map(|g| g.get().to_u32()).collect()),
        CoverageTable::Format2(t) => {
            let mut ranges: Vec<_> = t.range_records().iter().collect();
            ranges.sort_by_key(|r| r.start_coverage_index());
            let mut gids = Vec::new();
            for r in ranges {
                gids.extend(r.start_glyph_id().to_u32()..r.end_glyph_id().to_u32() + 1);
            }
            Ok(gids)
        }
        other => Err(format!("unsupported Coverage format {}", other.coverage_format()).into()),
    }
}
