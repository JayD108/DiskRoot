//! Size/RAM spike for a native Xylem UI (egui + OpenGL, no browser engine).
//! Draws a sunburst (stem cross-section) and a ranked list from demo data,
//! repainting only on input, so it idles at ~0% CPU. Not production code.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui::{self, Color32, Mesh, Pos2, Sense, Shape, Stroke, pos2, vec2};
use std::f32::consts::TAU;

struct Node {
    name: &'static str,
    size: f32,
    color: Color32,
    kids: Vec<Node>,
}

fn leaf(name: &'static str, size: f32, color: Color32) -> Node {
    Node { name, size, color, kids: vec![] }
}

fn dir(name: &'static str, color: Color32, kids: Vec<Node>) -> Node {
    let size = kids.iter().map(|k| k.size).sum();
    Node { name, size, color, kids }
}

fn demo() -> Node {
    let apps = Color32::from_rgb(0x4a, 0x6f, 0xa5);
    let ai = Color32::from_rgb(0x7a, 0x5e, 0xa8);
    let dev = Color32::from_rgb(0x2f, 0x8f, 0x83);
    let sys = Color32::from_rgb(0xb4, 0x49, 0x3b);
    let temp = Color32::from_rgb(0xd0, 0x8a, 0x2e);
    let docs = Color32::from_rgb(0x6f, 0x9a, 0x3a);
    dir("C:\\", Color32::GRAY, vec![
        dir("datasets", ai, vec![leaf("satellite-2019", 182.0, ai), leaf("raw", 140.0, ai), leaf("eeg", 96.0, ai), leaf("wavelets", 71.5, ai)]),
        dir("Program Files (x86)", apps, vec![dir("Steam", apps, vec![leaf("RPG", 96.0, apps), leaf("Shooter", 72.0, apps), leaf("Racing", 61.0, apps)]), leaf("Other", 11.8, apps)]),
        dir("Program Files", apps, vec![leaf("Epic Games", 177.0, apps), leaf("Other", 22.6, apps)]),
        dir("Users", docs, vec![dir("AppData", temp, vec![leaf("Temp", 33.0, temp), leaf("Programs", 16.3, apps), leaf("DXCache", 8.1, temp)]), dir("projects", dev, vec![leaf("node_modules", 12.7, dev), leaf(".venv", 4.4, dev)]), leaf("Documents", 36.7, docs)]),
        dir("Windows", sys, vec![leaf("WinSxS", 15.9, sys), leaf("System32", 13.1, sys)]),
        leaf("pagefile.sys", 22.0, sys),
    ])
}

/// Filled ring segment as a triangle strip (works for any span).
fn ring_segment(c: Pos2, r0: f32, r1: f32, a0: f32, a1: f32, color: Color32) -> Shape {
    let steps = (((a1 - a0) / TAU) * 180.0).ceil().max(2.0) as usize;
    let mut mesh = Mesh::default();
    for i in 0..=steps {
        let a = a0 + (a1 - a0) * i as f32 / steps as f32 - TAU / 4.0;
        let (s, co) = a.sin_cos();
        mesh.colored_vertex(pos2(c.x + co * r0, c.y + s * r0), color);
        mesh.colored_vertex(pos2(c.x + co * r1, c.y + s * r1), color);
        if i > 0 {
            let b = (i as u32) * 2;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b + 1, b);
        }
    }
    Shape::mesh(mesh)
}

fn lighten(c: Color32, t: f32) -> Color32 {
    let m = |x: u8| (x as f32 + (252.0 - x as f32) * t) as u8;
    Color32::from_rgb(m(c.r()), m(c.g()), m(c.b()))
}

struct App {
    root: Node,
}

impl App {
    fn sunburst(&self, painter: &egui::Painter, c: Pos2, r_pith: f32, ring: f32) {
        fn walk(p: &egui::Painter, n: &Node, c: Pos2, a0: f32, a1: f32, d: usize, r_pith: f32, ring: f32) {
            if d > 0 {
                let r0 = r_pith + (d - 1) as f32 * ring;
                p.add(ring_segment(c, r0 + 1.0, r0 + ring - 1.0, a0 + 0.004, a1 - 0.004, lighten(n.color, (d - 1) as f32 * 0.16)));
            }
            if d >= 3 {
                return;
            }
            let mut a = a0;
            for k in &n.kids {
                let span = (a1 - a0) * k.size / n.size;
                walk(p, k, c, a, a + span, d + 1, r_pith, ring);
                a += span;
            }
        }
        walk(painter, &self.root, c, 0.0, TAU, 0, r_pith, ring);
        painter.circle(c, r_pith - 3.0, Color32::from_rgb(0xfc, 0xfb, 0xf8), Stroke::new(1.0, Color32::from_rgb(0xd9, 0xd5, 0xcc)));
        painter.text(c, egui::Align2::CENTER_CENTER, format!("{:.2} TB", self.root.size / 1024.0), egui::FontId::proportional(16.0), Color32::from_rgb(0x1e, 0x1d, 0x1a));
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| { let _ = ui.button("Scan drive"); });
                ui.menu_button("View", |ui| { let _ = ui.button("Treemap"); });
                ui.menu_button("Help", |ui| { let _ = ui.button("About"); });
            });
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.label("Ready · 1,473,330 files · MFT 1.48 s · Read-only");
        });
        egui::Panel::left("tree").default_size(420.0).show(ui, |ui| {
            egui::Grid::new("rows").striped(true).num_columns(2).show(ui, |ui| {
                for k in &self.root.kids {
                    ui.colored_label(k.color, format!("■ {}", k.name));
                    ui.label(format!("{:.1} GB", k.size));
                    ui.end_row();
                }
            });
        });
        egui::CentralPanel::default().show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
            let r = rect.width().min(rect.height()) / 2.0 - 10.0;
            let pith = (r * 0.27).max(46.0);
            self.sunburst(ui.painter(), rect.center(), pith, (r - pith) / 3.0);
        });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("Xylem (native spike)").with_inner_size(vec2(1280.0, 800.0)),
        ..Default::default()
    };
    eframe::run_native("Xylem native spike", options, Box::new(|_cc| Ok(Box::new(App { root: demo() }))))
}
