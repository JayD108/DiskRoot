//! The map pane: a sunburst (a stem cross-section: pith in the middle,
//! growth rings outward) or a squarified treemap, drawn from the pruned
//! view the session returns. Layout is cached per (data, size), so an idle
//! window costs nothing.

use std::f32::consts::{FRAC_PI_2, TAU};

use eframe::egui::{
    Align2, Color32, CornerRadius, Mesh, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, emath::Rot2, epaint::TextShape,
    pos2, vec2,
};
use diskroot_session::Shape as Node;

use crate::theme::{self, Tokens};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Sunburst,
    Treemap,
}

pub struct Arc_ {
    pub i: usize,
    pub a0: f32,
    pub a1: f32,
    pub ri: f32,
    pub ro: f32,
    pub depth: u8,
}

pub struct Tile {
    pub i: usize,
    pub rect: Rect,
    pub header: bool,
    pub depth: u8,
}

pub enum Layout {
    Sun { center: Pos2, r0: f32, arcs: Vec<Arc_> },
    Tree { tiles: Vec<Tile> },
}

pub enum Hit {
    Node(usize),
    Pith,
}

/// Children of each view node (indices into the node list).
pub fn children(nodes: &[Node]) -> Vec<Vec<usize>> {
    let mut kids = vec![Vec::new(); nodes.len()];
    for (i, n) in nodes.iter().enumerate().skip(1) {
        if n.parent >= 0 {
            kids[n.parent as usize].push(i);
        }
    }
    kids
}

pub fn layout(kind: Kind, nodes: &[Node], kids: &[Vec<usize>], rect: Rect) -> Layout {
    match kind {
        Kind::Sunburst => layout_sun(nodes, kids, rect),
        Kind::Treemap => layout_tree(nodes, kids, rect),
    }
}

fn layout_sun(nodes: &[Node], kids: &[Vec<usize>], rect: Rect) -> Layout {
    let center = rect.center();
    let max_depth = nodes.iter().map(|n| n.depth).max().unwrap_or(1).clamp(1, 3);
    let radius = (rect.width().min(rect.height()) / 2.0 - 10.0).max(60.0);
    let r0 = (radius * 0.27).max(44.0);
    let ring = (radius - r0) / f32::from(max_depth);
    let mut arcs = Vec::new();
    // Iterative walk: (node, a0, a1).
    let mut stack = vec![(0usize, 0.0f32, TAU)];
    while let Some((i, a0, a1)) = stack.pop() {
        let n = &nodes[i];
        if n.depth > 0 {
            let d = f32::from(n.depth);
            arcs.push(Arc_ { i, a0, a1, ri: r0 + (d - 1.0) * ring, ro: r0 + d * ring, depth: n.depth });
        }
        if n.depth >= max_depth || n.size == 0 {
            continue;
        }
        let mut a = a0;
        for &k in &kids[i] {
            let span = (a1 - a0) * (nodes[k].size as f64 / n.size as f64) as f32;
            stack.push((k, a, a + span));
            a += span;
        }
    }
    Layout::Sun { center, r0, arcs }
}

fn squarify(items: &mut [(usize, f64)], mut r: Rect, out: &mut Vec<(usize, Rect)>) {
    items.sort_by(|a, b| b.1.total_cmp(&a.1));
    let total: f64 = items.iter().map(|x| x.1).sum();
    if total <= 0.0 || r.width() <= 0.0 || r.height() <= 0.0 {
        return;
    }
    let scale = f64::from(r.width() * r.height()) / total;
    let mut i = 0;
    while i < items.len() {
        // No room left (the rest are too small to see): stop. Without this a
        // zero-wide strip gives infinite tiles, which break the 2.5D view.
        if r.width() < 0.5 || r.height() < 0.5 {
            break;
        }
        let short = f64::from(r.width().min(r.height()));
        let max_a = items[i].1 * scale;
        let (mut end, mut sum, mut best) = (i, 0.0, f64::INFINITY);
        while end < items.len() {
            let a = items[end].1 * scale;
            let s = sum + a;
            let worst = (short * short * max_a / (s * s)).max(s * s / (short * short * a));
            if worst > best {
                break;
            }
            (best, sum) = (worst, s);
            end += 1;
        }
        if r.width() >= r.height() {
            let w = (sum / f64::from(r.height())) as f32;
            let mut y = r.min.y;
            for it in &items[i..end] {
                let h = (it.1 * scale) as f32 / w;
                out.push((it.0, Rect::from_min_size(pos2(r.min.x, y), vec2(w, h))));
                y += h;
            }
            r.min.x += w;
        } else {
            let h = (sum / f64::from(r.width())) as f32;
            let mut x = r.min.x;
            for it in &items[i..end] {
                let w = (it.1 * scale) as f32 / h;
                out.push((it.0, Rect::from_min_size(pos2(x, r.min.y), vec2(w, h))));
                x += w;
            }
            r.min.y += h;
        }
        i = end;
    }
}

fn layout_tree(nodes: &[Node], kids: &[Vec<usize>], rect: Rect) -> Layout {
    let mut tiles = Vec::new();
    let mut top = Vec::new();
    let mut items: Vec<(usize, f64)> =
        kids[0].iter().filter(|&&k| nodes[k].size > 0).map(|&k| (k, nodes[k].size as f64)).collect();
    squarify(&mut items, rect.shrink(6.0), &mut top);
    top.retain(|(_, r)| r.is_finite() && r.is_positive());
    for (i, r) in top {
        let nest = !kids[i].is_empty() && r.width() > 50.0 && r.height() > 44.0;
        tiles.push(Tile { i, rect: r, header: nest, depth: 1 });
        if nest {
            let inner = Rect::from_min_max(pos2(r.min.x + 2.0, r.min.y + 20.0), pos2(r.max.x - 2.0, r.max.y - 2.0));
            let mut sub: Vec<(usize, f64)> =
                kids[i].iter().filter(|&&k| nodes[k].size > 0).map(|&k| (k, nodes[k].size as f64)).collect();
            let mut placed = Vec::new();
            squarify(&mut sub, inner, &mut placed);
            placed.retain(|(_, r)| r.is_finite() && r.is_positive());
            tiles.extend(placed.into_iter().map(|(k, rr)| Tile { i: k, rect: rr, header: false, depth: 2 }));
        }
    }
    Layout::Tree { tiles }
}

pub fn hit(layout: &Layout, p: Pos2) -> Option<Hit> {
    match layout {
        Layout::Sun { center, r0, arcs } => {
            let d = p - *center;
            let r = d.length();
            if r < r0 - 3.0 {
                return Some(Hit::Pith);
            }
            let mut a = d.y.atan2(d.x) + FRAC_PI_2;
            if a < 0.0 {
                a += TAU;
            }
            arcs.iter().find(|s| r >= s.ri && r < s.ro && a >= s.a0 && a < s.a1).map(|s| Hit::Node(s.i))
        }
        Layout::Tree { tiles } => tiles.iter().rev().find(|t| t.rect.contains(p)).map(|t| Hit::Node(t.i)),
    }
}

fn color_of(n: &Node, depth: u8, t: &Tokens, today: Option<u16>, over: Option<Color32>) -> Color32 {
    let base = match today {
        _ if over.is_some() => over.unwrap_or_default(),
        Some(d) => theme::age_color(diskroot_session::age_bucket(d, n.mtime)),
        None if n.cat == u8::MAX => theme::cat_color(9),
        None => theme::cat_color(n.cat),
    };
    let c = theme::tint(base, depth, t);
    if n.id == -1 && n.key.is_empty() {
        // "N small items": muted
        let g = (u16::from(c.r()) + u16::from(c.g()) + u16::from(c.b())) / 3;
        let m = |x: u8| ((u16::from(x) + g * 2) / 3) as u8;
        return Color32::from_rgb(m(c.r()), m(c.g()), m(c.b()));
    }
    c
}

/// Ring segment as a triangle strip; `gap` px of negative space each side.
pub fn ring_mesh(c: Pos2, ri: f32, ro: f32, a0: f32, a1: f32, color: Color32) -> Option<Mesh> {
    // Negative space between neighbours; a full ring has no neighbours.
    let gap_o = if a1 - a0 > TAU - 1e-3 { 0.0 } else { 0.75 / ro };
    let (a0, a1) = (a0 + gap_o, a1 - gap_o);
    if a1 <= a0 {
        return None;
    }
    let (ri, ro) = (ri + 0.75, ro - 0.75);
    let steps = (((a1 - a0) / TAU) * 220.0).ceil().max(1.0) as u32;
    let mut mesh = Mesh::default();
    for s in 0..=steps {
        let a = a0 + (a1 - a0) * s as f32 / steps as f32 - FRAC_PI_2;
        let (sn, cs) = a.sin_cos();
        mesh.colored_vertex(pos2(c.x + cs * ri, c.y + sn * ri), color);
        mesh.colored_vertex(pos2(c.x + cs * ro, c.y + sn * ro), color);
        if s > 0 {
            let b = s * 2;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b + 1, b);
        }
    }
    Some(mesh)
}

fn ring_outline(c: Pos2, ri: f32, ro: f32, a0: f32, a1: f32) -> Vec<Pos2> {
    let steps = (((a1 - a0) / TAU) * 220.0).ceil().max(1.0) as u32;
    let at = |r: f32, a: f32| {
        let (sn, cs) = (a - FRAC_PI_2).sin_cos();
        pos2(c.x + cs * r, c.y + sn * r)
    };
    let mut pts: Vec<Pos2> = (0..=steps).map(|s| at(ro, a0 + (a1 - a0) * s as f32 / steps as f32)).collect();
    pts.extend((0..=steps).rev().map(|s| at(ri, a0 + (a1 - a0) * s as f32 / steps as f32)));
    pts
}

/// Shortens `text` with "…" to fit `max_w` points.
pub fn fit(painter: &Painter, text: &str, size: f32, max_w: f32) -> Option<String> {
    if max_w < 18.0 {
        return None;
    }
    let w = |s: &str| painter.layout_no_wrap(s.to_owned(), theme::regular(size), Color32::WHITE).size().x;
    if w(text) <= max_w {
        return Some(text.to_owned());
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let cand: String = chars[..mid].iter().collect::<String>() + "…";
        if w(&cand) <= max_w { lo = mid } else { hi = mid - 1 }
    }
    (lo > 1).then(|| chars[..lo].iter().collect::<String>() + "…")
}

/// Draws `text` centred on screen angle `mid` (radians, 0 = 3 o'clock) along
/// a circle of radius `r`. Upper half: reads clockwise; lower half: reads
/// counter-clockwise, so the text is never upside down.
fn curved_text(p: &Painter, center: Pos2, r: f32, mid: f32, text: &str, color: Color32) {
    let font = theme::regular(11.5);
    let chars: Vec<(char, f32)> =
        text.chars().map(|c| (c, p.layout_no_wrap(c.to_string(), font.clone(), color).size().x)).collect();
    let total: f32 = chars.iter().map(|c| c.1).sum();
    let lower = mid.sin() > 0.15; // bottom of the circle (screen y grows down)
    let dir = if lower { -1.0 } else { 1.0 };
    let mut theta = mid - dir * total / r / 2.0;
    for (c, w) in chars {
        let a = theta + dir * w / r / 2.0; // angle at this letter's centre
        theta += dir * w / r;
        if c == ' ' {
            continue;
        }
        let rot = if lower { a - FRAC_PI_2 } else { a + FRAC_PI_2 };
        let galley = p.layout_no_wrap(c.to_string(), font.clone(), color);
        let at = center + vec2(a.cos(), a.sin()) * r;
        let pos = at - Rot2::from_angle(rot) * (galley.size() / 2.0);
        let mut ts = TextShape::new(pos, galley, color);
        ts.angle = rot;
        p.add(ts);
    }
}

pub struct PaintCtx<'a> {
    pub nodes: &'a [Node],
    pub hover: Option<usize>,
    pub selected: Option<usize>,
    pub t: &'a Tokens,
    pub center_title: &'a str,
    pub center_value: &'a str,
    pub can_go_up: bool,
    /// Some(today): colour by last-modified age instead of category.
    pub today: Option<u16>,
    /// Map modes (growth, reclaimable): a colour per node; empty = by category.
    pub over: &'a [Color32],
    /// Transition progress, 0..1 (1 = done): the sunburst sweeps round,
    /// treemap tiles grow from the middle. Labels appear at the end.
    pub grow: f32,
}

pub fn paint(p: &Painter, layout: &Layout, cx: &PaintCtx) {
    let t = cx.t;
    match layout {
        Layout::Sun { center, r0, arcs } => {
            let g = cx.grow.clamp(0.0, 1.0);
            for s in arcs {
                let (a0, a1) = (s.a0 * g, s.a1 * g);
                if a1 - a0 < 0.002 {
                    continue;
                }
                if let Some(m) = ring_mesh(*center, s.ri, s.ro, a0, a1, color_of(&cx.nodes[s.i], s.depth, t, cx.today, cx.over.get(s.i).copied())) {
                    p.add(Shape::mesh(m));
                }
            }
            for s in arcs.iter().filter(|_| g >= 1.0) {
                if Some(s.i) == cx.hover || Some(s.i) == cx.selected {
                    let pts = ring_outline(*center, s.ri + 0.75, s.ro - 0.75, s.a0, s.a1);
                    p.add(Shape::closed_line(pts, Stroke::new(2.0, t.text)));
                }
            }
            // Labels follow the ring: each letter is placed on the arc and
            // turned along it (upright on both halves of the circle), so
            // long names bend with the segment instead of sticking out.
            for s in arcs.iter().filter(|_| g >= 1.0) {
                let rm = (s.ri + s.ro) / 2.0;
                let len = (s.a1 - s.a0) * rm;
                if len < 60.0 || s.ro - s.ri < 15.0 {
                    continue;
                }
                let ink = theme::ink(s.depth, t);
                let Some(label) = fit(p, &cx.nodes[s.i].name, 11.5, (len - 14.0).min(320.0)) else { continue };
                curved_text(p, *center, rm, (s.a0 + s.a1) / 2.0 - FRAC_PI_2, &label, ink);
            }
            // Pith with faint growth rings.
            p.circle_filled(*center, r0 - 3.0, t.panel);
            for k in 1..=3 {
                p.circle_stroke(*center, (r0 - 3.0) * (0.35 + k as f32 * 0.2), Stroke::new(1.0, t.line));
            }
            p.text(*center - vec2(0.0, 6.0), Align2::CENTER_CENTER, cx.center_value, theme::semibold(16.0), t.text);
            if let Some(title) = fit(p, cx.center_title, 11.0, (r0 - 12.0) * 1.6) {
                p.text(*center + vec2(0.0, 13.0), Align2::CENTER_CENTER, title, theme::regular(11.0), t.muted);
            }
            if cx.can_go_up {
                p.text(*center + vec2(0.0, 28.0), Align2::CENTER_CENTER, "↑ up", theme::regular(10.5), t.faint);
            }
        }
        Layout::Tree { tiles } => {
            let g = cx.grow.clamp(0.0, 1.0);
            if g < 1.0 {
                let all = tiles.iter().fold(Rect::NOTHING, |r, x| r.union(x.rect));
                let c = all.center();
                for tile in tiles {
                    let r = Rect::from_center_size(c + (tile.rect.center() - c) * g, tile.rect.size() * g);
                    p.rect_filled(r, CornerRadius::ZERO, color_of(&cx.nodes[tile.i], tile.depth, t, cx.today, cx.over.get(tile.i).copied()));
                }
                return;
            }
            for tile in tiles {
                let n = &cx.nodes[tile.i];
                p.rect_filled(tile.rect, CornerRadius::ZERO, color_of(n, tile.depth, t, cx.today, cx.over.get(tile.i).copied()));
                p.rect_stroke(tile.rect, CornerRadius::ZERO, Stroke::new(2.0, t.panel), StrokeKind::Inside);
                if Some(tile.i) == cx.hover || Some(tile.i) == cx.selected {
                    p.rect_stroke(tile.rect.shrink(1.5), CornerRadius::ZERO, Stroke::new(2.0, t.text), StrokeKind::Inside);
                }
            }
            for tile in tiles {
                let r = tile.rect;
                if r.width() < 54.0 || r.height() < 18.0 {
                    continue;
                }
                let n = &cx.nodes[tile.i];
                let ink = theme::ink(tile.depth, t);
                let font = if tile.depth == 1 { theme::semibold(11.5) } else { theme::regular(11.5) };
                if let Some(name) = fit(p, &n.name, 11.5, r.width() - 12.0) {
                    p.text(r.min + vec2(6.0, 4.0), Align2::LEFT_TOP, name, font, ink);
                }
                if !tile.header && r.height() > 34.0 {
                    p.text(r.min + vec2(6.0, 19.0), Align2::LEFT_TOP, crate::fmt_bytes(n.size), theme::regular(11.0), ink);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 2.5D: the same layout, tilted and lifted. No 3D engine: flat polygons in
// painter's order (back to front), so it costs about as much as the 2D map.
// ---------------------------------------------------------------------------

/// How the flat layout is tilted and lifted on screen.
pub struct Tilt {
    /// Ground origin (the sunburst's centre / the treemap's top-left).
    c: Pos2,
    /// Where the ground origin lands on screen (at height 0).
    o: Pos2,
    ex: f32,
    ey: f32,
    /// Treemap: screen x shift per screen y (the city leans; its left and
    /// front faces show).
    sh: f32,
    /// Tallest height, in screen points.
    pub lift: f32,
    max_depth: u8,
    max_leaf: u64,
}

impl Tilt {
    pub fn new(layout: &Layout, nodes: &[Node], area: Rect) -> Self {
        let pad = 10.0;
        let lift = (area.height() * 0.2).clamp(24.0, 120.0);
        match layout {
            Layout::Sun { center, arcs, .. } => {
                let max_depth = arcs.iter().map(|a| a.depth).max().unwrap_or(1).max(1);
                // Low steps: higher inner rings would hide the outer rings' back half.
                let lift = lift.min(9.0 * f32::from(max_depth));
                let r = arcs.iter().map(|a| a.ro).fold(1.0, f32::max);
                let ew = (area.width() - 2.0 * pad).min((area.height() - lift - 2.0 * pad) / 0.5).max(40.0);
                let ex = ew / (2.0 * r);
                let eh = ew * 0.5;
                let o = pos2(area.center().x, area.min.y + pad + lift + eh / 2.0 + ((area.height() - 2.0 * pad - lift - eh) / 2.0).max(0.0));
                Self { c: *center, o, ex, ey: ex * 0.5, sh: 0.0, lift, max_depth, max_leaf: 1 }
            }
            Layout::Tree { tiles } => {
                let g = tiles.iter().fold(Rect::NOTHING, |r, t| r.union(t.rect));
                let (gw, gh) = (g.width().max(1.0), g.height().max(1.0));
                let sh = 0.35;
                let mut ey = (area.height() - lift - 2.0 * pad) / gh;
                let mut ex = (area.width() - 2.0 * pad - gh * ey * sh) / gw;
                if ey > ex * 0.65 {
                    ey = ex * 0.65;
                    ex = (area.width() - 2.0 * pad - gh * ey * sh) / gw;
                }
                let used_w = gw * ex + gh * ey * sh;
                let used_h = gh * ey + lift;
                let o = pos2(area.min.x + (area.width() - used_w) / 2.0, area.min.y + (area.height() - used_h) / 2.0 + lift);
                let max_leaf = tiles.iter().filter(|t| !t.header).map(|t| nodes[t.i].size).max().unwrap_or(1).max(1);
                Self { c: g.min, o, ex, ey, sh, lift, max_depth: 1, max_leaf }
            }
        }
    }

    /// Ground point at height `z` → screen.
    pub fn at(&self, p: Pos2, z: f32) -> Pos2 {
        let d = p - self.c;
        pos2(self.o.x + d.x * self.ex + d.y * self.ey * self.sh, self.o.y + d.y * self.ey - z)
    }

    /// Screen point → ground point, assuming height `z`.
    fn ground(&self, s: Pos2, z: f32) -> Pos2 {
        let gy = (s.y + z - self.o.y) / self.ey;
        let gx = (s.x - self.o.x - gy * self.ey * self.sh) / self.ex;
        self.c + vec2(gx, gy)
    }

    /// Sunburst ring height: inner rings stand higher (a stepped cake).
    fn ring_z(&self, depth: u8) -> f32 {
        let step = self.lift / f32::from(self.max_depth);
        f32::from(self.max_depth.saturating_sub(depth) + 1) * step
    }

    /// Treemap box height: bigger items stand taller (square root, so small
    /// ones still show).
    fn box_z(&self, size: u64) -> f32 {
        (self.lift * (size as f32 / self.max_leaf as f32).sqrt()).max(3.0)
    }
}

fn shade(c: Color32, k: f32) -> Color32 {
    Color32::from_rgb((f32::from(c.r()) * k) as u8, (f32::from(c.g()) * k) as u8, (f32::from(c.b()) * k) as u8)
}

/// Treemap boxes back to front: (tile index, height). Folder headers lie
/// flat on the ground and come first.
fn tree_order(tiles: &[Tile], nodes: &[Node], tilt: &Tilt) -> Vec<(usize, f32)> {
    let mut v: Vec<(usize, f32)> = tiles.iter().enumerate().map(|(k, t)| (k, if t.header { 0.0 } else { tilt.box_z(nodes[t.i].size) })).collect();
    v.sort_by(|a, b| {
        let (ta, tb) = (&tiles[a.0], &tiles[b.0]);
        tb.header.cmp(&ta.header).then(ta.rect.max.y.total_cmp(&tb.rect.max.y)).then(tb.rect.min.x.total_cmp(&ta.rect.min.x))
    });
    v
}

pub fn paint_25d(p: &Painter, layout: &Layout, tilt: &Tilt, cx: &PaintCtx) {
    let t = cx.t;
    let g = cx.grow.clamp(0.0, 1.0);
    match layout {
        Layout::Sun { center, r0, arcs } => {
            // Which drawn arcs have drawn children (their wall stops on top of them).
            let mut parent_of = std::collections::HashSet::new();
            for s in arcs {
                if cx.nodes[s.i].parent > 0 {
                    parent_of.insert(cx.nodes[s.i].parent as usize);
                }
            }
            for d in (1..=tilt.max_depth).rev() {
                let z = tilt.ring_z(d) * g;
                let below = if d < tilt.max_depth { tilt.ring_z(d + 1) * g } else { 0.0 };
                // Outer walls on the front half (map angles π/2..3π/2).
                for s in arcs.iter().filter(|s| s.depth == d) {
                    let (a0, a1) = (s.a0.max(FRAC_PI_2), s.a1.min(3.0 * FRAC_PI_2));
                    if a1 - a0 < 0.002 {
                        continue;
                    }
                    let bottom = if parent_of.contains(&s.i) { below } else { 0.0 };
                    let base = color_of(&cx.nodes[s.i], s.depth, t, cx.today, cx.over.get(s.i).copied());
                    let steps = (((a1 - a0) / TAU) * 160.0).ceil().max(1.0) as u32;
                    let mut m = Mesh::default();
                    for k in 0..=steps {
                        let a = a0 + (a1 - a0) * k as f32 / steps as f32 - FRAC_PI_2;
                        let gp = *center + vec2(a.cos(), a.sin()) * (s.ro - 0.75);
                        // Light from the front: faces turned to the viewer are brighter.
                        let col = shade(base, 0.62 + 0.2 * a.sin().max(0.0));
                        m.colored_vertex(tilt.at(gp, z), col);
                        m.colored_vertex(tilt.at(gp, bottom), col);
                        if k > 0 {
                            let b = k * 2;
                            m.add_triangle(b - 2, b - 1, b);
                            m.add_triangle(b - 1, b + 1, b);
                        }
                    }
                    p.add(Shape::mesh(m));
                }
                // Tops of this ring.
                for s in arcs.iter().filter(|s| s.depth == d) {
                    if s.a1 - s.a0 < 0.002 {
                        continue;
                    }
                    if let Some(mut m) = ring_mesh(*center, s.ri, s.ro, s.a0, s.a1, color_of(&cx.nodes[s.i], s.depth, t, cx.today, cx.over.get(s.i).copied())) {
                        for v in &mut m.vertices {
                            v.pos = tilt.at(v.pos, z);
                        }
                        p.add(Shape::mesh(m));
                    }
                }
            }
            // Pith on top, outlines, centre text.
            let z1 = tilt.ring_z(1) * g;
            if let Some(mut m) = ring_mesh(*center, 0.0, r0 - 3.0, 0.0, TAU, t.panel) {
                for v in &mut m.vertices {
                    v.pos = tilt.at(v.pos, z1);
                }
                p.add(Shape::mesh(m));
            }
            if g >= 1.0 {
                for s in arcs {
                    if Some(s.i) == cx.hover || Some(s.i) == cx.selected {
                        let z = tilt.ring_z(s.depth);
                        let pts = ring_outline(*center, s.ri + 0.75, s.ro - 0.75, s.a0, s.a1).into_iter().map(|q| tilt.at(q, z)).collect();
                        p.add(Shape::closed_line(pts, Stroke::new(2.0, t.text)));
                    }
                }
            }
            let c = tilt.at(*center, z1);
            p.text(c - vec2(0.0, 4.0), Align2::CENTER_CENTER, cx.center_value, theme::semibold(15.0), t.text);
            if cx.can_go_up {
                p.text(c + vec2(0.0, 12.0), Align2::CENTER_CENTER, "↑ up", theme::regular(10.5), t.faint);
            }
        }
        Layout::Tree { tiles } => {
            for (k, z) in tree_order(tiles, cx.nodes, tilt) {
                let tile = &tiles[k];
                let n = &cx.nodes[tile.i];
                let z = z * g;
                let base = color_of(n, tile.depth, t, cx.today, cx.over.get(tile.i).copied());
                let r = tile.rect.shrink(if tile.header { 0.0 } else { 1.0 });
                let (a, b, c, d) = (r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom());
                if z > 0.5 {
                    // Left and front faces (the ones turned to the viewer).
                    p.add(Shape::convex_polygon(vec![tilt.at(a, z), tilt.at(d, z), tilt.at(d, 0.0), tilt.at(a, 0.0)], shade(base, 0.66), Stroke::NONE));
                    p.add(Shape::convex_polygon(vec![tilt.at(d, z), tilt.at(c, z), tilt.at(c, 0.0), tilt.at(d, 0.0)], shade(base, 0.8), Stroke::NONE));
                }
                let top = vec![tilt.at(a, z), tilt.at(b, z), tilt.at(c, z), tilt.at(d, z)];
                let edge = if tile.header { Stroke::new(1.0, t.panel) } else { Stroke::new(0.75, shade(base, 0.85)) };
                p.add(Shape::convex_polygon(top.clone(), base, edge));
                if g >= 1.0 && (Some(tile.i) == cx.hover || Some(tile.i) == cx.selected) {
                    p.add(Shape::closed_line(top, Stroke::new(2.0, t.text)));
                }
                // Names on tops (and folder headers) that are big enough.
                let w = (tilt.at(b, z) - tilt.at(a, z)).length();
                let h = (tilt.at(d, z) - tilt.at(a, z)).length();
                if g >= 1.0 && w > 54.0 && h > 14.0 {
                    let ink = theme::ink(tile.depth, t);
                    if let Some(name) = fit(p, &n.name, 11.0, w - 10.0) {
                        p.text(tilt.at(a, z) + vec2(5.0, 2.0), Align2::LEFT_TOP, name, if tile.header { theme::semibold(11.0) } else { theme::regular(11.0) }, ink);
                    }
                }
            }
        }
    }
}

pub fn hit_25d(layout: &Layout, tilt: &Tilt, nodes: &[Node], p: Pos2) -> Option<Hit> {
    match layout {
        Layout::Sun { center, r0, arcs } => {
            // Inner rings stand higher and are drawn last: test them first.
            for d in 1..=tilt.max_depth {
                let gp = tilt.ground(p, tilt.ring_z(d));
                let v = gp - *center;
                let r = v.length();
                if d == 1 && r < r0 - 3.0 {
                    return Some(Hit::Pith);
                }
                let mut a = v.y.atan2(v.x) + FRAC_PI_2;
                if a < 0.0 {
                    a += TAU;
                }
                if let Some(s) = arcs.iter().find(|s| s.depth == d && r >= s.ri && r < s.ro && a >= s.a0 && a < s.a1) {
                    return Some(Hit::Node(s.i));
                }
            }
            None
        }
        Layout::Tree { tiles } => {
            for (k, z) in tree_order(tiles, nodes, tilt).into_iter().rev() {
                if tiles[k].rect.contains(tilt.ground(p, z)) {
                    return Some(Hit::Node(tiles[k].i));
                }
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// root (100) → a (60) [a1 (40), a2 (20)], b (40).
    fn nodes() -> Vec<Node> {
        vec![
            Node::plain(0, -1, "root".into(), 100, 3, true, 0),
            Node::plain(1, 0, "a".into(), 60, 2, true, 1),
            Node::plain(2, 0, "b".into(), 40, 1, false, 1),
            Node::plain(3, 1, "a1".into(), 40, 1, false, 2),
            Node::plain(4, 1, "a2".into(), 20, 1, false, 2),
        ]
    }

    #[test]
    fn tilted_sunburst_hits_what_is_drawn() {
        let n = nodes();
        let kids = children(&n);
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 500.0));
        let lay = layout(Kind::Sunburst, &n, &kids, area);
        let tilt = Tilt::new(&lay, &n, area);
        let Layout::Sun { center, arcs, .. } = &lay else { panic!() };
        for s in arcs {
            // The middle of each ring piece, on its top face.
            let a = (s.a0 + s.a1) / 2.0 - FRAC_PI_2;
            let g = *center + vec2(a.cos(), a.sin()) * ((s.ri + s.ro) / 2.0);
            let screen = tilt.at(g, tilt.ring_z(s.depth));
            assert!(matches!(hit_25d(&lay, &tilt, &n, screen), Some(Hit::Node(i)) if i == s.i), "piece {} ({})", s.i, n[s.i].name);
        }
        assert!(matches!(hit_25d(&lay, &tilt, &n, tilt.at(*center, tilt.ring_z(1))), Some(Hit::Pith)));
        assert!(hit_25d(&lay, &tilt, &n, pos2(2.0, 2.0)).is_none(), "corner: nothing");
    }

    #[test]
    fn treemap_tiles_are_always_finite() {
        // One huge item and thousands of tiny ones (like a whole drive).
        let mut n = vec![Node::plain(0, -1, "root".into(), 0, 0, true, 0)];
        n.push(Node::plain(1, 0, "big".into(), 1u64 << 40, 1, false, 1));
        for k in 0..3000 {
            n.push(Node::plain(2 + k, 0, format!("f{k}"), 1 + (k as u64 % 7), 1, false, 1));
        }
        n[0].size = n.iter().skip(1).map(|x| x.size).sum();
        let kids = children(&n);
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(1500.0, 700.0));
        let Layout::Tree { tiles } = layout(Kind::Treemap, &n, &kids, area) else { panic!() };
        let finite = |r: Rect| r.min.x.is_finite() && r.min.y.is_finite() && r.max.x.is_finite() && r.max.y.is_finite();
        let bad = tiles.iter().filter(|t| !finite(t.rect)).count();
        assert_eq!(bad, 0, "non-finite tiles");
        let lay = layout(Kind::Treemap, &n, &kids, area);
        let tilt = Tilt::new(&lay, &n, area);
        assert!(tilt.at(pos2(10.0, 10.0), 5.0).x.is_finite(), "projection is finite");
    }

    #[test]
    fn tilted_treemap_hits_what_is_drawn() {
        let n = nodes();
        let kids = children(&n);
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 500.0));
        let lay = layout(Kind::Treemap, &n, &kids, area);
        let tilt = Tilt::new(&lay, &n, area);
        let Layout::Tree { tiles } = &lay else { panic!() };
        for (k, z) in tree_order(tiles, &n, &tilt) {
            let t = &tiles[k];
            let screen = tilt.at(t.rect.center(), z);
            // A box's top is hit as that box (headers can be covered by their children).
            if !t.header {
                assert!(matches!(hit_25d(&lay, &tilt, &n, screen), Some(Hit::Node(i)) if i == t.i), "tile {}", n[t.i].name);
            }
        }
    }
}
