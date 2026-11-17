//! Port of `FontFeatures.asFeaAST()` (fontFeatures/feaLib) together with the
//! `asFea()` serialisation of the fontTools.feaLib.ast nodes it builds.

use std::collections::{HashMap, HashSet};

use super::model::{
    Attachment, FeatRef, FontFeatures, Ref, Routine, Rule, RuleKind, Slot, ValueRecord,
    sub_lookup_type,
};
use super::tables::GdefInfo;
use crate::Error;

const SHIFT: &str = "    ";

/// `fontTools.feaLib.ast.fea_keywords`
const FEA_KEYWORDS: &[&str] = &[
    "anchor",
    "anchordef",
    "anon",
    "anonymous",
    "by",
    "contour",
    "cursive",
    "device",
    "enum",
    "enumerate",
    "excludedflt",
    "exclude_dflt",
    "feature",
    "from",
    "ignore",
    "ignorebaseglyphs",
    "ignoreligatures",
    "ignoremarks",
    "include",
    "includedflt",
    "include_dflt",
    "language",
    "languagesystem",
    "lookup",
    "lookupflag",
    "mark",
    "markattachmenttype",
    "markclass",
    "nameid",
    "null",
    "parameters",
    "pos",
    "position",
    "required",
    "righttoleft",
    "reversesub",
    "rsub",
    "script",
    "sub",
    "substitute",
    "subtable",
    "table",
    "usemarkfilteringset",
    "useextension",
    "valuerecorddef",
    "base",
    "gdef",
    "head",
    "hhea",
    "name",
    "vhea",
    "vmtx",
];

/// `ast.asFea(glyphname)`: escape glyph names that are keywords.
fn fea_glyph(g: &str) -> String {
    if FEA_KEYWORDS.contains(&g.to_lowercase().as_str()) {
        format!("\\{g}")
    } else {
        g.to_string()
    }
}

/// `GlyphClass(...).asFea()`
fn glyph_class(glyphs: &[String]) -> String {
    let names: Vec<String> = glyphs.iter().map(|g| fea_glyph(g)).collect();
    format!("[{}]", names.join(" "))
}

/// `glyphref` of feaLib/Substitution, Chaining and Attachment: a glyph name
/// for one glyph, an (unsorted) class otherwise.
fn glyphref(g: &[String]) -> String {
    if g.len() == 1 {
        fea_glyph(&g[0])
    } else {
        glyph_class(g)
    }
}

/// `glyphref` of feaLib/Positioning: classes are sorted.
fn glyphref_sorted(g: &[String]) -> String {
    if g.len() == 1 {
        fea_glyph(&g[0])
    } else {
        let mut sorted = g.to_vec();
        sorted.sort();
        glyph_class(&sorted)
    }
}

fn join_refs(slots: &[Slot]) -> String {
    slots
        .iter()
        .map(|s| glyphref(s))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `ast.ValueRecord.asFea()` for a horizontal record without devices.
fn value_record(vr: &ValueRecord) -> String {
    if !vr.truthy() {
        return "<NULL>".to_string();
    }
    if vr.x_placement.is_none() && vr.y_placement.is_none() && vr.y_advance.is_none() {
        // Format A; xAdvance can't be None here since the record is truthy.
        return match vr.x_advance {
            Some(v) => v.to_string(),
            None => "None".to_string(),
        };
    }
    let z = |v: Option<i32>| v.unwrap_or(0);
    format!(
        "<{} {} {} {}>",
        z(vr.x_placement),
        z(vr.y_placement),
        z(vr.x_advance),
        z(vr.y_advance)
    )
}

fn anchor(pos: (i32, i32)) -> String {
    format!("<anchor {} {}>", pos.0, pos.1)
}

/// The subset of fontTools.feaLib.ast nodes whose `asFea` depends on the
/// indent; everything else is pre-rendered text.
enum Stmt {
    Text(String),
    Block(Vec<Stmt>),
    /// `MarkBasePosStatement` / `MarkMarkPosStatement` with one mark.
    MarkPos {
        kind: &'static str,
        glyphs: String,
        anchor: String,
        class: String,
    },
    Lookup {
        name: String,
        body: Vec<Stmt>,
    },
    Feature {
        name: String,
        body: Vec<Stmt>,
    },
}

impl Stmt {
    fn fea(&self, indent: &str) -> String {
        match self {
            Stmt::Text(t) => t.clone(),
            Stmt::Block(stmts) => block_fea(stmts, indent),
            Stmt::MarkPos {
                kind,
                glyphs,
                anchor,
                class,
            } => {
                format!("pos {kind} {glyphs}\n{indent}{SHIFT}{anchor} mark @{class};")
            }
            Stmt::Lookup { name, body } => {
                format!(
                    "lookup {name} {{\n{}{indent}}} {name};\n",
                    block_fea(body, indent)
                )
            }
            Stmt::Feature { name, body } => {
                let name = name.trim();
                format!(
                    "{indent}feature {name} {{\n{}{indent}}} {name};\n",
                    block_fea(body, indent)
                )
            }
        }
    }
}

/// `Block.asFea`
fn block_fea(stmts: &[Stmt], indent: &str) -> String {
    let indent = format!("{indent}{SHIFT}");
    let parts: Vec<String> = stmts.iter().map(|s| s.fea(&indent)).collect();
    format!("{indent}{}\n", parts.join(&format!("\n{indent}")))
}

/// Python's `str.isspace()` for one character (what `\s` matches).
fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// `re.search(r"\[\s*\]", fea)`
fn has_empty_class(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '[' {
            let mut j = i + 1;
            while j < chars.len() && py_space(chars[j]) {
                j += 1;
            }
            if j < chars.len() && chars[j] == ']' {
                return true;
            }
        }
    }
    false
}

/// `bad_statement_to_comment`
fn bad_statement_to_comment(s: Stmt) -> Stmt {
    let fea = s.fea("");
    if has_empty_class(&fea) {
        Stmt::Text(format!("# {fea}"))
    } else {
        s
    }
}

/// `sortByAnchor`: group glyphs by anchor position, in first-seen order.
fn sort_by_anchor(items: &[(String, (i32, i32))]) -> Vec<(Vec<String>, (i32, i32))> {
    let mut groups: Vec<(Vec<String>, (i32, i32))> = Vec::new();
    let mut index: HashMap<(i32, i32), usize> = HashMap::new();
    for (name, pos) in items {
        match index.get(pos) {
            Some(&i) => groups[i].0.push(name.clone()),
            None => {
                index.insert(*pos, groups.len());
                groups.push((vec![name.clone()], *pos));
            }
        }
    }
    groups
}

/// State that fontFeatures keeps in `ff.namedClasses`, `ff.scratch`,
/// `ff.symbols` and the module-global counter of feaLib/Routine.py.
#[derive(Default)]
struct FeaState {
    named_classes: Vec<(String, Vec<String>)>,
    named_by_sorted: HashMap<Vec<String>, String>,
    /// `ff.scratch["index"]` (Chaining.gensym)
    scratch_index: usize,
    /// `ff.scratch["mark_classes_done"]`
    mark_classes_done: HashSet<(String, Vec<String>)>,
    /// feaLib/Routine.py's global `counter`. It is per process in Python;
    /// extractufo converts one font per call, so it starts at 0 here.
    routine_counter: usize,
    /// `ff.symbols`
    symbols: HashMap<String, usize>,
}

impl FeaState {
    /// `FontFeatures.getNamedClassFor`
    fn get_named_class_for(&mut self, glyphs: Vec<String>, name: String) -> String {
        let mut sorted = glyphs.clone();
        sorted.sort();
        if let Some(k) = self.named_by_sorted.get(&sorted) {
            return k.clone();
        }
        self.named_by_sorted.insert(sorted, name.clone());
        self.named_classes.push((name.clone(), glyphs));
        name
    }

    /// Chaining.gensym
    fn chain_gensym(&mut self) -> String {
        self.scratch_index += 1;
        self.scratch_index.to_string()
    }

    /// feaLib/Routine.gensym
    fn routine_gensym(&mut self, prefix: &str) -> String {
        self.routine_counter += 1;
        format!("{prefix}{}", self.routine_counter)
    }

    /// `FontFeatures.gensym`
    fn ff_gensym(&mut self, category: &str) -> String {
        let n = self.symbols.entry(category.to_string()).or_insert(0);
        *n += 1;
        format!("{category}{n}")
    }

    /// `replaceLongWithClasses`
    fn replace_long_with_classes(&mut self, slots: &mut [Slot]) {
        for slot in slots.iter_mut() {
            if slot.len() > 5 {
                let mut sorted = slot.clone();
                sorted.sort();
                let name = format!("class{}", self.chain_gensym());
                let classname = self.get_named_class_for(sorted, name);
                *slot = vec![format!("@{classname}")];
            }
        }
    }
}

/// `FontFeatures.partitionRoutine` keyed on `(languages, type(rule),
/// lookup_type(rule))`, with `replaceRoutineWithSplitList`. Rules made by the
/// unparser have no languages, so the key reduces to the type and lookup type.
fn partition_routine(ff: &mut FontFeatures, rid: usize) -> Result<Option<Vec<usize>>, Error> {
    if ff.arena[rid].rules.is_empty() {
        return Ok(None);
    }
    let rules = std::mem::take(&mut ff.arena[rid].rules);
    let mut groups: Vec<((&'static str, u32), Vec<Rule>)> = Vec::new();
    for rule in rules {
        let key = (rule.type_name(), rule.fea_lookup_type());
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(rule),
            None => groups.push((key, vec![rule])),
        }
    }
    let mut groups = groups.into_iter();
    let (_, first) = groups.next().unwrap();
    ff.arena[rid].rules = first;
    let mut all = vec![rid];
    for (_, rules) in groups {
        let copy = Routine {
            rules,
            ..ff.arena[rid].clone_without_rules()
        };
        all.push(ff.new_routine(copy));
    }

    // replaceRoutineWithSplitList
    let index = ff
        .routines
        .iter()
        .position(|&r| r == rid)
        .ok_or("routine is not in the routine list")?;
    ff.routines.splice(index..index + 1, all.iter().copied());
    let new_refs: Vec<Ref> = all
        .iter()
        .map(|&r| Ref {
            routine: r,
            name: ff.arena[r].name.clone(),
        })
        .collect();
    let order = ff.routines.clone();
    for r in order {
        for rule in ff.arena[r].rules.iter_mut() {
            let RuleKind::Chaining { lookups, .. } = &mut rule.kind else {
                continue;
            };
            for list in lookups.iter_mut().flatten() {
                let mut i = 0;
                while i < list.len() {
                    if list[i].routine == rid {
                        list.splice(i..i + 1, new_refs.iter().cloned());
                    }
                    i += 1;
                }
            }
        }
    }
    for (_, list) in ff.features.iter_mut() {
        let mut i = 0;
        while i < list.len() {
            if list[i].routine == rid {
                list.splice(
                    i..i + 1,
                    all.iter().map(|&r| FeatRef {
                        routine: r,
                        languages: Vec::new(),
                    }),
                );
            }
            i += 1;
        }
    }
    Ok(Some(all))
}

impl Routine {
    fn clone_without_rules(&self) -> Routine {
        Routine {
            name: self.name.clone(),
            rules: Vec::new(),
            flags: self.flags,
            languages: self.languages.clone(),
            mark_filtering_set: self.mark_filtering_set.clone(),
            mark_attachment_set: self.mark_attachment_set.clone(),
            usecount: self.usecount,
            mark_filtering_set_as_class: self.mark_filtering_set_as_class.clone(),
            mark_attachment_set_as_class: self.mark_attachment_set_as_class.clone(),
        }
    }
}

/// `reorderAndResolve`
fn reorder_and_resolve(ff: &mut FontFeatures) -> Result<Vec<usize>, Error> {
    ff.resolve_all_routines();
    let n = ff.routines.len();
    let mut ordering: Vec<usize> = (0..n).collect();
    let position: HashMap<usize, usize> = ff
        .routines
        .iter()
        .enumerate()
        .map(|(i, &r)| (r, i))
        .collect();
    for ptr in (0..n).rev() {
        let rid = ff.routines[ptr];
        if !ff.arena[rid].rules.iter().any(|r| r.is_chaining()) {
            continue;
        }
        let mut later = Vec::new();
        let mut referenced = Vec::new();
        for rule in &ff.arena[rid].rules {
            match &rule.kind {
                RuleKind::Chaining { lookups, .. } => {
                    for r in lookups.iter().flatten().flatten() {
                        referenced.push(r.routine);
                        let ix = *position
                            .get(&r.routine)
                            .ok_or("chained routine is not in the routine list")?;
                        if ix > ptr {
                            later.push(ix);
                        }
                    }
                }
                RuleKind::Substitution { .. } => {}
                _ => return Err("a routine mixes chaining and positioning rules".into()),
            }
        }
        for r in referenced {
            ff.arena[r].usecount = 999;
        }
        for ix in later {
            let p = ordering.iter().position(|&o| o == ix).unwrap();
            let f = ordering.remove(p);
            ordering.insert(ptr, f);
        }
    }
    Ok(ordering.into_iter().map(|i| ff.routines[i]).collect())
}

struct Writer<'a> {
    ff: &'a mut FontFeatures,
    gdef: &'a GdefInfo,
    st: FeaState,
}

impl Writer<'_> {
    /// `Routine.feaPreamble` (with the rules' preambles)
    fn routine_preamble(&mut self, rid: usize) -> Result<Vec<Stmt>, Error> {
        let mut pre = Vec::new();
        let mut rules = std::mem::take(&mut self.ff.arena[rid].rules);
        for rule in rules.iter_mut() {
            match &mut rule.kind {
                RuleKind::Attachment(att) if !att.is_cursive() => {
                    let mut block = Vec::new();
                    for (glyphs, pos) in sort_by_anchor(&att.marks.items) {
                        let key = (att.fullname.clone(), glyphs.clone());
                        if !self.st.mark_classes_done.contains(&key) {
                            block.push(Stmt::Text(format!(
                                "markClass {} {} @{};",
                                glyphref(&glyphs),
                                anchor(pos),
                                att.fullname
                            )));
                            self.st.mark_classes_done.insert(key);
                        }
                    }
                    pre.push(Stmt::Block(block));
                }
                RuleKind::Chaining {
                    input,
                    precontext,
                    postcontext,
                    lookups,
                    ..
                } => {
                    self.st.replace_long_with_classes(input);
                    self.st.replace_long_with_classes(precontext);
                    self.st.replace_long_with_classes(postcontext);
                    for r in lookups.iter_mut().flatten().flatten() {
                        if self.ff.arena[r.routine].name.is_empty() {
                            let name = format!("ChainedRoutine{}", self.st.chain_gensym());
                            self.ff.arena[r.routine].name = name;
                        }
                        if r.name.is_empty() {
                            r.name = self.ff.arena[r.routine].name.clone();
                        }
                    }
                }
                _ => {}
            }
        }
        self.ff.arena[rid].rules = rules;

        let flags = self.ff.arena[rid].flags;
        if flags & 0xFF00 != 0 {
            let set = self.ff.arena[rid]
                .mark_attachment_set
                .clone()
                .ok_or("markAttachmentSet is None")?;
            let name = self.st.routine_gensym("markAttachmentSet");
            let cls = self.st.get_named_class_for(set, name);
            self.ff.arena[rid].mark_attachment_set_as_class = Some(cls);
        }
        if flags & 0x10 != 0 {
            let set = self.ff.arena[rid]
                .mark_filtering_set
                .clone()
                .ok_or("markFilteringSet is None")?;
            let name = self.st.routine_gensym("markFilteringSet");
            let cls = self.st.get_named_class_for(set, name);
            self.ff.arena[rid].mark_filtering_set_as_class = Some(cls);
        }
        Ok(pre)
    }

    /// `Routine.asFeaAST`
    fn routine_ast(&self, rid: usize) -> Result<Stmt, Error> {
        let routine = &self.ff.arena[rid];
        let mut body = Vec::new();
        body.push(Stmt::Text(lookupflag(routine)));
        body.push(Stmt::Text(";".to_string()));
        let mut lastaddress: Option<usize> = None;
        for rule in &routine.rules {
            if let Some(a) = rule.address
                && a != 0
                && Some(a) != lastaddress
            {
                body.push(Stmt::Text(format!("# Original source: {a} ")));
                lastaddress = Some(a);
            }
            if let RuleKind::Chaining {
                note: Some(note), ..
            } = &rule.kind
            {
                let lines: Vec<String> = note.split('\n').map(|n| format!("# {n}")).collect();
                body.push(Stmt::Text(lines.join("\n")));
            }
            body.push(bad_statement_to_comment(self.rule_ast(rule)?));
        }
        if routine.name.is_empty() {
            Ok(Stmt::Block(body))
        } else {
            Ok(Stmt::Lookup {
                name: routine.name.clone(),
                body,
            })
        }
    }

    /// `Rule.asFeaAST`
    fn rule_ast(&self, rule: &Rule) -> Result<Stmt, Error> {
        match &rule.kind {
            RuleKind::Substitution { .. } => substitution_ast(rule),
            RuleKind::Positioning {
                glyphs,
                valuerecords,
            } => {
                let refs: Vec<String> = glyphs.iter().map(|g| glyphref_sorted(g)).collect();
                if glyphs.len() == 1 {
                    // SinglePosStatement without context
                    Ok(Stmt::Text(format!(
                        "pos {} {};",
                        refs[0],
                        value_record(&valuerecords[0])
                    )))
                } else if glyphs.len() == 2 {
                    // PairPosStatement
                    if valuerecords[1].truthy() {
                        Ok(Stmt::Text(format!(
                            "pos {} {} {} {};",
                            refs[0],
                            value_record(&valuerecords[0]),
                            refs[1],
                            value_record(&valuerecords[1])
                        )))
                    } else {
                        Ok(Stmt::Text(format!(
                            "pos {} {} {};",
                            refs[0],
                            refs[1],
                            value_record(&valuerecords[0])
                        )))
                    }
                } else {
                    Err("positioning rule with more than two glyphs".into())
                }
            }
            RuleKind::Attachment(att) => self.attachment_ast(att),
            RuleKind::Chaining {
                input,
                precontext,
                postcontext,
                lookups,
                ..
            } => {
                if lookups.iter().any(|l| l.is_some()) {
                    let verb = match self.ff.chain_stage(lookups, 0)? {
                        Some("sub") => "sub",
                        _ => "pos",
                    };
                    // ChainContextSubstStatement / ChainContextPosStatement
                    let mut res = format!("{verb} ");
                    if !precontext.is_empty() {
                        res += &join_refs(precontext);
                        res += " ";
                    }
                    for (i, g) in input.iter().enumerate() {
                        res += &glyphref(g);
                        res += "'";
                        if let Some(Some(list)) = lookups.get(i) {
                            for lu in list {
                                res += " lookup ";
                                res += &lu.name;
                            }
                        }
                        if i + 1 < input.len() {
                            res += " ";
                        }
                    }
                    if !postcontext.is_empty() {
                        res += " ";
                        res += &join_refs(postcontext);
                    }
                    res += ";";
                    Ok(Stmt::Text(res))
                } else {
                    // IgnoreSubstStatement
                    let mut res = String::new();
                    if !precontext.is_empty() {
                        res += &join_refs(precontext);
                        res += " ";
                    }
                    res += &input
                        .iter()
                        .map(|g| format!("{}'", glyphref(g)))
                        .collect::<Vec<_>>()
                        .join(" ");
                    if !postcontext.is_empty() {
                        res += " ";
                        res += &join_refs(postcontext);
                    }
                    Ok(Stmt::Text(format!("ignore sub {res};")))
                }
            }
        }
    }

    /// `Attachment.asFeaAST`
    fn attachment_ast(&self, att: &Attachment) -> Result<Stmt, Error> {
        let mut block = Vec::new();
        if att.is_cursive() {
            // Python iterates `set(bases) | set(marks)`, whose order depends
            // on PYTHONHASHSEED; entries then exits-only glyphs are used here.
            let mut glyphs: Vec<&String> = att.bases.items.iter().map(|(g, _)| g).collect();
            for (g, _) in &att.marks.items {
                if !att.bases.contains_key(g) {
                    glyphs.push(g);
                }
            }
            for g in glyphs {
                let entry = att
                    .bases
                    .get(g)
                    .map(|p| anchor(*p))
                    .unwrap_or_else(|| "<anchor NULL>".into());
                let exit = att
                    .marks
                    .get(g)
                    .map(|p| anchor(*p))
                    .unwrap_or_else(|| "<anchor NULL>".into());
                block.push(Stmt::Text(format!(
                    "pos cursive {} {} {};",
                    glyphref(std::slice::from_ref(g)),
                    entry,
                    exit
                )));
            }
        } else {
            for (glyphs, pos) in sort_by_anchor(&att.bases.items) {
                let mut kind = "base";
                if att.has_font && self.gdef.is_mark(&glyphs[0])? {
                    kind = "mark";
                }
                if att.force_markmark {
                    kind = "mark";
                }
                block.push(Stmt::MarkPos {
                    kind,
                    glyphs: glyphref(&glyphs),
                    anchor: anchor(pos),
                    class: att.fullname.clone(),
                });
            }
        }
        Ok(Stmt::Block(block))
    }
}

/// `LookupFlagStatement.asFea`
fn lookupflag(routine: &Routine) -> String {
    let mut res: Vec<String> = Vec::new();
    let names = [
        "RightToLeft",
        "IgnoreBaseGlyphs",
        "IgnoreLigatures",
        "IgnoreMarks",
    ];
    for (i, name) in names.iter().enumerate() {
        if routine.flags & (1 << i) != 0 {
            res.push(name.to_string());
        }
    }
    if routine.flags & 0xFF00 != 0
        && let Some(c) = &routine.mark_attachment_set_as_class
    {
        res.push(format!("MarkAttachmentType @{c}"));
    }
    if routine.flags & 0x10 != 0
        && let Some(c) = &routine.mark_filtering_set_as_class
    {
        res.push(format!("UseMarkFilteringSet @{c}"));
    }
    if res.is_empty() {
        res.push("0".to_string());
    }
    format!("lookupflag {};", res.join(" "))
}

/// `Substitution.asFeaAST` for the shapes the unparser produces.
fn substitution_ast(rule: &Rule) -> Result<Stmt, Error> {
    let RuleKind::Substitution {
        input,
        replacement,
        precontext,
        postcontext,
        ..
    } = &rule.kind
    else {
        unreachable!()
    };
    let lut = sub_lookup_type(rule);
    let has_context = !precontext.is_empty() || !postcontext.is_empty();
    // The "glyph(s)'" part of a statement, with context if any.
    let target = |glyphs: Vec<String>| -> String {
        if has_context {
            let mut res = String::new();
            if !precontext.is_empty() {
                res += &join_refs(precontext);
                res += " ";
            }
            res += &glyphs
                .iter()
                .map(|g| format!("{g}'"))
                .collect::<Vec<_>>()
                .join(" ");
            if !postcontext.is_empty() {
                res += " ";
                res += &join_refs(postcontext);
            }
            res
        } else {
            glyphs.join(" ")
        }
    };
    let Some(lut) = lut else {
        return Ok(Stmt::Text(String::new()));
    };
    if replacement.is_empty() {
        // MultipleSubstStatement deleting the glyph
        return Ok(Stmt::Text(format!(
            "sub {} by NULL;",
            target(vec![glyphref(&input[0])])
        )));
    }
    match lut {
        1 => {
            // SingleSubstStatement
            let glyphs: Vec<String> = input.iter().map(|g| glyphref(g)).collect();
            Ok(Stmt::Text(format!(
                "sub {} by {};",
                target(glyphs),
                join_refs(replacement)
            )))
        }
        2 => {
            // is_paired() is false: every input slot has a single glyph.
            Ok(Stmt::Text(format!(
                "sub {} by {};",
                target(vec![glyphref(&input[0])]),
                join_refs(replacement)
            )))
        }
        3 => Ok(Stmt::Text(format!(
            "sub {} from {};",
            target(vec![glyphref(&input[0])]),
            glyph_class(&replacement[0])
        ))),
        4 => {
            // has_classes() is false: every slot has a single glyph.
            let glyphs: Vec<String> = input.iter().map(|g| glyphref(g)).collect();
            Ok(Stmt::Text(format!(
                "sub {} by {};",
                target(glyphs),
                glyphref(&replacement[0])
            )))
        }
        8 => {
            // ReverseChainSingleSubstStatement
            let glyphs: Vec<String> = input.iter().map(|g| glyphref(g)).collect();
            Ok(Stmt::Text(format!(
                "rsub {} by {};",
                target(glyphs),
                glyphref(&replacement[0])
            )))
        }
        other => Err(format!("cannot express GSUB lookup type {other} as a substitution").into()),
    }
}

/// `FontFeatures.asFea()` (`asFeaAST(do_gdef=True).asFea()`).
pub fn as_fea(ff: &mut FontFeatures, gdef: &GdefInfo) -> Result<String, Error> {
    let mut statements: Vec<Stmt> = Vec::new();

    // add_language_system_statements / hoist_languages
    let mut scripts: Vec<(String, Vec<String>)> = Vec::new();
    let mut count = 0;
    for &rid in &ff.routines {
        for (script, lang) in &ff.arena[rid].languages {
            let i = match scripts.iter().position(|(s, _)| s == script) {
                Some(i) => i,
                None => {
                    scripts.push((script.clone(), Vec::new()));
                    scripts.len() - 1
                }
            };
            if lang == "*" {
                continue;
            }
            if !scripts[i].1.contains(lang) {
                count += 1;
                scripts[i].1.push(lang.clone());
            }
        }
    }
    if count >= 2 {
        for (s, langs) in &scripts {
            for l in langs {
                statements.push(Stmt::Text(format!("languagesystem {} {};", s, l.trim())));
            }
        }
    }

    // add_gdef: the unparser never fills ff.glyphclasses.
    statements.push(Stmt::Text(
        "table GDEF {\n    GlyphClassDef [], [], [], [];\n} GDEF;\n".to_string(),
    ));

    // Partition routines by rule type.
    for fi in 0..ff.features.len() {
        let mut i = 0;
        while i < ff.features[fi].1.len() {
            let rid = ff.features[fi].1[i].routine;
            if let Some(parts) = partition_routine(ff, rid)?
                && !ff.arena[rid].name.is_empty()
                && parts.len() > 1
            {
                for p in parts {
                    let rule = &ff.arena[p].rules[0];
                    let suffix =
                        format!("DFLT_dflt_{}_{}", rule.type_name(), rule.fea_lookup_type());
                    ff.arena[p].name.push_str(&suffix);
                }
            }
            i += 1;
        }
    }

    for &rid in &ff.routines {
        ff.arena[rid].usecount = 0;
    }

    // Split references to routines with several languages.
    for fi in 0..ff.features.len() {
        let mut references = Vec::new();
        for reference in &ff.features[fi].1 {
            let langs = &ff.arena[reference.routine].languages;
            if langs.len() > 1 {
                for l in langs {
                    references.push(FeatRef {
                        routine: reference.routine,
                        languages: vec![l.clone()],
                    });
                }
            } else {
                references.push(FeatRef {
                    routine: reference.routine,
                    languages: langs.clone(),
                });
            }
        }
        ff.features[fi].1 = references;
    }

    let new_routines = reorder_and_resolve(ff)?;

    let mut w = Writer {
        ff,
        gdef,
        st: FeaState::default(),
    };

    // Preamble
    for &rid in &new_routines {
        if w.ff.arena[rid].name.is_empty() && w.ff.arena[rid].usecount != 1 {
            w.ff.arena[rid].name = w.st.ff_gensym("Routine_");
        }
        let pre = w.routine_preamble(rid)?;
        if !w.ff.arena[rid].rules.is_empty() {
            statements.extend(pre);
        }
    }

    for (name, glyphs) in &w.st.named_classes {
        statements.push(Stmt::Text(format!("@{name} = {};", glyph_class(glyphs))));
    }

    statements.push(Stmt::Text(String::new()));

    for &rid in &new_routines {
        if !w.ff.arena[rid].rules.is_empty() {
            statements.push(w.routine_ast(rid)?);
        }
    }

    for (tag, refs) in &w.ff.features {
        for r in refs {
            let mut body = Vec::new();
            if let Some((script, lang)) = r.languages.first() {
                body.push(Stmt::Text(format!("script {};", script.trim())));
                body.push(Stmt::Text(format!("language {};", lang.trim())));
            }
            let name = &w.ff.arena[r.routine].name;
            if name.is_empty() {
                return Err("a feature references an unnamed routine".into());
            }
            body.push(Stmt::Block(vec![Stmt::Text(format!("lookup {name};"))]));
            statements.push(Stmt::Feature {
                name: tag.clone(),
                body,
            });
        }
    }

    let parts: Vec<String> = statements.iter().map(|s| s.fea("")).collect();
    Ok(parts.join("\n"))
}
