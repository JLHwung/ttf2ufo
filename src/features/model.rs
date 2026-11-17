//! fontFeatures' data model (`fontFeatures/__init__.py`), reduced to what the
//! binary unparser produces.
//!
//! Python object identity is modelled with indices: routines live in an
//! arena (`FontFeatures::arena`) and `FontFeatures::routines` is the ordered
//! list of arena indices that Python keeps in `ff.routines`.

use std::collections::HashMap;
use std::hash::Hash;

/// An insertion-ordered dict with Python's semantics: re-assigning an
/// existing key keeps its original position.
#[derive(Clone, Debug)]
pub struct ODict<K, V> {
    pub items: Vec<(K, V)>,
    index: HashMap<K, usize>,
}

impl<K: Clone + Eq + Hash, V> Default for ODict<K, V> {
    fn default() -> Self {
        ODict {
            items: Vec::new(),
            index: HashMap::new(),
        }
    }
}

impl<K: Clone + Eq + Hash, V> ODict<K, V> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, key: K, value: V) {
        if let Some(&i) = self.index.get(&key) {
            self.items[i].1 = value;
        } else {
            self.index.insert(key.clone(), self.items.len());
            self.items.push((key, value));
        }
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.index.get(key).map(|&i| &self.items[i].1)
    }

    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        match self.index.get(key) {
            Some(&i) => Some(&mut self.items[i].1),
            None => None,
        }
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.index.contains_key(key)
    }
}

/// One glyph-stream position: a list of glyph names.
pub type Slot = Vec<String>;

/// A `(script, language)` pair.
pub type LangPair = (String, String);

/// `fontFeatures.RoutineReference` as used inside chaining rules. The name
/// is captured when the reference is created (and filled in by `resolve`
/// when empty), so it can go stale if the routine is renamed later.
#[derive(Clone, Debug)]
pub struct Ref {
    pub routine: usize,
    pub name: String,
}

/// `fontFeatures.RoutineReference` as stored in `ff.features`.
#[derive(Clone, Debug)]
pub struct FeatRef {
    pub routine: usize,
    pub languages: Vec<LangPair>,
}

/// `fontFeatures.ValueRecord` as built by `GPOSUnparser.makeValueRecord`
/// (zero values are stored as `None`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValueRecord {
    pub x_placement: Option<i32>,
    pub y_placement: Option<i32>,
    pub x_advance: Option<i32>,
    pub y_advance: Option<i32>,
}

impl ValueRecord {
    /// `ValueRecord.__bool__`
    pub fn truthy(&self) -> bool {
        self.x_placement.is_some()
            || self.y_placement.is_some()
            || self.x_advance.is_some()
            || self.y_advance.is_some()
    }
}

#[derive(Clone, Debug)]
pub struct Attachment {
    pub base_name: String,
    #[allow(dead_code)]
    pub mark_name: String,
    pub fullname: String,
    pub bases: ODict<String, (i32, i32)>,
    pub marks: ODict<String, (i32, i32)>,
    /// Whether the rule was given `font=` (mark-to-base / mark-to-mark).
    pub has_font: bool,
    pub force_markmark: bool,
}

impl Attachment {
    pub fn is_cursive(&self) -> bool {
        self.base_name == "cursive_entry" || self.base_name == "entry"
    }
}

#[derive(Clone, Debug)]
pub enum RuleKind {
    Substitution {
        input: Vec<Slot>,
        replacement: Vec<Slot>,
        precontext: Vec<Slot>,
        postcontext: Vec<Slot>,
        reverse: bool,
        force_alt: bool,
    },
    Positioning {
        glyphs: Vec<Slot>,
        valuerecords: Vec<ValueRecord>,
    },
    Attachment(Attachment),
    Chaining {
        input: Vec<Slot>,
        precontext: Vec<Slot>,
        postcontext: Vec<Slot>,
        lookups: Vec<Option<Vec<Ref>>>,
        note: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub kind: RuleKind,
    /// `rule.address`: the lookup index, or `None`.
    pub address: Option<usize>,
}

impl Rule {
    /// `type(rule).__name__`
    pub fn type_name(&self) -> &'static str {
        match self.kind {
            RuleKind::Substitution { .. } => "Substitution",
            RuleKind::Positioning { .. } => "Positioning",
            RuleKind::Attachment(_) => "Attachment",
            RuleKind::Chaining { .. } => "Chaining",
        }
    }

    pub fn is_chaining(&self) -> bool {
        matches!(self.kind, RuleKind::Chaining { .. })
    }

    /// `fontFeatures.feaLib.Routine.lookup_type`
    pub fn fea_lookup_type(&self) -> u32 {
        match &self.kind {
            RuleKind::Substitution { .. } => sub_lookup_type(self).unwrap_or(0),
            RuleKind::Positioning { glyphs, .. } => pos_lookup_type(glyphs),
            RuleKind::Attachment(a) => a.is_cursive() as u32,
            RuleKind::Chaining { .. } => 1,
        }
    }
}

/// `fontFeatures.ttLib.Substitution.lookup_type`. The unparser never
/// produces substitutions with chained lookups, and its only rules with
/// context are reverse ones, so `forFea` makes no difference here.
pub fn sub_lookup_type(rule: &Rule) -> Option<u32> {
    let RuleKind::Substitution {
        input,
        replacement,
        precontext,
        postcontext,
        reverse,
        force_alt,
    } = &rule.kind
    else {
        return None;
    };
    if *reverse {
        return Some(8);
    }
    if replacement.is_empty() {
        return Some(2);
    }
    if !precontext.is_empty() || !postcontext.is_empty() {
        return Some(6);
    }
    if input.len() == 1 && replacement.len() == 1 {
        if *force_alt || (input[0].len() == 1 && replacement[0].len() > 1) {
            return Some(3);
        }
        return Some(1);
    }
    if input.len() > 1 && replacement.len() == 1 {
        return Some(4);
    }
    if input.len() > 1 && replacement.len() > 1 {
        return Some(9);
    }
    if replacement.len() > 1 {
        return Some(2);
    }
    None
}

/// `fontFeatures.ttLib.Positioning.lookup_type` (the unparser never adds
/// context to positioning rules).
pub fn pos_lookup_type(glyphs: &[Slot]) -> u32 {
    match glyphs.len() {
        1 => 1,
        2 => 2,
        _ => 8,
    }
}

#[derive(Clone, Debug, Default)]
pub struct Routine {
    pub name: String,
    pub rules: Vec<Rule>,
    pub flags: u16,
    pub languages: Vec<LangPair>,
    pub mark_filtering_set: Option<Vec<String>>,
    pub mark_attachment_set: Option<Vec<String>>,
    pub usecount: i64,
    pub mark_filtering_set_as_class: Option<String>,
    pub mark_attachment_set_as_class: Option<String>,
}

#[derive(Debug, Default)]
pub struct FontFeatures {
    /// Every routine ever created.
    pub arena: Vec<Routine>,
    /// `ff.routines`: arena indices, in order.
    pub routines: Vec<usize>,
    /// `ff.features`: feature tag -> references, in insertion order.
    pub features: Vec<(String, Vec<FeatRef>)>,
}

impl FontFeatures {
    pub fn new_routine(&mut self, routine: Routine) -> usize {
        self.arena.push(routine);
        self.arena.len() - 1
    }

    pub fn feature_mut(&mut self, tag: &str) -> &mut Vec<FeatRef> {
        let i = match self.features.iter().position(|(t, _)| t == tag) {
            Some(i) => i,
            None => {
                self.features.push((tag.to_string(), Vec::new()));
                self.features.len() - 1
            }
        };
        &mut self.features[i].1
    }

    /// `FontFeatures.resolveAllRoutines` / `RoutineReference.resolve`: give
    /// nameless chain references the current name of their routine.
    pub fn resolve_all_routines(&mut self) {
        let names: Vec<String> = self.arena.iter().map(|r| r.name.clone()).collect();
        for &ri in &self.routines {
            for rule in &mut self.arena[ri].rules {
                if let RuleKind::Chaining { lookups, .. } = &mut rule.kind {
                    for list in lookups.iter_mut() {
                        if let Some(refs) = list {
                            if refs.is_empty() {
                                *list = None;
                                continue;
                            }
                            for r in refs.iter_mut() {
                                if r.name.is_empty() {
                                    r.name = names[r.routine].clone();
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// `Routine.stage` (`None` for an empty routine).
    pub fn routine_stage(
        &self,
        routine: usize,
        depth: usize,
    ) -> Result<Option<&'static str>, crate::Error> {
        if depth > 900 {
            return Err("maximum recursion depth exceeded computing a routine's stage".into());
        }
        let Some(rule) = self.arena[routine].rules.first() else {
            return Ok(None);
        };
        match &rule.kind {
            RuleKind::Substitution { .. } => Ok(Some("sub")),
            RuleKind::Positioning { .. } | RuleKind::Attachment(_) => Ok(Some("pos")),
            RuleKind::Chaining { lookups, .. } => self.chain_stage(lookups, depth + 1),
        }
    }

    /// `Chaining.stage`
    pub fn chain_stage(
        &self,
        lookups: &[Option<Vec<Ref>>],
        depth: usize,
    ) -> Result<Option<&'static str>, crate::Error> {
        for list in lookups.iter().flatten() {
            if let Some(r) = list.first() {
                return self.routine_stage(r.routine, depth);
            }
        }
        Ok(None)
    }
}
