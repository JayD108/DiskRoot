//! CPU renderer: DiskRoot without a graphics driver.
//!
//! Normally the window is drawn with OpenGL. On PCs where OpenGL can't start
//! (no graphics driver: virtual machines, some Remote Desktop sessions, the
//! "Microsoft Basic Display Adapter"), this draws the same UI on the CPU:
//! egui's triangles are rasterised here, in horizontal bands on several
//! threads, and the finished picture goes to the window through plain GDI
//! (softbuffer). Nothing here needs a GPU, Direct3D or OpenGL.
//!
//! Windows' own software renderer (Direct3D 12 on WARP) was measured first:
//! a fixed ~300-400 ms per frame. This is the lighter path.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, ClippedPrimitive, Color32, TextureId, ViewportBuilder, ViewportCommand, ViewportId,
    ViewportInfo,
};
use eframe::epaint::{Primitive, textures::TexturesDelta};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

// ---------------------------------------------------------------------------
// Rasteriser
// ---------------------------------------------------------------------------

/// One texture: premultiplied RGBA, like egui's Color32.
struct Tex {
    w: usize,
    h: usize,
    px: Vec<Color32>,
    /// Smooth (bilinear) sampling when magnified, e.g. the logo; the font
    /// atlas is drawn pixel-exact.
    linear: bool,
}

#[derive(Default)]
pub struct Raster {
    textures: HashMap<TextureId, Tex>,
}

/// Premultiplied colour as four floats 0..255.
#[derive(Clone, Copy)]
struct C4 {
    r: f32,
    g: f32,
    b: f32,
    a: f32,
}

impl C4 {
    fn of(c: Color32) -> Self {
        Self {
            r: f32::from(c.r()),
            g: f32::from(c.g()),
            b: f32::from(c.b()),
            a: f32::from(c.a()),
        }
    }
}

impl Raster {
    pub fn set_textures(&mut self, delta: &TexturesDelta) {
        for (id, d) in delta
            .set
            .iter()
            .flat_map(|(id, ds)| ds.iter().map(move |d| (id, d)))
        {
            let egui::ImageData::Color(img) = &d.image;
            let linear = d.options.magnification == egui::TextureFilter::Linear
                && *id != TextureId::default();
            match d.pos {
                None => {
                    self.textures.insert(
                        *id,
                        Tex {
                            w: img.size[0],
                            h: img.size[1],
                            px: img.pixels.clone(),
                            linear,
                        },
                    );
                }
                Some([x0, y0]) => {
                    let Some(t) = self.textures.get_mut(id) else {
                        continue;
                    };
                    for y in 0..img.size[1] {
                        for x in 0..img.size[0] {
                            let (tx, ty) = (x0 + x, y0 + y);
                            if tx < t.w && ty < t.h {
                                t.px[ty * t.w + tx] = img.pixels[y * img.size[0] + x];
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn free_textures(&mut self, delta: &TexturesDelta) {
        for id in &delta.free {
            self.textures.remove(id);
        }
    }

    /// Paints `prims` into `buf` (0x00RRGGBB, `w` x `h` pixels), only inside
    /// `area` (x0, y0, x1, y1 in pixels); the rest of `buf` is left as is.
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &self,
        prims: &[ClippedPrimitive],
        ppp: f32,
        buf: &mut [u32],
        w: usize,
        h: usize,
        clear: Color32,
        area: [i32; 4],
    ) {
        let [ax0, ay0, ax1, ay1] = [
            area[0].max(0),
            area[1].max(0),
            area[2].min(w as i32),
            area[3].min(h as i32),
        ];
        if ax0 >= ax1 || ay0 >= ay1 {
            return;
        }
        let bg = (u32::from(clear.r()) << 16) | (u32::from(clear.g()) << 8) | u32::from(clear.b());
        let rows = (ay1 - ay0) as usize;
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .clamp(1, 16);
        let rows_per = rows.div_ceil(threads).max(16);
        let region = &mut buf[ay0 as usize * w..ay1 as usize * w];
        std::thread::scope(|s| {
            for (band, chunk) in region.chunks_mut(rows_per * w).enumerate() {
                let y0 = ay0 as usize + band * rows_per;
                let band_rows = chunk.len() / w;
                let mut work = move || {
                    for row in chunk.chunks_exact_mut(w) {
                        row[ax0 as usize..ax1 as usize].fill(bg);
                    }
                    for p in prims {
                        let Primitive::Mesh(mesh) = &p.primitive else {
                            continue;
                        };
                        let Some(tex) = self.textures.get(&mesh.texture_id) else {
                            continue;
                        };
                        // Clip rect in pixels, intersected with the area and this band.
                        let cx0 = ((p.clip_rect.min.x * ppp).round() as i32).max(ax0);
                        let cy0 = ((p.clip_rect.min.y * ppp).round() as i32).max(y0 as i32);
                        let cx1 = ((p.clip_rect.max.x * ppp).round() as i32).min(ax1);
                        let cy1 =
                            ((p.clip_rect.max.y * ppp).round() as i32).min((y0 + band_rows) as i32);
                        if cx0 >= cx1 || cy0 >= cy1 {
                            continue;
                        }
                        let clip = [cx0, cy0, cx1, cy1];
                        for t in mesh.indices.as_chunks::<3>().0 {
                            let (Some(a), Some(b), Some(c)) = (
                                mesh.vertices.get(t[0] as usize),
                                mesh.vertices.get(t[1] as usize),
                                mesh.vertices.get(t[2] as usize),
                            ) else {
                                continue;
                            };
                            tri(chunk, w, y0, clip, ppp, [a, b, c], tex);
                        }
                    }
                };
                if rows <= rows_per {
                    work(); // small area: no thread
                } else {
                    s.spawn(work);
                }
            }
        });
    }
}

/// Pixel rectangle (x0, y0, x1, y1) a primitive can touch: its clip rect
/// cut down to its vertices, 1 px margin for anti-aliasing.
fn bounds(p: &ClippedPrimitive, ppp: f32) -> [i32; 4] {
    let Primitive::Mesh(m) = &p.primitive else {
        return [0, 0, 0, 0];
    };
    let mut r = egui::Rect::NOTHING;
    for v in &m.vertices {
        r.extend_with(v.pos);
    }
    let r = r.intersect(p.clip_rect);
    if !r.is_positive() {
        return [0, 0, 0, 0];
    }
    [
        (r.min.x * ppp).floor() as i32 - 1,
        (r.min.y * ppp).floor() as i32 - 1,
        (r.max.x * ppp).ceil() as i32 + 1,
        (r.max.y * ppp).ceil() as i32 + 1,
    ]
}

fn same(a: &ClippedPrimitive, b: &ClippedPrimitive) -> bool {
    match (&a.primitive, &b.primitive) {
        (Primitive::Mesh(x), Primitive::Mesh(y)) => a.clip_rect == b.clip_rect && x == y,
        _ => false,
    }
}

/// The pixel rectangle that differs between two frames, or None if they are
/// identical. Compares from both ends, so something added or removed in the
/// middle (a tooltip, a hover highlight) only dirties its own area.
pub fn changed_area(
    old: &[ClippedPrimitive],
    new: &[ClippedPrimitive],
    ppp: f32,
) -> Option<[i32; 4]> {
    let pre = old.iter().zip(new).take_while(|(a, b)| same(a, b)).count();
    let suf = old[pre..]
        .iter()
        .rev()
        .zip(new[pre..].iter().rev())
        .take_while(|(a, b)| same(a, b))
        .count();
    let mut d: Option<[i32; 4]> = None;
    for p in old[pre..old.len() - suf]
        .iter()
        .chain(&new[pre..new.len() - suf])
    {
        let b = bounds(p, ppp);
        if b[0] >= b[2] || b[1] >= b[3] {
            continue;
        }
        d = Some(match d {
            None => b,
            Some(o) => [
                o[0].min(b[0]),
                o[1].min(b[1]),
                o[2].max(b[2]),
                o[3].max(b[3]),
            ],
        });
    }
    if pre == old.len() && pre == new.len() {
        None
    } else {
        Some(d.unwrap_or([0, 0, 0, 0]))
    }
}

/// Rasterises one triangle into the band starting at row `y0`.
fn tri(
    band: &mut [u32],
    w: usize,
    y0: usize,
    clip: [i32; 4],
    ppp: f32,
    v: [&egui::epaint::Vertex; 3],
    tex: &Tex,
) {
    let mut v = v;
    let mut p = v.map(|v| (v.pos.x * ppp, v.pos.y * ppp));
    let mut area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[1].1 - p[0].1) * (p[2].0 - p[0].0);
    if area.abs() < 1e-6 {
        return;
    }
    if area < 0.0 {
        // One winding for all triangles, so the fill rule below works.
        v.swap(1, 2);
        p.swap(1, 2);
        area = -area;
    }
    let minx = (p[0].0.min(p[1].0).min(p[2].0).floor() as i32).max(clip[0]);
    let maxx = (p[0].0.max(p[1].0).max(p[2].0).ceil() as i32).min(clip[2]);
    let miny = (p[0].1.min(p[1].1).min(p[2].1).floor() as i32).max(clip[1]);
    let maxy = (p[0].1.max(p[1].1).max(p[2].1).ceil() as i32).min(clip[3]);
    if minx >= maxx || miny >= maxy {
        return;
    }
    let inv = 1.0 / area;
    let col = v.map(|v| C4::of(v.color));
    let uv = v.map(|v| (v.uv.x * tex.w as f32, v.uv.y * tex.h as f32));
    // Solid fill: same colour everywhere and the atlas' white pixel.
    let white = |u: (f32, f32)| u.0 < 1.0 && u.1 < 1.0;
    let solid = v[0].color == v[1].color
        && v[1].color == v[2].color
        && uv.iter().all(|&u| white(u))
        && tex.px.first().is_some_and(|&c| c == Color32::WHITE);
    // Edge function of edge (i, j) at point (x, y): > 0 inside.
    let e = |i: usize, j: usize, x: f32, y: f32| {
        (p[j].0 - p[i].0) * (y - p[i].1) - (p[j].1 - p[i].1) * (x - p[i].0)
    };
    // Fill rule: a pixel centre exactly on an edge belongs to only one of the
    // two triangles sharing that edge (they run along it in opposite
    // directions), so shared edges aren't drawn twice.
    let owns = |i: usize, j: usize| {
        let (dx, dy) = (p[j].0 - p[i].0, p[j].1 - p[i].1);
        dy > 0.0 || (dy == 0.0 && dx < 0.0)
    };
    let own = [owns(1, 2), owns(2, 0), owns(0, 1)];
    let inside = |e: f32, own: bool| e > 0.0 || (e == 0.0 && own);
    for y in miny..maxy {
        let row = &mut band[(y as usize - y0) * w..(y as usize - y0 + 1) * w];
        let py = y as f32 + 0.5;
        for x in minx..maxx {
            let px = x as f32 + 0.5;
            let (e0, e1, e2) = (e(1, 2, px, py), e(2, 0, px, py), e(0, 1, px, py));
            if !(inside(e0, own[0]) && inside(e1, own[1]) && inside(e2, own[2])) {
                continue;
            }
            let (w0, w1, w2) = (e0 * inv, e1 * inv, e2 * inv);
            let src = if solid {
                col[0]
            } else {
                let c = C4 {
                    r: col[0].r * w0 + col[1].r * w1 + col[2].r * w2,
                    g: col[0].g * w0 + col[1].g * w1 + col[2].g * w2,
                    b: col[0].b * w0 + col[1].b * w1 + col[2].b * w2,
                    a: col[0].a * w0 + col[1].a * w1 + col[2].a * w2,
                };
                let (u, vv) = (
                    uv[0].0 * w0 + uv[1].0 * w1 + uv[2].0 * w2,
                    uv[0].1 * w0 + uv[1].1 * w1 + uv[2].1 * w2,
                );
                let t = sample(tex, u, vv);
                C4 {
                    r: c.r * t.r / 255.0,
                    g: c.g * t.g / 255.0,
                    b: c.b * t.b / 255.0,
                    a: c.a * t.a / 255.0,
                }
            };
            if src.a <= 0.5 {
                continue;
            }
            let d = &mut row[x as usize];
            *d = blend(*d, src);
        }
    }
}

fn sample(t: &Tex, u: f32, v: f32) -> C4 {
    if !t.linear {
        let x = (u as i64).clamp(0, t.w as i64 - 1) as usize;
        let y = (v as i64).clamp(0, t.h as i64 - 1) as usize;
        return C4::of(t.px[y * t.w + x]);
    }
    let (u, v) = (u - 0.5, v - 0.5);
    let (x0, y0) = (u.floor(), v.floor());
    let (fx, fy) = (u - x0, v - y0);
    let at = |x: f32, y: f32| {
        C4::of(
            t.px[(y.clamp(0.0, t.h as f32 - 1.0) as usize) * t.w
                + x.clamp(0.0, t.w as f32 - 1.0) as usize],
        )
    };
    let (a, b, c, d) = (
        at(x0, y0),
        at(x0 + 1.0, y0),
        at(x0, y0 + 1.0),
        at(x0 + 1.0, y0 + 1.0),
    );
    let mix = |p: f32, q: f32, r: f32, s: f32| {
        (p * (1.0 - fx) + q * fx) * (1.0 - fy) + (r * (1.0 - fx) + s * fx) * fy
    };
    C4 {
        r: mix(a.r, b.r, c.r, d.r),
        g: mix(a.g, b.g, c.g, d.g),
        b: mix(a.b, b.b, c.b, d.b),
        a: mix(a.a, b.a, c.a, d.a),
    }
}

/// Premultiplied "over": dst = src + dst * (1 - src.a).
fn blend(dst: u32, s: C4) -> u32 {
    let k = 1.0 - s.a / 255.0;
    let ch = |shift: u32, sc: f32| -> u32 {
        (sc + ((dst >> shift) & 0xFF) as f32 * k).clamp(0.0, 255.0) as u32
    };
    (ch(16, s.r) << 16) | (ch(8, s.g) << 8) | ch(0, s.b)
}

// ---------------------------------------------------------------------------
// Window loop (winit + egui-winit + softbuffer)
// ---------------------------------------------------------------------------

type MakeApp = Box<dyn FnOnce(&egui::Context) -> crate::app::DiskRootApp>;

struct Runner {
    ctx: egui::Context,
    builder: ViewportBuilder,
    make: Option<MakeApp>,
    app: Option<crate::app::DiskRootApp>,
    window: Option<Rc<Window>>,
    state: Option<egui_winit::State>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    raster: Raster,
    /// Last frame, to redraw only what changed.
    prev: Vec<ClippedPrimitive>,
    prev_key: (u32, u32, u32),
    /// DISKROOT_SOFT_FULL: redraw everything every frame (benchmarks).
    always_full: bool,
    info: ViewportInfo,
    repaint_at: Option<Instant>,
    error: Option<String>,
}

impl Runner {
    fn paint(&mut self, el: &ActiveEventLoop) {
        let (Some(window), Some(state), Some(app), Some(surface)) = (
            self.window.clone(),
            self.state.as_mut(),
            self.app.as_mut(),
            self.surface.as_mut(),
        ) else {
            return;
        };
        let mut raw = state.take_egui_input(&window);
        egui_winit::update_viewport_info(&mut self.info, &self.ctx, &window, false);
        raw.viewports.insert(ViewportId::ROOT, self.info.clone());
        let mut out = self.ctx.run_ui(raw, |ui| app.show(ui));
        state.handle_platform_output(&window, out.platform_output);
        // Always keep textures (e.g. the font atlas) up to date, also when
        // nothing is drawn this frame (closing, minimised).
        self.raster.set_textures(&out.textures_delta);
        if let Some(vo) = out.viewport_output.get(&ViewportId::ROOT) {
            if vo.commands.contains(&ViewportCommand::Close) {
                out.textures_delta.clear();
                el.exit();
                return;
            }
            let mut actions = Vec::new();
            egui_winit::process_viewport_commands(
                &self.ctx,
                &mut self.info,
                vo.commands.clone(),
                &window,
                &mut actions,
            );
            self.repaint_at = if vo.repaint_delay.is_zero() {
                window.request_redraw();
                None
            } else if vo.repaint_delay < Duration::from_secs(3600) {
                Some(Instant::now() + vo.repaint_delay)
            } else {
                None
            };
        }
        let size = window.inner_size();
        if let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) // 0 when minimised
            && surface.resize(w, h).is_ok()
            && let Ok(mut buf) = surface.buffer_mut()
        {
            let ppp = out.pixels_per_point;
            let prims = self.ctx.tessellate(std::mem::take(&mut out.shapes), ppp);
            let (wu, hu) = (w.get() as usize, h.get() as usize);
            let full = [0, 0, wu as i32, hu as i32];
            let key = (w.get(), h.get(), ppp.to_bits());
            // Redraw everything when the old picture can't be reused: new
            // buffer, new size or scale, or a texture changed (e.g. new glyphs).
            let area = if buf.age() == 0
                || key != self.prev_key
                || !out.textures_delta.set.is_empty()
                || self.always_full
            {
                Some(full)
            } else {
                changed_area(&self.prev, &prims, ppp)
            };
            if let Some(area) = area {
                let clear = self.ctx.global_style().visuals.panel_fill;
                self.raster
                    .paint(&prims, ppp, &mut buf, wu, hu, clear, area);
            }
            // Always show the whole buffer: cheap, and correct when Windows
            // asks for a repaint because the window was uncovered.
            let _ = buf.present();
            self.prev = prims;
            self.prev_key = key;
        }
        self.raster.free_textures(&out.textures_delta);
        out.textures_delta.clear();
    }
}

impl ApplicationHandler for Runner {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window = match egui_winit::create_window(&self.ctx, el, &self.builder) {
            Ok(w) => Rc::new(w),
            Err(e) => {
                self.error = Some(format!("couldn't open the window: {e}"));
                el.exit();
                return;
            }
        };
        let state = egui_winit::State::new(
            self.ctx.clone(),
            ViewportId::ROOT,
            el,
            Some(window.scale_factor() as f32),
            None,
            Some(8192),
        );
        let surface = softbuffer::Context::new(window.clone())
            .and_then(|c| softbuffer::Surface::new(&c, window.clone()));
        match surface {
            Ok(s) => self.surface = Some(s),
            Err(e) => {
                self.error = Some(format!("couldn't draw into the window: {e}"));
                el.exit();
                return;
            }
        }
        egui_winit::update_viewport_info(&mut self.info, &self.ctx, &window, true);
        if let Some(make) = self.make.take() {
            self.app = Some(make(&self.ctx));
        }
        self.state = Some(state);
        window.request_redraw();
        self.window = Some(window);
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match &event {
            WindowEvent::CloseRequested => {
                el.exit();
                return;
            }
            WindowEvent::RedrawRequested => {
                self.paint(el);
                return;
            }
            _ => {}
        }
        if let (Some(window), Some(state)) = (&self.window, self.state.as_mut()) {
            let r = state.on_window_event(window, &event);
            if r.repaint
                || matches!(
                    event,
                    WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }
                )
            {
                window.request_redraw();
            }
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        match self.repaint_at {
            Some(t) if Instant::now() >= t => {
                self.repaint_at = None;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
                el.set_control_flow(ControlFlow::Wait);
            }
            Some(t) => el.set_control_flow(ControlFlow::WaitUntil(t)),
            None => el.set_control_flow(ControlFlow::Wait),
        }
    }
}

/// Runs DiskRoot drawn on the CPU.
pub fn run(builder: ViewportBuilder, make: MakeApp) -> Result<(), String> {
    let el = EventLoop::new().map_err(|e| e.to_string())?;
    let mut r = Runner {
        ctx: egui::Context::default(),
        builder,
        make: Some(make),
        app: None,
        window: None,
        state: None,
        surface: None,
        raster: Raster::default(),
        prev: Vec::new(),
        prev_key: (0, 0, 0),
        always_full: std::env::var_os("DISKROOT_SOFT_FULL").is_some(),
        info: ViewportInfo::default(),
        repaint_at: None,
        error: None,
    };
    el.run_app(&mut r).map_err(|e| e.to_string())?;
    r.error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rasterises_a_solid_and_a_textured_triangle() {
        let mut r = Raster::default();
        // Font-atlas-like texture: top-left pixel white.
        let mut px = vec![Color32::from_rgba_premultiplied(0, 0, 0, 0); 4 * 4];
        px[0] = Color32::WHITE;
        px[5] = Color32::from_rgba_premultiplied(0, 255, 0, 255); // (1,1) green
        r.textures.insert(
            TextureId::default(),
            Tex {
                w: 4,
                h: 4,
                px,
                linear: false,
            },
        );
        let v = |x: f32, y: f32, u: f32, vv: f32, c: Color32| egui::epaint::Vertex {
            pos: egui::pos2(x, y),
            uv: egui::pos2(u, vv),
            color: c,
        };
        let red = Color32::from_rgb(255, 0, 0);
        // A solid red quad over the left half.
        let mut mesh = egui::epaint::Mesh {
            vertices: vec![
                v(0.0, 0.0, 0.0, 0.0, red),
                v(8.0, 0.0, 0.0, 0.0, red),
                v(8.0, 16.0, 0.0, 0.0, red),
                v(0.0, 16.0, 0.0, 0.0, red),
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            ..Default::default()
        };
        // A green-textured quad over the right half (samples texel (1,1)).
        let wht = Color32::WHITE;
        let uvx = 1.5 / 4.0;
        mesh.vertices.extend([
            v(8.0, 0.0, uvx, uvx, wht),
            v(16.0, 0.0, uvx, uvx, wht),
            v(16.0, 16.0, uvx, uvx, wht),
            v(8.0, 16.0, uvx, uvx, wht),
        ]);
        mesh.indices.extend([4, 5, 6, 4, 6, 7]);
        let prims = vec![ClippedPrimitive {
            clip_rect: egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(16.0, 16.0)),
            primitive: Primitive::Mesh(mesh),
        }];
        let mut buf = vec![0u32; 16 * 16];
        r.paint(
            &prims,
            1.0,
            &mut buf,
            16,
            16,
            Color32::BLACK,
            [0, 0, 16, 16],
        );
        assert_eq!(buf[8 * 16 + 2], 0xFF0000, "left: red");
        assert_eq!(buf[8 * 16 + 12], 0x00FF00, "right: green from the texture");
        // Clipping: nothing outside the clip rect.
        let mesh = egui::epaint::Mesh {
            vertices: vec![
                v(0.0, 0.0, 0.0, 0.0, red),
                v(16.0, 0.0, 0.0, 0.0, red),
                v(16.0, 16.0, 0.0, 0.0, red),
                v(0.0, 16.0, 0.0, 0.0, red),
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            ..Default::default()
        };
        let prims = vec![ClippedPrimitive {
            clip_rect: egui::Rect::from_min_max(egui::pos2(4.0, 4.0), egui::pos2(8.0, 8.0)),
            primitive: Primitive::Mesh(mesh),
        }];
        let mut buf = vec![0u32; 16 * 16];
        r.paint(
            &prims,
            1.0,
            &mut buf,
            16,
            16,
            Color32::BLACK,
            [0, 0, 16, 16],
        );
        assert_eq!(buf[5 * 16 + 5], 0xFF0000);
        assert_eq!(buf[12 * 16 + 12], 0x000000, "clipped away");
        // Half-transparent white over black blends to grey.
        let half = Color32::from_rgba_premultiplied(128, 128, 128, 128);
        let mesh = egui::epaint::Mesh {
            vertices: vec![
                v(0.0, 0.0, 0.0, 0.0, half),
                v(16.0, 0.0, 0.0, 0.0, half),
                v(16.0, 16.0, 0.0, 0.0, half),
                v(0.0, 16.0, 0.0, 0.0, half),
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            ..Default::default()
        };
        let prims = vec![ClippedPrimitive {
            clip_rect: egui::Rect::EVERYTHING,
            primitive: Primitive::Mesh(mesh),
        }];
        let mut buf = vec![0u32; 16 * 16];
        r.paint(
            &prims,
            1.0,
            &mut buf,
            16,
            16,
            Color32::BLACK,
            [0, 0, 16, 16],
        );
        assert_eq!(buf[3 * 16 + 3], 0x808080);
    }

    #[test]
    fn redrawing_only_the_changed_area_matches_a_full_redraw() {
        let mut r = Raster::default();
        r.textures.insert(
            TextureId::default(),
            Tex {
                w: 1,
                h: 1,
                px: vec![Color32::WHITE],
                linear: false,
            },
        );
        let quad = |x0: f32, y0: f32, x1: f32, y1: f32, c: Color32| {
            let v = |x: f32, y: f32| egui::epaint::Vertex {
                pos: egui::pos2(x, y),
                uv: egui::pos2(0.0, 0.0),
                color: c,
            };
            let m = egui::epaint::Mesh {
                vertices: vec![v(x0, y0), v(x1, y0), v(x1, y1), v(x0, y1)],
                indices: vec![0, 1, 2, 0, 2, 3],
                ..Default::default()
            };
            ClippedPrimitive {
                clip_rect: egui::Rect::EVERYTHING,
                primitive: Primitive::Mesh(m),
            }
        };
        let (w, h) = (64usize, 48usize);
        let base = vec![
            quad(0.0, 0.0, 64.0, 10.0, Color32::from_rgb(10, 20, 30)),
            quad(5.0, 30.0, 60.0, 40.0, Color32::from_rgb(200, 0, 0)),
        ];
        // Next frame: a hover highlight appears in the middle, one quad changes colour.
        let mut next = base.clone();
        next.insert(
            1,
            quad(
                20.5,
                12.25,
                40.0,
                25.0,
                Color32::from_rgba_premultiplied(0, 60, 0, 120),
            ),
        );
        next[2] = quad(5.0, 30.0, 60.0, 40.0, Color32::from_rgb(0, 0, 200));
        assert!(
            changed_area(&base, &base, 1.0).is_none(),
            "same frame: nothing to redraw"
        );
        let area = changed_area(&base, &next, 1.0).expect("changed");
        assert!(
            area[1] > 10,
            "the unchanged top bar isn't redrawn: {area:?}"
        );
        let full = [0, 0, w as i32, h as i32];
        let mut partial = vec![0u32; w * h];
        r.paint(&base, 1.0, &mut partial, w, h, Color32::BLACK, full);
        r.paint(&next, 1.0, &mut partial, w, h, Color32::BLACK, area);
        let mut whole = vec![0u32; w * h];
        r.paint(&next, 1.0, &mut whole, w, h, Color32::BLACK, full);
        assert!(partial == whole, "partial redraw differs from a full one");
    }
}
