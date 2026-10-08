//! Shared chrome colors. egui selects the current palette from live OS events.

use egui::{Color32, Context, Theme, ThemePreference, Visuals};

use crate::config::Appearance;

#[derive(Clone, Copy)]
pub struct Palette {
    pub background: Color32,
    pub panel: Color32,
    pub bar: Color32,
    pub accent: Color32,
    pub muted: Color32,
    pub error: Color32,
    pub good: Color32,
    pub ink: Color32,
    pub disabled: Color32,
    pub selection: Color32,
    pub hover: Color32,
}

impl Palette {
    pub fn from_ctx(ctx: &Context) -> Self {
        Self::new(ctx.theme() == Theme::Dark)
    }

    pub fn new(dark: bool) -> Self {
        if dark {
            Self {
                background: Color32::from_rgb(24, 29, 36),
                panel: Color32::from_rgb(31, 37, 46),
                bar: Color32::from_rgb(38, 44, 54),
                accent: Color32::from_rgb(104, 180, 255),
                muted: Color32::from_rgb(163, 174, 188),
                error: Color32::from_rgb(255, 125, 125),
                good: Color32::from_rgb(99, 207, 134),
                ink: Color32::from_rgb(224, 230, 238),
                disabled: Color32::from_rgb(111, 121, 136),
                selection: Color32::from_rgb(47, 76, 108),
                hover: Color32::from_rgb(55, 64, 77),
            }
        } else {
            Self {
                background: Color32::from_rgb(250, 250, 251),
                panel: Color32::from_rgb(243, 244, 246),
                bar: Color32::from_rgb(236, 237, 240),
                accent: Color32::from_rgb(0, 104, 214),
                muted: Color32::from_rgb(100, 106, 118),
                error: Color32::from_rgb(196, 40, 40),
                good: Color32::from_rgb(24, 122, 55),
                ink: Color32::from_rgb(40, 44, 54),
                disabled: Color32::from_rgb(170, 174, 182),
                selection: Color32::from_rgb(205, 226, 250),
                hover: Color32::from_rgb(226, 229, 235),
            }
        }
    }
}

pub fn choose(ctx: &Context, light: Color32, dark: Color32) -> Color32 {
    if ctx.theme() == Theme::Dark { dark } else { light }
}

pub fn apply(ctx: &Context, appearance: Appearance, system: Option<Theme>) {
    let preference = match appearance {
        // Native detection is independent of a window's manual appearance override.
        Appearance::System => system.map_or(ThemePreference::System, Into::into),
        Appearance::Light => ThemePreference::Light,
        Appearance::Dark => ThemePreference::Dark,
    };
    if ctx.options(|options| options.theme_preference) != preference {
        ctx.set_theme(preference);
        ctx.request_repaint();
    }
}

pub fn setup(ctx: &Context, appearance: Appearance) {
    // Keep the familiar light view when a desktop cannot report its appearance.
    ctx.options_mut(|options| options.fallback_theme = Theme::Light);
    for theme in [Theme::Light, Theme::Dark] {
        let colors = Palette::new(theme == Theme::Dark);
        let mut visuals = if theme == Theme::Dark { Visuals::dark() } else { Visuals::light() };
        visuals.panel_fill = colors.panel;
        visuals.window_fill = colors.background;
        visuals.selection.bg_fill = colors.selection;
        visuals.selection.stroke.color = colors.ink;
        visuals.hyperlink_color = colors.accent;
        visuals.error_fg_color = colors.error;
        ctx.set_visuals_of(theme, visuals);
    }
    ctx.all_styles_mut(|style| style.spacing.item_spacing = egui::vec2(6.0, 5.0));
    apply(ctx, appearance, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_system_detection_overrides_stale_window_events() {
        let ctx = Context::default();
        setup(&ctx, Appearance::System);
        apply(&ctx, Appearance::System, Some(Theme::Light));
        assert_eq!(ctx.theme(), Theme::Light);
        // While the window is manually Dark, the OS also becomes Dark. Native
        // window libraries may suppress that event, so returning to System must
        // consult the independent OS reading instead of the old Light event.
        apply(&ctx, Appearance::Dark, Some(Theme::Light));
        apply(&ctx, Appearance::System, Some(Theme::Dark));
        assert_eq!(ctx.theme(), Theme::Dark);
        apply(&ctx, Appearance::System, Some(Theme::Light));
        assert_eq!(ctx.theme(), Theme::Light);
        apply(&ctx, Appearance::Light, Some(Theme::Dark));
        assert_eq!(ctx.theme(), Theme::Light, "manual choices override native detection");
    }

    fn luminance(color: Color32) -> f64 {
        let linear = |v: u8| { let v = f64::from(v) / 255.0; if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) } };
        0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
    }

    #[test]
    fn text_colors_remain_readable_on_both_palettes() {
        for dark in [false, true] {
            let colors = Palette::new(dark);
            for foreground in [colors.ink, colors.muted, colors.accent, colors.error, colors.good] {
                for background in [colors.background, colors.panel, colors.bar] {
                    let (a, b) = (luminance(foreground), luminance(background));
                    let contrast = (a.max(b) + 0.05) / (a.min(b) + 0.05);
                    assert!(contrast >= 4.5, "dark={dark}, {foreground:?} on {background:?}: {contrast}");
                }
            }
        }
    }
}
