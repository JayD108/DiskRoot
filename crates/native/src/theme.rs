//! DiskRoot's look. Two styles:
//! - **Studio** (default): the "DiskRoot Studio" design. Near-black panes
//!   (dark) or alabaster (Daylight), hairline borders, an emerald accent,
//!   clear category colours, numbers in a monospaced font so they line up.
//! - **Classic**: warm paper surfaces, a moss-green accent and one
//!   botanical colour per category.
//!
//! Segoe UI and Consolas are loaded from Windows at start-up instead of
//! being bundled (keeps the exe small; nothing to license).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
    /// Corner radius of panes (widgets use a little less).
    pub radius: u8,
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
    faint: hex(0x6f6a62),
    sel: hex(0xdde8d4),
    hover: hex(0xeeece5),
    accent: hex(0x3e6b35),
    accent_ink: hex(0xffffff),
    dark: false,
    radius: 4,
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
    faint: hex(0x8c887f),
    sel: hex(0x2b3a27),
    hover: hex(0x24261f),
    accent: hex(0x8fbf74),
    accent_ink: hex(0x10140e),
    dark: true,
    radius: 4,
};

/// Studio dark ("Root Void" / "Pane Glass"); hairlines are the prototype's
/// white-at-6% / 11% over the pane colour.
pub const STUDIO_DARK: Tokens = Tokens {
    win: hex(0x07090c),
    panel: hex(0x0d1117),
    field: hex(0x0a0d12),
    line: hex(0x1c2026),
    line_strong: hex(0x2a2e34),
    head: hex(0x121821),
    text: hex(0xe6edf3),
    muted: hex(0x8a95a3),
    faint: hex(0x747e8c),
    sel: hex(0x0f2a24),
    hover: hex(0x151b23),
    accent: hex(0x10b981),
    accent_ink: hex(0x03140d),
    dark: true,
    radius: 8,
};

/// Studio "Daylight".
pub const STUDIO_LIGHT: Tokens = Tokens {
    win: hex(0xf6f5f2),
    panel: hex(0xffffff),
    field: hex(0xfbfaf8),
    line: hex(0xe7eaee),
    line_strong: hex(0xd8dee6),
    head: hex(0xf8f7f4),
    text: hex(0x0f172a),
    muted: hex(0x586476),
    faint: hex(0x687181),
    sel: hex(0xe2f6ef),
    hover: hex(0xf1f0ec),
    accent: hex(0x047857),
    accent_ink: hex(0xffffff),
    dark: false,
    radius: 8,
};

/// Classic look instead of Studio (View → Look). Read wherever colours are
/// picked, so it's a flag, not a parameter on every call.
static CLASSIC: AtomicBool = AtomicBool::new(false);

pub fn set_classic(on: bool) {
    CLASSIC.store(on, Ordering::Relaxed);
}

pub fn classic() -> bool {
    CLASSIC.load(Ordering::Relaxed)
}

pub fn tokens(t: Theme) -> Tokens {
    match (t, classic()) {
        (Theme::Light, true) => LIGHT,
        (Theme::Dark, true) => DARK,
        (Theme::Light, false) => STUDIO_LIGHT,
        (Theme::Dark, false) => STUDIO_DARK,
    }
}

/// Category colours in the order of `diskroot_core::classify::CATEGORIES`:
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

/// Studio category colours (same order).
const STUDIO_CAT: [Color32; 10] = [
    hex(0x3b82f6), // Apps & Games
    hex(0xa855f7), // Media
    hex(0x22c55e), // Documents
    hex(0x14b8a6), // Dev stuff
    hex(0x6366f1), // AI & data
    hex(0xef4444), // System
    hex(0xf59e0b), // Temp & caches
    hex(0xeab308), // Downloads
    hex(0x06b6d4), // Cloud-synced
    hex(0x64748b), // Other
];

/// Amber for warnings (cloud, Git, large deletes, integrity notes): one
/// that stays readable (WCAG 4.5:1) on the theme's background.
pub fn warn() -> Color32 {
    if IS_DARK.load(Ordering::Relaxed) { hex(0xc58a2a) } else { hex(0x9a6412) }
}

/// The theme in use is dark (set by `apply`, read by `warn`).
static IS_DARK: AtomicBool = AtomicBool::new(false);

pub fn cat_color(cat: u8) -> Color32 {
    let set = if classic() { &CAT } else { &STUDIO_CAT };
    set.get(cat as usize).copied().unwrap_or(set[9])
}

/// Age view, newest to oldest (warm to cool), then "date unknown" (grey).
/// Matches `diskroot_session::AGE_BUCKETS`.
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

/// Map modes. Growth: grew a lot, grew, unchanged, shrank, new, no data.
/// Reclaimable: safe to clear, check first, everything else.
pub const GROWTH: [Color32; 6] = [hex(0xc8452c), hex(0xe3a33f), hex(0xa9a59d), hex(0x4f9a52), hex(0x7d62c4), hex(0xd3cfc7)];
pub const RECLAIM: [Color32; 3] = [hex(0x3f9a4a), hex(0xd99a2b), hex(0xc2bdb4)];

/// Legend swatches: 0-99 are categories, 100-199 age buckets, 200-209
/// growth, 210-219 reclaimable.
pub fn swatch(c: u8) -> Color32 {
    match c {
        210.. => RECLAIM.get(usize::from(c - 210)).copied().unwrap_or(RECLAIM[2]),
        200.. => GROWTH.get(usize::from(c - 200)).copied().unwrap_or(GROWTH[5]),
        100.. => age_color(usize::from(c - 100)),
        _ => cat_color(c),
    }
}

/// Blend toward the panel colour: deeper rings / inner tiles get lighter
/// (light theme) or darker (dark theme).
pub fn tint(c: Color32, depth: u8, t: &Tokens) -> Color32 {
    let k = ((depth.saturating_sub(1)) as f32 * 0.16).min(0.55);
    let target = [f32::from(t.panel.r()), f32::from(t.panel.g()), f32::from(t.panel.b())];
    let mix = |a: u8, b: f32| (a as f32 * (1.0 - k) + b * k) as u8;
    Color32::from_rgb(mix(c.r(), target[0]), mix(c.g(), target[1]), mix(c.b(), target[2]))
}

/// Label colour that reads on a segment of the given depth.
pub fn ink(depth: u8, t: &Tokens) -> Color32 {
    if t.dark || depth <= 1 { Color32::from_rgba_unmultiplied(255, 255, 255, 235) } else { t.text }
}

/// Loads Segoe UI (+ Semibold) from the Windows fonts folder. Falls back to
/// egui's built-in fonts if they can't be read.
/// Uses only fonts that ship with Windows (nothing is bundled in the exe,
/// so every component of DiskRoot stays under an OSI-approved license).
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
    let mono = first("mono", &["CascadiaMono.ttf", "consola.ttf", "cour.ttf"]);
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
    ctx.set_fonts(fonts.clone());
    if let Ok(mut f) = FONTS.lock() {
        *f = Some((fonts, 0));
    }
}

/// The fonts in use and which extra scripts are loaded (bit per `SCRIPTS`
/// entry), so a script's font can be added when a name needs it.
static FONTS: std::sync::Mutex<Option<(FontDefinitions, u32)>> = std::sync::Mutex::new(None);

/// Scripts Segoe UI doesn't cover, and Windows' own font for each. These
/// fonts are big (5-19 MB), so each is loaded only when a file or folder
/// name on screen needs it.
/// (name, font files to try, does this character need it).
type Script = (&'static str, &'static [&'static str], fn(u32) -> bool);
const SCRIPTS: [Script; 5] = [
    ("cjk", &["msyh.ttc", "YuGothR.ttc", "simsun.ttc", "msgothic.ttc"], |c| {
        (0x2E80..=0x9FFF).contains(&c) || (0xF900..=0xFAFF).contains(&c) || (0xFF00..=0xFFEF).contains(&c) || (0x2_0000..=0x2_FFFF).contains(&c)
    }),
    ("hangul", &["malgun.ttf"], |c| (0x1100..=0x11FF).contains(&c) || (0x3130..=0x318F).contains(&c) || (0xAC00..=0xD7AF).contains(&c)),
    ("indic", &["Nirmala.ttc", "Nirmala.ttf"], |c| (0x0900..=0x0DFF).contains(&c)),
    ("thai", &["LeelawUI.ttf"], |c| (0x0E00..=0x0EFF).contains(&c)),
    ("emoji", &["seguiemj.ttf"], |c| (0x1_F000..=0x1_FAFF).contains(&c)),
];

/// Makes sure every script in `names` can be drawn: loads Windows' font for
/// a script the first time a name uses it (Chinese, Japanese, Korean, Hindi
/// and other Indian scripts, Thai, emoji). Cheap when nothing new is needed.
pub fn cover<'a>(ctx: &egui::Context, names: impl IntoIterator<Item = &'a str>) {
    let mut need = 0u32;
    for n in names {
        if n.is_ascii() {
            continue;
        }
        for ch in n.chars() {
            let c = ch as u32;
            if c < 0x0900 {
                continue;
            }
            for (k, (_, _, test)) in SCRIPTS.iter().enumerate() {
                if test(c) {
                    need |= 1 << k;
                }
            }
        }
    }
    let Ok(mut guard) = FONTS.lock() else { return };
    let Some((fonts, loaded)) = guard.as_mut() else { return };
    let new = need & !*loaded;
    if new == 0 {
        return;
    }
    let dir = std::env::var_os("WINDIR").map_or_else(|| "C:\\Windows".into(), std::path::PathBuf::from).join("Fonts");
    for (k, (name, files, _)) in SCRIPTS.iter().enumerate() {
        if new & (1 << k) == 0 {
            continue;
        }
        *loaded |= 1 << k; // tried once, found or not
        let Some(bytes) = files.iter().find_map(|f| std::fs::read(dir.join(f)).ok()) else { continue };
        fonts.font_data.insert((*name).to_owned(), Arc::new(FontData::from_owned(bytes)));
        for fam in fonts.families.values_mut() {
            fam.push((*name).to_owned());
        }
    }
    ctx.set_fonts(fonts.clone());
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}

/// Sizes and counts. Segoe UI's regular digits all have the same width
/// (tabular), so columns of numbers line up without a monospaced font.
pub fn num(size: f32) -> FontId {
    regular(size)
}

/// Applies the tokens to egui's widgets.
pub fn apply(ctx: &egui::Context, theme: Theme) {
    let t = tokens(theme);
    IS_DARK.store(t.dark, Ordering::Relaxed);
    let mut v = if t.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = t.win;
    v.window_fill = t.win;
    v.window_stroke = Stroke::new(1.0, t.line_strong);
    v.window_corner_radius = CornerRadius::same(t.radius + 4);
    v.menu_corner_radius = CornerRadius::same(t.radius);
    v.extreme_bg_color = t.field;
    v.faint_bg_color = t.head;
    v.code_bg_color = t.head;
    v.override_text_color = Some(t.text);
    v.hyperlink_color = t.accent;
    v.selection.bg_fill = t.sel;
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.window_shadow.color = Color32::from_black_alpha(if t.dark { 110 } else { 40 });
    v.popup_shadow.color = Color32::from_black_alpha(if t.dark { 90 } else { 30 });

    let r = CornerRadius::same(if t.radius > 4 { 6 } else { 4 });
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
    // DiskRoot's own Light/Dark choice wins over the Windows theme (egui would
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
