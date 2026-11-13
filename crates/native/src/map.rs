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
    for (i, r) in top {
        let nest = !kids[i].is_empty() && r.width() > 50.0 && r.height() > 44.0;
        tiles.push(Tile { i, rect: r, header: nest, depth: 1 });
        if nest {
            let inner = Rect::from_min_max(pos2(r.min.x + 2.0, r.min.y + 20.0), pos2(r.max.x - 2.0, r.max.y - 2.0));
            let mut sub: Vec<(usize, f64)> =
                kids[i].iter().filter(|&&k| nodes[k].size > 0).map(|&k| (k, nodes[k].size as f64)).collect();
            let mut placed = Vec::new();
            squarify(&mut sub, inner, &mut placed);
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
fn ring_mesh(c: Pos2, ri: f32, ro: f32, a0: f32, a1: f32, color: Color32) -> Option<Mesh> {
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
