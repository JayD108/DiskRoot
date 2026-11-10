//! Child lists and the pruned "view" sent to the UI.
//!
//! The UI never gets the whole tree (millions of nodes). It gets a view
//! rooted at the current folder: a few levels deep, with children too small
//! to see grouped into one "N small items" node. That keeps it to a few
//! thousand shapes, so drawing stays smooth.

use serde::Serialize;

use crate::{Index, flags};

/// Children of every node, biggest (on disk) first.
pub struct Children {
    start: Vec<u32>,
    kids: Vec<u32>,
}

impl Children {
    pub fn build(idx: &Index) -> Self {
        let n = idx.len();
        let mut start = vec![0u32; n + 1];
        for i in 1..n as u32 {
            start[idx.parent(i) as usize + 1] += 1;
        }
        for i in 1..start.len() {
            start[i] += start[i - 1];
        }
        let mut fill = start.clone();
        let mut kids = vec![0u32; n.saturating_sub(1)];
        for i in 1..n as u32 {
            let at = &mut fill[idx.parent(i) as usize];
            kids[*at as usize] = i;
            *at += 1;
        }
        for i in 0..n {
            kids[start[i] as usize..start[i + 1] as usize]
                .sort_unstable_by_key(|&c| (std::cmp::Reverse(idx.alloc(c)), c));
        }
        Self { start, kids }
    }

    pub fn of(&self, id: u32) -> &[u32] {
        let i = id as usize;
        &self.kids[self.start[i] as usize..self.start[i + 1] as usize]
    }
}

/// One shape in the view. `parent` indexes into the same view list.
#[derive(Clone, Debug, Serialize)]
pub struct ViewNode {
    /// Index node id, or -1 for an "N small items" group.
    pub id: i64,
    pub parent: i32,
    pub name: String,
    /// Size on disk.
    pub size: u64,
    pub files: u32,
    pub dir: bool,
    pub flags: u8,
    pub depth: u8,
    /// For groups: how many items were merged.
    pub merged: u32,
}

pub struct ViewLimits {
    pub max_depth: u8,
    /// Children smaller than this share of the view root are grouped.
    pub min_fraction: f64,
    /// At most this many children shown per folder.
    pub max_children: usize,
}

impl Default for ViewLimits {
    fn default() -> Self {
        Self { max_depth: 6, min_fraction: 0.002, max_children: 120 }
    }
}

/// Builds the pruned view rooted at `root`, parents before children.
pub fn build_view(idx: &Index, ch: &Children, root: u32, lim: &ViewLimits) -> Vec<ViewNode> {
    let mut out = Vec::new();
    let threshold = ((idx.alloc(root) as f64 * lim.min_fraction) as u64).max(1);
    out.push(node(idx, root, -1, 0));
    // Explicit stack of (node id, its index in `out`, depth).
    let mut stack = vec![(root, 0usize, 0u8)];
    while let Some((id, at, depth)) = stack.pop() {
        if depth >= lim.max_depth || !idx.is_dir(id) {
            continue;
        }
        let kids = ch.of(id);
        let shown = kids
            .iter()
            .take(lim.max_children)
            .take_while(|&&c| idx.alloc(c) >= threshold)
            .count();
        for &c in &kids[..shown] {
            out.push(node(idx, c, at as i32, depth + 1));
            stack.push((c, out.len() - 1, depth + 1));
        }
        let rest = &kids[shown..];
        let rest_size: u64 = rest.iter().map(|&c| idx.alloc(c)).sum();
        if rest_size > 0 {
            out.push(ViewNode {
                id: -1,
                parent: at as i32,
                name: format!("{} small items", rest.len()),
                size: rest_size,
                files: rest.iter().map(|&c| idx.files(c)).sum(),
                dir: false,
                flags: 0,
                depth: depth + 1,
                merged: rest.len() as u32,
            });
        }
    }
    out
}

fn node(idx: &Index, id: u32, parent: i32, depth: u8) -> ViewNode {
    ViewNode {
        id: i64::from(id),
        parent,
        name: idx.name(id).to_owned(),
        size: idx.alloc(id),
        files: idx.files(id),
        dir: idx.flags(id) & flags::DIR != 0,
        flags: idx.flags(id),
        depth,
        merged: 0,
    }
}

/// (id, name) from the index root down to `id`, for breadcrumbs.
pub fn ancestors(idx: &Index, id: u32) -> Vec<(u32, String)> {
    let mut chain = Vec::new();
    let mut cur = id;
    while cur != crate::NO_PARENT {
        chain.push((cur, idx.name(cur).to_owned()));
        cur = idx.parent(cur);
    }
    chain.reverse();
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Index {
        let mut idx = Index::new("C:\\");
        let big = idx.push(0, "big", 0, 0, flags::DIR);
        idx.push(big, "huge.bin", 9000, 9000, 0);
        idx.push(big, "deep", 0, 0, flags::DIR);
        for i in 0..50 {
            idx.push(0, &format!("tiny{i}"), 1, 1, 0);
        }
        idx.push(0, "mid.bin", 950, 950, 0);
        idx.compute_totals();
        idx
    }

    #[test]
    fn children_are_sorted_biggest_first() {
        let idx = tree();
        let ch = Children::build(&idx);
        let names: Vec<_> = ch.of(0).iter().take(3).map(|&c| idx.name(c)).collect();
        assert_eq!(names, ["big", "mid.bin", "tiny0"]);
        assert_eq!(ch.of(0).len(), 52);
    }

    #[test]
    fn view_groups_small_items_and_respects_depth() {
        let idx = tree();
        let ch = Children::build(&idx);
        let lim = ViewLimits { max_depth: 1, min_fraction: 0.01, max_children: 10 };
        let v = build_view(&idx, &ch, 0, &lim);
        // root, big, mid.bin, group of 50 tiny files
        assert_eq!(v.len(), 4);
        assert_eq!(v[3].name, "50 small items");
        assert_eq!((v[3].size, v[3].merged, v[3].id), (50, 50, -1));
        assert!(v.iter().all(|n| n.depth <= 1), "depth limit");
        let sum: u64 = v[1..].iter().map(|n| n.size).sum();
        assert_eq!(sum, v[0].size, "children add up to the root");

        let deeper = build_view(&idx, &ch, 0, &ViewLimits { max_depth: 2, ..lim });
        assert!(deeper.iter().any(|n| n.name == "huge.bin" && n.depth == 2));
        // Parents come before children.
        assert!(deeper.iter().enumerate().skip(1).all(|(i, n)| (n.parent as usize) < i));
    }

    #[test]
    fn builds_breadcrumbs() {
        let idx = tree();
        let a = ancestors(&idx, 2);
        let names: Vec<_> = a.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(names, ["C:\\", "big", "huge.bin"]);
    }
}
