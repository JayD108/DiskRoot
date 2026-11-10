//! "What is it?": sorts every file into a category and sub-type, using the
//! rules in `rules.json` (data, not code).
//!
//! One pass in id order (parents before children):
//! - A folder or file rule matching a node decides its (category, sub-type).
//!   Everything inside inherits it, until a deeper rule matches.
//! - A file-extension rule re-classifies a file only inside "weak" places
//!   (Desktop, Documents, unclassified folders...), or anywhere if `strong`.
//! - The node where a classification was decided is the file's **anchor**.
//!   The by-category view lists anchors ("which folders make up Dev >
//!   node_modules"), and sizes add up exactly.
//!
//! Rule lookup is a hash of the node's lowercased name to the rules ending
//! in that name. Most nodes match nothing, so the pass is O(n) with a tiny
//! constant.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::view::{Children, ViewNode};
use crate::{Index, NO_PARENT, flags};

/// Category ids (index into [`CATEGORIES`]).
pub const NCAT: usize = 10;
pub const CAT_OTHER: u8 = 9;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Category {
    pub key: &'static str,
    pub name: &'static str,
}

pub const CATEGORIES: [Category; NCAT] = [
    Category { key: "apps", name: "Apps & Games" },
    Category { key: "media", name: "Media" },
    Category { key: "docs", name: "Documents" },
    Category { key: "dev", name: "Dev stuff" },
    Category { key: "ai", name: "AI & data" },
    Category { key: "system", name: "System" },
    Category { key: "temp", name: "Temp & caches" },
    Category { key: "downloads", name: "Downloads" },
    Category { key: "cloud", name: "Cloud-synced" },
    Category { key: "other", name: "Other" },
];

pub const NO_RULE: u16 = u16::MAX;
/// Sub-type 0 is always Other > Unclassified.
pub const SUB_UNCLASSIFIED: u16 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Safety {
    Green,
    Yellow,
    Red,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protect {
    /// Only the matched item itself can't be deleted.
    #[serde(rename = "self")]
    OnlySelf,
    /// The matched item and everything inside can't be deleted.
    Tree,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Dir,
    File,
    #[default]
    Any,
}

#[derive(Deserialize)]
struct RawFile {
    rules: Vec<RawRule>,
    extensions: Vec<RawExt>,
}

#[derive(Deserialize)]
struct RawRule {
    #[serde(rename = "match")]
    pattern: String,
    #[serde(default)]
    kind: Kind,
    cat: String,
    sub: String,
    safety: Option<Safety>,
    protect: Option<Protect>,
    #[serde(default)]
    weak: bool,
    note: Option<String>,
}

#[derive(Deserialize)]
struct RawExt {
    ext: Vec<String>,
    cat: String,
    sub: String,
    #[serde(default)]
    strong: bool,
}

/// Public facts about a rule, for notes, badges and delete checks.
#[derive(Clone, Debug, Serialize)]
pub struct RuleInfo {
    pub safety: Option<Safety>,
    pub note: Option<String>,
    #[serde(skip)]
    pub protect: Option<Protect>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SubType {
    pub cat: u8,
    pub name: String,
}

struct Rule {
    /// Pattern components, lowercased, in order.
    comps: Vec<String>,
    anchored: bool,
    kind: Kind,
    sub: u16,
    weak: bool,
}

#[derive(Clone, Copy)]
struct ExtRule {
    sub: u16,
    strong: bool,
}

/// The compiled rule set.
pub struct Rules {
    rules: Vec<Rule>,
    pub info: Vec<RuleInfo>,
    pub subs: Vec<SubType>,
    /// Hash of a lowercase last component -> rules ending with it.
    by_last: HashMap<u64, Vec<usize>>,
    /// Rules whose last component contains '*'.
    wild_last: Vec<usize>,
    ext: HashMap<u64, ExtRule>,
}

fn hash_ci(s: &str) -> u64 {
    // FNV-1a over ASCII-lowercased bytes; collisions are re-checked.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b.to_ascii_lowercase());
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Case-insensitive (ASCII) match with '*' wildcards.
fn glob_ci(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] != b'*' && p[pi].eq_ignore_ascii_case(&t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == b'*')
}

impl Rules {
    /// The rules shipped with Drishti.
    pub fn builtin() -> Self {
        Self::from_json(include_str!("rules.json")).expect("built-in rules.json is valid")
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let raw: RawFile = serde_json::from_str(text).map_err(|e| format!("rules: {e}"))?;
        let mut subs = vec![SubType { cat: CAT_OTHER, name: "Unclassified".into() }];
        let mut sub_id = |cat: &str, name: &str| -> Result<u16, String> {
            let cat = CATEGORIES
                .iter()
                .position(|c| c.key == cat)
                .ok_or_else(|| format!("rules: unknown category '{cat}'"))? as u8;
            if let Some(i) = subs.iter().position(|s| s.cat == cat && s.name == name) {
                return Ok(i as u16);
            }
            subs.push(SubType { cat, name: name.to_owned() });
            u16::try_from(subs.len() - 1).map_err(|_| "rules: too many sub-types".to_owned())
        };

        let mut rules = Vec::new();
        let mut info = Vec::new();
        let (mut by_last, mut wild_last) = (HashMap::<u64, Vec<usize>>::new(), Vec::new());
        for r in raw.rules {
            let anchored = r.pattern.starts_with('/');
            let comps: Vec<String> = r
                .pattern
                .split('/')
                .filter(|c| !c.is_empty())
                .map(|c| c.to_ascii_lowercase())
                .collect();
            let Some(last) = comps.last() else { return Err(format!("rules: empty pattern '{}'", r.pattern)) };
            let i = rules.len();
            if last.contains('*') {
                wild_last.push(i);
            } else {
                by_last.entry(hash_ci(last)).or_default().push(i);
            }
            rules.push(Rule { comps, anchored, kind: r.kind, sub: sub_id(&r.cat, &r.sub)?, weak: r.weak });
            info.push(RuleInfo { safety: r.safety, note: r.note, protect: r.protect });
        }
        if rules.len() >= NO_RULE as usize {
            return Err("rules: too many rules".into());
        }
        let mut ext = HashMap::new();
        for e in raw.extensions {
            let sub = sub_id(&e.cat, &e.sub)?;
            for x in e.ext {
                ext.insert(hash_ci(&x), ExtRule { sub, strong: e.strong });
            }
        }
        // Prefer the most specific rule: more components, then no wildcards.
        let specificity = |r: &Rule| (r.comps.len(), !r.comps.iter().any(|c| c.contains('*')));
        for v in by_last.values_mut() {
            v.sort_by_key(|&i| std::cmp::Reverse(specificity(&rules[i])));
        }
        wild_last.sort_by_key(|&i| std::cmp::Reverse(specificity(&rules[i])));
        Ok(Self { rules, info, subs, by_last, wild_last, ext })
    }

    pub fn sub_cat(&self, sub: u16) -> u8 {
        self.subs[sub as usize].cat
    }
}

/// Path components of a node from itself up to the drive, e.g. for
/// `C:\Users\me\x`: "x", "me", "Users", "C:". The scan root's name may be
/// a whole path, so it is split into its components.
struct Ancestry<'a> {
    idx: &'a Index,
    root_comps: &'a [&'a str],
    cur: u32,
    /// Position inside `root_comps` once we reach the root (counting down).
    root_pos: usize,
}

impl<'a> Iterator for Ancestry<'a> {
    type Item = &'a str;
    fn next(&mut self) -> Option<&'a str> {
        if self.cur != 0 && self.cur != NO_PARENT {
            let name = self.idx.name(self.cur);
            self.cur = self.idx.parent(self.cur);
            return Some(name);
        }
        self.cur = NO_PARENT;
        if self.root_pos == 0 {
            return None;
        }
        self.root_pos -= 1;
        Some(self.root_comps[self.root_pos])
    }
}

/// Result of classifying an index.
pub struct Classification {
    pub cat: Vec<u8>,
    pub sub: Vec<u16>,
    /// Node where this node's classification was decided.
    pub anchor: Vec<u32>,
    /// Rule in effect (inherited), for notes and safety. NO_RULE if none.
    pub rule: Vec<u16>,
    /// bit 0: weak (extension rules may re-classify inside);
    /// bit 1: a rule matched at this very node.
    bits: Vec<u8>,
}

const WEAK: u8 = 1;
const MATCHED_HERE: u8 = 2;

fn split_root(name: &str) -> Vec<&str> {
    name.split(['\\', '/']).filter(|c| !c.is_empty()).collect()
}

impl Classification {
    pub fn run(idx: &Index, rules: &Rules) -> Self {
        let n = idx.len();
        let root_name = idx.name(0).to_owned();
        let root_comps = split_root(&root_name);
        let mut c = Self {
            cat: vec![CAT_OTHER; n],
            sub: vec![SUB_UNCLASSIFIED; n],
            anchor: vec![0; n],
            rule: vec![NO_RULE; n],
            bits: vec![WEAK; n],
        };
        let ancestry = |id: u32| Ancestry { idx, root_comps: &root_comps, cur: id, root_pos: root_comps.len() };
        // Components below the drive (drive root = 0, "Users" = 1, ...). An
        // anchored rule with k components can only match at depth k, which
        // rejects most candidates without walking up the tree.
        let mut depth = vec![0u16; n];
        depth[0] = root_comps.len().saturating_sub(1) as u16;

        let matches = |r: &Rule, id: u32, is_dir: bool, d: u16| -> bool {
            match r.kind {
                Kind::Dir if !is_dir => return false,
                Kind::File if is_dir => return false,
                _ => {}
            }
            if r.anchored && d as usize != r.comps.len() {
                return false;
            }
            let mut up = ancestry(id);
            for comp in r.comps.iter().rev() {
                match up.next() {
                    Some(name) if glob_ci(comp, name) => {}
                    _ => return false,
                }
            }
            // Anchored: only the drive (e.g. "C:") may remain above.
            !r.anchored || (up.next().is_some_and(|d| d.ends_with(':')) && up.next().is_none())
        };

        let find = |id: u32, name: &str, is_dir: bool, d: u16| -> Option<usize> {
            let exact = rules.by_last.get(&hash_ci(name)).into_iter().flatten();
            let mut best: Option<usize> = None;
            for &i in exact.chain(rules.wild_last.iter()) {
                let better = best.is_none_or(|b| rules.rules[i].comps.len() > rules.rules[b].comps.len());
                if better && matches(&rules.rules[i], id, is_dir, d) {
                    best = Some(i);
                }
            }
            best
        };

        // The root: if the scan started inside a classified place (e.g. a
        // scan of C:\Windows), apply that rule from the start.
        let root_rule = root_comps.last().and_then(|last| find(0, last, true, depth[0]));
        if let Some(r) = root_rule {
            c.apply_rule(0, r, rules);
        }

        for id in 1..n as u32 {
            let i = id as usize;
            let p = idx.parent(id) as usize;
            let name = idx.name(id);
            let is_dir = idx.is_dir(id);
            depth[i] = depth[p].saturating_add(1);
            if let Some(r) = find(id, name, is_dir, depth[i]) {
                c.apply_rule(id, r, rules);
                continue;
            }
            // Inherit from the parent.
            (c.cat[i], c.sub[i], c.anchor[i], c.rule[i]) = (c.cat[p], c.sub[p], c.anchor[p], c.rule[p]);
            c.bits[i] = c.bits[p] & WEAK;
            if !is_dir
                && let Some(dot) = name.rfind('.')
                && let Some(e) = rules.ext.get(&hash_ci(&name[dot + 1..]))
                && (e.strong || c.bits[p] & WEAK != 0)
            {
                c.cat[i] = rules.sub_cat(e.sub);
                c.sub[i] = e.sub;
                // Group by the folder holding them ("this folder has 40 GB
                // of datasets"), not one place per file.
                c.anchor[i] = p as u32;
                c.bits[i] = 0;
            } else if p == 0 && c.sub[i] == SUB_UNCLASSIFIED {
                // Unclassified top-level items anchor themselves, so "Other"
                // breaks down by top-level folder instead of one blob.
                c.anchor[i] = id;
            }
        }
        c
    }

    fn apply_rule(&mut self, id: u32, r: usize, rules: &Rules) {
        let i = id as usize;
        let rule = &rules.rules[r];
        self.cat[i] = rules.sub_cat(rule.sub);
        self.sub[i] = rule.sub;
        self.anchor[i] = id;
        self.rule[i] = r as u16;
        self.bits[i] = MATCHED_HERE | if rule.weak { WEAK } else { 0 };
    }

    /// Why `id` must not be deleted from Drishti, if it mustn't.
    pub fn protection(&self, idx: &Index, rules: &Rules, id: u32) -> Option<String> {
        if id == 0 {
            return Some("This is the scanned folder itself.".into());
        }
        let mut cur = id;
        while cur != NO_PARENT {
            let r = self.rule[cur as usize];
            if self.bits[cur as usize] & MATCHED_HERE != 0 && r != NO_RULE {
                let protect = rules.info[r as usize].protect;
                let sub = &rules.subs[self.sub[cur as usize] as usize].name;
                if protect == Some(Protect::Tree) || (protect == Some(Protect::OnlySelf) && cur == id) {
                    let what = if cur == id { "This item" } else { "Everything inside it" };
                    return Some(format!("{what} is protected ({sub}): {}", idx.name(cur)));
                }
            }
            cur = idx.parent(cur);
        }
        None
    }
}

/// A place holding part of a sub-type: the bytes and files of that
/// sub-type anchored at one node.
#[derive(Clone, Copy, Debug)]
pub struct Place {
    pub node: u32,
    pub bytes: u64,
    pub files: u32,
}

/// Sizes per category, per sub-type and per place, plus each folder's
/// breakdown by category. Recomputed after deletions (fast, one pass).
pub struct CatStats {
    pub cat_total: [u64; NCAT],
    pub cat_files: [u64; NCAT],
    pub sub_total: Vec<u64>,
    pub sub_files: Vec<u64>,
    /// Places of each sub-type, biggest first.
    pub places_by_sub: Vec<Vec<Place>>,
    /// The category with the most bytes inside each node (its colour).
    pub dominant: Vec<u8>,
    dir_slot: Vec<u32>,
    dir_cats: Vec<[u64; NCAT]>,
}

impl CatStats {
    pub fn compute(idx: &Index, cls: &Classification, nsubs: usize) -> Self {
        let n = idx.len();
        let mut s = Self {
            cat_total: [0; NCAT],
            cat_files: [0; NCAT],
            sub_total: vec![0; nsubs],
            sub_files: vec![0; nsubs],
            places_by_sub: vec![Vec::new(); nsubs],
            dominant: cls.cat.clone(),
            dir_slot: vec![u32::MAX; n],
            dir_cats: Vec::new(),
        };
        // Bytes/files per (sub-type, anchor). Almost always the file's
        // sub-type equals its anchor's own sub-type, so that case uses plain
        // arrays; the rare mixed case (videos found by extension inside a
        // Documents folder) goes to a small map.
        let mut own_bytes = vec![0u64; n];
        let mut own_files = vec![0u32; n];
        let mut mixed: HashMap<(u16, u32), (u64, u32)> = HashMap::new();
        for id in 0..n as u32 {
            if idx.is_dir(id) && idx.flags(id) & flags::DELETED == 0 {
                s.dir_slot[id as usize] = s.dir_cats.len() as u32;
                s.dir_cats.push([0; NCAT]);
            }
        }
        // Files first, then fold folders into parents (children have higher
        // ids, so a reverse pass sees every child before its parent).
        for id in (0..n as u32).rev() {
            let i = id as usize;
            if idx.flags(id) & flags::DELETED != 0 {
                continue;
            }
            let p = idx.parent(id);
            if !idx.is_dir(id) {
                let a = idx.alloc(id);
                let cat = cls.cat[i] as usize;
                let sub = cls.sub[i];
                s.cat_total[cat] += a;
                s.cat_files[cat] += 1;
                s.sub_total[sub as usize] += a;
                s.sub_files[sub as usize] += 1;
                let anchor = cls.anchor[i];
                if cls.sub[anchor as usize] == sub {
                    own_bytes[anchor as usize] += a;
                    own_files[anchor as usize] += 1;
                } else {
                    let e = mixed.entry((sub, anchor)).or_insert((0, 0));
                    e.0 += a;
                    e.1 += 1;
                }
                if p != NO_PARENT {
                    s.dir_cats[s.dir_slot[p as usize] as usize][cat] += a;
                }
            } else {
                let mine = s.dir_cats[s.dir_slot[i] as usize];
                s.dominant[i] = mine
                    .iter()
                    .enumerate()
                    .max_by_key(|&(_, v)| *v)
                    .filter(|&(_, v)| *v > 0)
                    .map_or(cls.cat[i], |(c, _)| c as u8);
                if p != NO_PARENT {
                    let ps = s.dir_slot[p as usize] as usize;
                    for (t, v) in s.dir_cats[ps].iter_mut().zip(mine) {
                        *t += v;
                    }
                }
            }
        }
        for (node, &bytes) in own_bytes.iter().enumerate() {
            if bytes > 0 {
                let p = Place { node: node as u32, bytes, files: own_files[node] };
                s.places_by_sub[cls.sub[node] as usize].push(p);
            }
        }
        for ((sub, node), (bytes, files)) in mixed {
            if bytes > 0 {
                s.places_by_sub[sub as usize].push(Place { node, bytes, files });
            }
        }
        for v in &mut s.places_by_sub {
            v.sort_unstable_by_key(|p| (std::cmp::Reverse(p.bytes), p.node));
        }
        s
    }

    /// Bytes per category inside folder `id` (all zeros for files).
    pub fn breakdown(&self, id: u32) -> [u64; NCAT] {
        match self.dir_slot.get(id as usize) {
            Some(&slot) if slot != u32::MAX => self.dir_cats[slot as usize],
            _ => [0; NCAT],
        }
    }
}

/// Where the by-category view is rooted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatKey {
    All,
    Cat(u8),
    Sub(u16),
}

impl CatKey {
    /// Parses "", "cat:3" or "sub:12".
    pub fn parse(s: &str, rules: &Rules) -> Option<Self> {
        if s.is_empty() {
            return Some(Self::All);
        }
        let (kind, num) = s.split_once(':')?;
        match kind {
            "cat" => num.parse::<u8>().ok().filter(|&c| (c as usize) < NCAT).map(Self::Cat),
            "sub" => num.parse::<u16>().ok().filter(|&x| (x as usize) < rules.subs.len()).map(Self::Sub),
            _ => None,
        }
    }

    pub fn key(self) -> String {
        match self {
            Self::All => String::new(),
            Self::Cat(c) => format!("cat:{c}"),
            Self::Sub(s) => format!("sub:{s}"),
        }
    }
}

/// Breadcrumb for the by-category view.
#[derive(Clone, Debug, Serialize)]
pub struct CatCrumb {
    pub key: String,
    pub name: String,
}

/// Inputs for [`category_view`].
pub struct CatModel<'a> {
    pub idx: &'a Index,
    pub cls: &'a Classification,
    pub stats: &'a CatStats,
    pub rules: &'a Rules,
}

impl CatModel<'_> {
    fn label(&self, key: CatKey) -> String {
        match key {
            CatKey::All => format!("{} by category", self.idx.name(0)),
            CatKey::Cat(c) => CATEGORIES[c as usize].name.to_owned(),
            CatKey::Sub(s) => self.rules.subs[s as usize].name.clone(),
        }
    }

    fn size(&self, key: CatKey) -> u64 {
        match key {
            CatKey::All => self.stats.cat_total.iter().sum(),
            CatKey::Cat(c) => self.stats.cat_total[c as usize],
            CatKey::Sub(s) => self.stats.sub_total[s as usize],
        }
    }

    fn cat_of(&self, key: CatKey) -> u8 {
        match key {
            CatKey::All => u8::MAX,
            CatKey::Cat(c) => c,
            CatKey::Sub(s) => self.rules.sub_cat(s),
        }
    }

    pub fn crumbs(&self, key: CatKey) -> Vec<CatCrumb> {
        let mut keys = vec![CatKey::All];
        match key {
            CatKey::All => {}
            CatKey::Cat(_) => keys.push(key),
            CatKey::Sub(s) => keys.extend([CatKey::Cat(self.rules.sub_cat(s)), key]),
        }
        keys.into_iter().map(|k| CatCrumb { key: k.key(), name: self.label(k) }).collect()
    }

    fn group_node(&self, at: usize, key: CatKey, depth: u8) -> ViewNode {
        let files = match key {
            CatKey::All => self.stats.cat_files.iter().sum(),
            CatKey::Cat(c) => self.stats.cat_files[c as usize],
            CatKey::Sub(s) => self.stats.sub_files[s as usize],
        };
        let files = u32::try_from(files).unwrap_or(u32::MAX);
        let mut n = ViewNode::plain(-2, at as i32, self.label(key), self.size(key), files, true, depth);
        n.key = key.key();
        n.cat = self.cat_of(key);
        n.own_cat = n.cat;
        if let CatKey::Sub(s) = key {
            n.sub = s;
        }
        n
    }

    /// Adds the places of sub-type `s` under `at`.
    fn push_anchors(&self, out: &mut Vec<ViewNode>, s: u16, at: usize, depth: u8, max: usize, threshold: u64) {
        let places = &self.stats.places_by_sub[s as usize];
        let shown = places.iter().take(max).take_while(|p| p.bytes >= threshold).count();
        let cat = self.rules.sub_cat(s);
        for p in &places[..shown] {
            let a = p.node;
            let mut n = ViewNode::plain(
                i64::from(a),
                at as i32,
                self.idx.name(a).to_owned(),
                p.bytes,
                p.files,
                self.idx.is_dir(a),
                depth,
            );
            n.flags = self.idx.flags(a);
            (n.cat, n.own_cat, n.sub, n.rule) = (cat, cat, s, self.cls.rule[a as usize]);
            n.hint = self.idx.path(a);
            out.push(n);
        }
        let rest = &places[shown..];
        let rest_size: u64 = rest.iter().map(|p| p.bytes).sum();
        if rest_size > 0 {
            let files = rest.iter().map(|p| p.files).sum();
            let mut g = ViewNode::plain(-1, at as i32, format!("{} more places", rest.len()), rest_size, files, false, depth);
            (g.merged, g.cat, g.own_cat, g.sub) = (rest.len() as u32, cat, cat, s);
            out.push(g);
        }
    }

    fn push_subs(&self, out: &mut Vec<ViewNode>, c: u8, at: usize, depth: u8, per_sub: usize, threshold: u64) {
        let mut subs: Vec<u16> = (0..self.rules.subs.len() as u16)
            .filter(|&s| self.rules.sub_cat(s) == c && self.stats.sub_total[s as usize] > 0)
            .collect();
        subs.sort_unstable_by_key(|&s| std::cmp::Reverse(self.stats.sub_total[s as usize]));
        for s in subs {
            let n = self.group_node(at, CatKey::Sub(s), depth);
            out.push(n);
            let me = out.len() - 1;
            self.push_anchors(out, s, me, depth + 1, per_sub, threshold);
        }
    }

    /// The by-category tree rooted at `key`: categories > sub-types > the
    /// places (anchors) holding the bytes. Sizes add up exactly.
    pub fn view(&self, key: CatKey, min_fraction: f64) -> Vec<ViewNode> {
        let threshold = ((self.size(key) as f64 * min_fraction) as u64).max(1);
        let mut out = Vec::new();
        let mut root = self.group_node(0, key, 0);
        root.parent = -1;
        out.push(root);
        match key {
            CatKey::All => {
                let mut cats: Vec<u8> = (0..NCAT as u8).filter(|&c| self.stats.cat_total[c as usize] > 0).collect();
                cats.sort_unstable_by_key(|&c| std::cmp::Reverse(self.stats.cat_total[c as usize]));
                for c in cats {
                    let n = self.group_node(0, CatKey::Cat(c), 1);
                    out.push(n);
                    let me = out.len() - 1;
                    self.push_subs(&mut out, c, me, 2, 12, threshold);
                }
            }
            CatKey::Cat(c) => self.push_subs(&mut out, c, 0, 1, 40, threshold),
            CatKey::Sub(s) => self.push_anchors(&mut out, s, 0, 1, 300, threshold),
        }
        out
    }
}


impl Index {
    /// Removes `id` and everything inside it from the totals (after it was
    /// deleted on disk). The nodes stay in the arrays, flagged DELETED.
    pub fn remove_subtree(&mut self, id: u32, ch: &Children) {
        assert!(self.totals_ready(), "compute_totals first");
        if id == 0 || id as usize >= self.len() || self.flags[id as usize] & flags::DELETED != 0 {
            return;
        }
        let (s, a, f) = (self.size(id), self.alloc(id), self.files(id));
        let mut cur = self.parent(id);
        while cur != NO_PARENT {
            let c = cur as usize;
            self.size[c] -= s;
            self.alloc[c] -= a;
            self.files[c] -= f;
            cur = self.parent(cur);
        }
        let mut stack = vec![id];
        while let Some(x) = stack.pop() {
            self.flags[x as usize] |= flags::DELETED;
            stack.extend_from_slice(ch.of(x));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct T {
        idx: Index,
    }

    impl T {
        fn new(root: &str) -> Self {
            Self { idx: Index::new(root) }
        }
        fn dir(&mut self, parent: u32, name: &str) -> u32 {
            self.idx.push(parent, name, 0, 0, flags::DIR)
        }
        fn file(&mut self, parent: u32, name: &str, alloc: u64) -> u32 {
            self.idx.push(parent, name, alloc, alloc, 0)
        }
    }

    fn sub_name(rules: &Rules, c: &Classification, id: u32) -> String {
        let s = &rules.subs[c.sub[id as usize] as usize];
        format!("{} > {}", CATEGORIES[s.cat as usize].name, s.name)
    }

    #[test]
    fn glob_matching() {
        assert!(glob_ci("$*", "$MFT"));
        assert!(glob_ci("onedrive*", "OneDrive - Personal"));
        assert!(glob_ci("*", "anything"));
        assert!(glob_ci("node_modules", "NODE_MODULES"));
        assert!(!glob_ci("$*", "Users"));
        assert!(!glob_ci("cache", "caches"));
    }

    #[test]
    fn classifies_a_drive() {
        let rules = Rules::builtin();
        let mut t = T::new("C:\\");
        let win = t.dir(0, "Windows");
        let sxs = t.dir(win, "WinSxS");
        let sxs_file = t.file(sxs, "x.dll", 100);
        let users = t.dir(0, "Users");
        let me = t.dir(users, "me");
        let appdata = t.dir(me, "AppData");
        let local = t.dir(appdata, "Local");
        let temp = t.dir(local, "Temp");
        let tmpf = t.file(temp, "junk.tmp", 50);
        let desktop = t.dir(me, "Desktop");
        let proj = t.dir(desktop, "proj");
        let nm = t.dir(proj, "node_modules");
        let nm_file = t.file(nm, "big.js", 10);
        let nm_video = t.file(nm, "demo.mp4", 10); // strong folder: stays Dev
        let video = t.file(desktop, "trip.mp4", 700); // weak folder: Media
        let dl = t.dir(me, "Downloads");
        let model = t.file(dl, "llama.gguf", 4000); // strong extension wins
        let setup = t.file(dl, "setup.exe", 30); // Downloads isn't weak
        let data = t.dir(0, "V2_V3_DeepLearning");
        let npy = t.file(data, "x.npy", 900);
        let other = t.file(data, "notes.xyz", 5);
        let page = t.file(0, "pagefile.sys", 2000);
        let mft = t.file(0, "$MFT", 300);
        let bin = t.dir(0, "$Recycle.Bin");
        t.idx.compute_totals();
        let c = Classification::run(&t.idx, &rules);

        assert_eq!(sub_name(&rules, &c, sxs_file), "System > Component store (WinSxS)");
        assert_eq!(sub_name(&rules, &c, tmpf), "Temp & caches > User temp");
        assert_eq!(sub_name(&rules, &c, nm_file), "Dev stuff > node_modules");
        assert_eq!(sub_name(&rules, &c, nm_video), "Dev stuff > node_modules");
        assert_eq!(sub_name(&rules, &c, video), "Media > Video");
        assert_eq!(sub_name(&rules, &c, model), "AI & data > Model files");
        assert_eq!(sub_name(&rules, &c, setup), "Downloads > Downloads");
        assert_eq!(sub_name(&rules, &c, npy), "AI & data > Datasets");
        assert_eq!(sub_name(&rules, &c, other), "Other > Unclassified");
        assert_eq!(sub_name(&rules, &c, page), "System > Page file");
        assert_eq!(sub_name(&rules, &c, mft), "System > NTFS metadata");
        assert_eq!(sub_name(&rules, &c, bin), "Temp & caches > Recycle Bin", "exact rule beats /$*");
        assert_eq!(c.anchor[nm_file as usize], nm);
        assert_eq!(c.anchor[other as usize], data, "unclassified anchors at top-level folder");

        // Protection
        let p = |id| c.protection(&t.idx, &rules, id);
        assert!(p(sxs_file).is_some(), "inside /Windows tree");
        assert!(p(desktop).is_some(), "Desktop folder itself");
        assert!(p(proj).is_none(), "a project on the Desktop is deletable");
        assert!(p(nm).is_none());
        assert!(p(page).is_some());
        assert!(p(me).is_some(), "profile folder itself");
        assert!(p(tmpf).is_none(), "temp files are deletable");
        assert!(p(temp).is_some(), "the Temp folder itself is not");
        assert!(p(0).is_some());

        // Stats add up and folders get the dominant colour.
        let s = CatStats::compute(&t.idx, &c, rules.subs.len());
        assert_eq!(s.cat_total.iter().sum::<u64>(), t.idx.alloc(0));
        let sub_sum: u64 = s.sub_total.iter().sum();
        assert_eq!(sub_sum, t.idx.alloc(0));
        let anchored: u64 = s.places_by_sub.iter().flatten().map(|p| p.bytes).sum();
        assert_eq!(anchored, t.idx.alloc(0));
        let ai = CATEGORIES.iter().position(|k| k.key == "ai").unwrap() as u8;
        assert_eq!(s.dominant[dl as usize], ai, "Downloads is mostly a model file");
        assert_eq!(s.breakdown(0).iter().sum::<u64>(), t.idx.alloc(0));

        // By-category view: categories, sub-types and places add up.
        let m = CatModel { idx: &t.idx, cls: &c, stats: &s, rules: &rules };
        let v = m.view(CatKey::All, 0.0);
        assert_eq!(v[0].size, t.idx.alloc(0));
        let cats: u64 = v.iter().filter(|n| n.depth == 1).map(|n| n.size).sum();
        let places: u64 = v.iter().filter(|n| n.depth == 3).map(|n| n.size).sum();
        assert_eq!((cats, places), (t.idx.alloc(0), t.idx.alloc(0)));
        assert!(v.iter().enumerate().skip(1).all(|(i, n)| (n.parent as usize) < i));
        let ai_view = m.view(CatKey::Cat(ai), 0.0);
        // Extension-classified files are grouped by the folder holding them.
        assert!(ai_view.iter().any(|n| n.name == "Downloads" && n.hint == "C:\\Users\\me\\Downloads" && n.size == 4000));
        assert_eq!(c.anchor[video as usize], desktop);
        assert_eq!(v[0].files, t.idx.files(0));
        let key = CatKey::parse(&ai_view[1].key, &rules).unwrap();
        assert!(matches!(key, CatKey::Sub(_)));
        assert_eq!(m.crumbs(key).len(), 3);
        assert_eq!(CatKey::parse("cat:99", &rules), None);
        assert_eq!(CatKey::parse("bogus", &rules), None);
    }

    #[test]
    fn folder_scan_root_uses_its_full_path() {
        let rules = Rules::builtin();
        let mut t = T::new("C:\\Users\\me");
        let dl = t.dir(0, "Downloads");
        let f = t.file(dl, "a.zip", 1);
        t.idx.compute_totals();
        let c = Classification::run(&t.idx, &rules);
        assert_eq!(sub_name(&rules, &c, f), "Downloads > Downloads");
        // Scanning inside Windows: everything is Windows.
        let mut t = T::new("C:\\Windows");
        let x = t.file(0, "notepad.exe", 1);
        t.idx.compute_totals();
        let c = Classification::run(&t.idx, &rules);
        assert_eq!(sub_name(&rules, &c, x), "System > Windows");
        assert!(c.protection(&t.idx, &rules, x).is_some());
    }

    #[test]
    fn removing_a_subtree_updates_totals() {
        let mut t = T::new("C:\\");
        let a = t.dir(0, "a");
        let b = t.dir(a, "b");
        t.file(b, "x", 100);
        t.file(a, "y", 5);
        t.idx.compute_totals();
        let ch = Children::build(&t.idx);
        t.idx.remove_subtree(b, &ch);
        assert_eq!((t.idx.alloc(0), t.idx.alloc(a), t.idx.files(0)), (5, 5, 1));
        assert!(t.idx.flags(3) & flags::DELETED != 0, "file inside is flagged too");
        t.idx.remove_subtree(b, &ch); // idempotent
        assert_eq!(t.idx.alloc(0), 5);
    }

    #[test]
    fn rejects_bad_rule_files() {
        assert!(Rules::from_json("{").is_err());
        let bad_cat = r#"{"rules":[{"match":"x","cat":"nope","sub":"y"}],"extensions":[]}"#;
        assert!(Rules::from_json(bad_cat).is_err());
    }

    #[test]
    #[ignore = "speed benchmark: cargo test --release -p drishti-core -- --ignored --nocapture"]
    fn classify_speed() {
        let rules = Rules::builtin();
        let mut idx = Index::new("C:\\");
        let mut dirs = vec![0u32];
        let mut x = 12345u64;
        for i in 0..1_770_000u32 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let parent = dirs[(x % dirs.len() as u64) as usize];
            if i % 6 == 0 {
                let names = ["node_modules", "src", "Temp", "cache", "data", "x64"];
                dirs.push(idx.push(parent, names[(x % 6) as usize], 0, 0, flags::DIR));
            } else {
                let names = ["a.js", "b.mp4", "c.npy", "d.txt", "e.dll", "f"];
                idx.push(parent, names[(x % 6) as usize], 4096, 4096, 0);
            }
        }
        idx.compute_totals();
        let t = std::time::Instant::now();
        let c = Classification::run(&idx, &rules);
        let t1 = t.elapsed();
        let s = CatStats::compute(&idx, &c, rules.subs.len());
        println!("classify {:.3} s, stats {:.3} s", t1.as_secs_f64(), (t.elapsed() - t1).as_secs_f64());
        assert_eq!(s.cat_total.iter().sum::<u64>(), idx.alloc(0));
    }
}
