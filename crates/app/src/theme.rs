//! Colours and fonts. Everything on screen is drawn with these.

use std::sync::Arc;

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId};

pub const BG: Color32 = Color32::from_rgb(0xf7, 0xf5, 0xf0);
pub const SIDEBAR: Color32 = Color32::from_rgb(0xef, 0xec, 0xe5);
/// Top bar and status bar.
pub const BAR: Color32 = Color32::from_rgb(0xf1, 0xee, 0xe7);
pub const LINE: Color32 = Color32::from_rgb(0xdf, 0xda, 0xcf);
/// Every second row of a table.
pub const STRIPE: Color32 = Color32::from_rgb(0xf2, 0xef, 0xe9);
/// The now-playing display and the search field.
pub const WELL: Color32 = Color32::from_rgb(0xfc, 0xfb, 0xf8);

pub const TEXT: Color32 = Color32::from_rgb(0x22, 0x20, 0x1d);
pub const MUTED: Color32 = Color32::from_rgb(0x6e, 0x69, 0x60);
pub const FAINT: Color32 = Color32::from_rgb(0xa5, 0x9f, 0x93);

/// Selection in the part of the window that has the keyboard.
pub const ACCENT: Color32 = Color32::from_rgb(0x2f, 0x6f, 0xde);
pub const ON_ACCENT: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
/// A row picked along with others, beside the one the cursor is on.
pub const PICKED: Color32 = Color32::from_rgb(0xcf, 0xdf, 0xf8);
/// Selection elsewhere.
pub const IDLE: Color32 = Color32::from_rgb(0xdd, 0xd8, 0xcc);

/// Something that went wrong.
pub const WARN: Color32 = Color32::from_rgb(0xb4, 0x3c, 0x2e);
/// A liked song's heart.
pub const LOVE: Color32 = Color32::from_rgb(0xd1, 0x45, 0x5c);

/// Stand-in for a missing cover.
pub const NO_COVER: Color32 = Color32::from_rgb(0xe6, 0xe2, 0xd8);
pub const COVER_EDGE: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 0x1c);

/// System fonts to use, best first: (regular, semibold or bold).
const FONT_FILES: &[(&str, &str)] = &[
    ("/usr/share/fonts/opentype/inter/Inter-Regular.otf", "/usr/share/fonts/opentype/inter/Inter-SemiBold.otf"),
    ("/usr/share/fonts/opentype/fira/FiraSans-Regular.otf", "/usr/share/fonts/opentype/fira/FiraSans-SemiBold.otf"),
    ("/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf"),
];

/// System fonts that fill in characters the font in use lacks: the typographic hyphen (U+2010) MusicBrainz writes in
/// names, symbols, and Japanese titles. Each is used if it is there.
const FALLBACK_FILES: &[&str] = &[
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansMath-Regular.ttf",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
];

const STRONG: &str = "strong";

pub struct Fonts {
    pub body: FontId,
    pub small: FontId,
    pub strong: FontId,
    /// Column headers and the sidebar heading.
    pub label: FontId,
    pub title: FontId,
}

impl Fonts {
    pub fn new() -> Self {
        let strong = || FontFamily::Name(STRONG.into());
        Fonts {
            body: FontId::proportional(13.5),
            small: FontId::proportional(12.0),
            strong: FontId::new(13.5, strong()),
            label: FontId::new(11.0, strong()),
            title: FontId::new(22.0, strong()),
        }
    }
}

/// Puts the first available pair of system fonts in front of egui's built-in
/// ones, which stay as the fallback for missing glyphs or missing files.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut strong = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();

    let pair = FONT_FILES.iter().find_map(|(regular, bold)| Some((std::fs::read(regular).ok()?, std::fs::read(bold).ok()?)));
    let found = pair.is_some();
    if let Some((regular, bold)) = pair {
        fonts.font_data.insert("ui".to_owned(), Arc::new(FontData::from_owned(regular)));
        fonts.font_data.insert("ui-strong".to_owned(), Arc::new(FontData::from_owned(bold)));
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "ui".to_owned());
        strong.insert(0, "ui".to_owned());
        strong.insert(0, "ui-strong".to_owned());
    }
    // Behind the font in use, ahead of egui's own.
    let mut at = usize::from(found);
    for path in FALLBACK_FILES {
        let Ok(data) = std::fs::read(path) else { continue };
        fonts.font_data.insert((*path).to_owned(), Arc::new(FontData::from_owned(data)));
        fonts.families.entry(FontFamily::Proportional).or_default().insert(at, (*path).to_owned());
        strong.insert(at + usize::from(found), (*path).to_owned());
        at += 1;
    }
    fonts.families.insert(FontFamily::Name(STRONG.into()), strong);
    ctx.set_fonts(fonts);
}
