//! Xylem's look: "classic utility, modern finish".
//!
//! Warm paper surfaces, hairline borders, a moss-green accent
//! (the sap), and one botanical colour per category. Segoe UI is loaded
//! from Windows at start-up instead of being bundled (keeps the exe small).

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Visuals,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

/// Design tokens: every colour the UI uses.
#[derive(Clone, Copy)]
pub struct Tokens {
    pub win: Color32,
    pub panel: Color32,
    pub field: Color32,
    pub line: Color32,
    pub line_strong: Color32,
    pub head: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub sel: Color32,
    pub hover: Color32,
    pub accent: Color32,
    pub accent_ink: Color32,
    pub dark: bool,
}

const fn hex(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

pub const LIGHT: Tokens = Tokens {
    win: hex(0xf4f3ef),
    panel: hex(0xfcfbf8),
    field: hex(0xffffff),
    line: hex(0xd9d5cc),
    line_strong: hex(0xbdb8ad),
    head: hex(0xefede7),
    text: hex(0x1e1d1a),
    muted: hex(0x6c6860),
    faint: hex(0x9a958b),
    sel: hex(0xdde8d4),
    hover: hex(0xeeece5),
    accent: hex(0x3e6b35),
    accent_ink: hex(0xffffff),
    dark: false,
};

pub const DARK: Tokens = Tokens {
    win: hex(0x1f201d),
    panel: hex(0x181916),
    field: hex(0x121310),
    line: hex(0x2e302b),
    line_strong: hex(0x41443d),
    head: hex(0x23241f),
    text: hex(0xe8e6df),
    muted: hex(0x9a978d),
    faint: hex(0x6d6a62),
    sel: hex(0x2b3a27),
    hover: hex(0x24261f),
    accent: hex(0x8fbf74),
    accent_ink: hex(0x10140e),
    dark: true,
};

pub fn tokens(t: Theme) -> Tokens {
    match t {
        Theme::Light => LIGHT,
        Theme::Dark => DARK,
    }
}

/// Category colours in the order of `xylem_core::classify::CATEGORIES`:
/// apps, media, docs, dev, ai, system, temp, downloads, cloud, other.
const CAT: [Color32; 10] = [
    hex(0x4a6fa5), // river blue   Apps & Games
    hex(0xb25a8a), // bloom        Media
    hex(0x6f9a3a), // moss         Documents
    hex(0x2f8f83), // lichen       Dev stuff
    hex(0x7a5ea8), // heather      AI & data
    hex(0xb4493b), // heartwood    System
    hex(0xd08a2e), // sap amber    Temp & caches
    hex(0xc9a227), // pollen       Downloads
    hex(0x7f9bb3), // mist         Cloud-synced
    hex(0x8c857b), // bark         Other
];

/// Amber for warnings (cloud, Git, large deletes, integrity notes).
pub const WARN: Color32 = hex(0xb7791f);

pub fn cat_color(cat: u8) -> Color32 {
    CAT.get(cat as usize).copied().unwrap_or(CAT[9])
}

/// Age view, newest to oldest (warm to cool), then "date unknown" (grey).
/// Matches `xylem_session::AGE_BUCKETS`.
const AGE: [Color32; 7] = [
    hex(0xc8452c),
    hex(0xe08a2c),
    hex(0xcfa935),
    hex(0x7fa65a),
    hex(0x4a8fa8),
    hex(0x5f6aa8),
    hex(0x9a958c),
];

pub fn age_color(bucket: usize) -> Color32 {
    AGE.get(bucket).copied().unwrap_or(AGE[6])
}

/// Legend swatches: 0-99 are categories, 100+ age buckets.
pub fn swatch(c: u8) -> Color32 {
    if c >= 100 { age_color(usize::from(c - 100)) } else { cat_color(c) }
}

/// Blend toward the panel colour: deeper rings / inner tiles get lighter
/// (light theme) or darker (dark theme).
pub fn tint(c: Color32, depth: u8, t: &Tokens) -> Color32 {
    let k = ((depth.saturating_sub(1)) as f32 * 0.16).min(0.55);
    let target = if t.dark { [24.0, 25.0, 22.0] } else { [252.0, 251.0, 248.0] };
    let mix = |a: u8, b: f32| (a as f32 * (1.0 - k) + b * k) as u8;
    Color32::from_rgb(mix(c.r(), target[0]), mix(c.g(), target[1]), mix(c.b(), target[2]))
}

/// Label colour that reads on a segment of the given depth.
pub fn ink(depth: u8, t: &Tokens) -> Color32 {
    if t.dark || depth <= 1 { Color32::from_rgba_unmultiplied(255, 255, 255, 235) } else { hex(0x1e1d1a) }
}

/// Loads Segoe UI (+ Semibold) from the Windows fonts folder. Falls back to
/// egui's built-in fonts if they can't be read.
/// Uses only fonts that ship with Windows (nothing is bundled in the exe,
/// so every component of Xylem stays under an OSI-approved license).
/// Each family tries a short fallback list.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::empty();
    let dir = std::env::var_os("WINDIR").map_or_else(|| "C:\\Windows".into(), std::path::PathBuf::from).join("Fonts");
    let mut first = |name: &str, files: &[&str]| -> Option<String> {
        files.iter().find_map(|f| {
            let bytes = std::fs::read(dir.join(f)).ok()?;
            fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_owned(bytes)));
            Some(name.to_owned())
        })
    };
    let regular = first("ui", &["segoeui.ttf", "tahoma.ttf", "arial.ttf"]);
    let semibold = first("ui-semibold", &["seguisb.ttf", "segoeuib.ttf", "tahomabd.ttf", "arialbd.ttf"]);
    let mono = first("mono", &["consola.ttf", "cour.ttf"]);
    let symbols = first("symbols", &["seguisym.ttf"]);
    let chain = |main: &Option<String>| -> Vec<String> {
        [main, &regular, &symbols].into_iter().flatten().cloned().fold(Vec::new(), |mut v, n| {
            if !v.contains(&n) {
                v.push(n);
            }
            v
        })
    };
    fonts.families.insert(FontFamily::Proportional, chain(&regular));
    fonts.families.insert(FontFamily::Monospace, chain(&mono));
    fonts.families.insert(FontFamily::Name("semibold".into()), chain(&semibold));
    ctx.set_fonts(fonts);
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}

/// Applies the tokens to egui's widgets.
pub fn apply(ctx: &egui::Context, theme: Theme) {
    let t = tokens(theme);
    let mut v = if t.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = t.win;
    v.window_fill = t.win;
    v.window_stroke = Stroke::new(1.0, t.line_strong);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(6);
    v.extreme_bg_color = t.field;
    v.faint_bg_color = t.head;
    v.code_bg_color = t.head;
    v.override_text_color = Some(t.text);
    v.hyperlink_color = t.accent;
    v.selection.bg_fill = t.sel;
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.window_shadow.color = Color32::from_black_alpha(if t.dark { 110 } else { 40 });
    v.popup_shadow.color = Color32::from_black_alpha(if t.dark { 90 } else { 30 });

    let r = CornerRadius::same(4);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = t.panel;
    w.noninteractive.weak_bg_fill = t.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.line);
    w.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    w.noninteractive.corner_radius = r;
    for (s, bg, stroke) in [
        (&mut w.inactive, t.field, t.line_strong),
        (&mut w.hovered, t.hover, t.line_strong),
        (&mut w.active, t.sel, t.accent),
        (&mut w.open, t.sel, t.line_strong),
    ] {
        s.bg_fill = bg;
        s.weak_bg_fill = bg;
        s.bg_stroke = Stroke::new(1.0, stroke);
        s.fg_stroke = Stroke::new(1.0, t.text);
        s.corner_radius = r;
        s.expansion = 0.0;
    }
    // Xylem's own Light/Dark choice wins over the Windows theme (egui would
    // otherwise switch to its default dark style when Windows goes dark).
    let egui_theme = if t.dark { egui::Theme::Dark } else { egui::Theme::Light };
    ctx.set_theme(egui_theme);
    ctx.set_visuals_of(egui_theme, v);

    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, regular(11.5)),
            (TextStyle::Body, regular(13.0)),
            (TextStyle::Button, regular(13.0)),
            (TextStyle::Heading, semibold(15.0)),
            (TextStyle::Monospace, FontId::monospace(12.0)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(10.0, 4.0);
        style.spacing.interact_size.y = 26.0;
        style.spacing.menu_margin = egui::Margin::same(4);
        style.interaction.selectable_labels = false;
    });
}
