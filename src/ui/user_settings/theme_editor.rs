//! Live theme editor (Settings > Rustcord > Theme Editor): colors, dark/light base, app gradient
//! (colours, angle, panel opacity), fonts, font size, corner radius; save as user theme, revert,
//! import/export TOML. Cozy / compact is Appearance's Message Display. Also the round theme
//! swatches of Appearance's picker.
//! Every edit changes `cx.theme` directly (live preview); nothing is written until "Save as…".
//! OWNERSHIP: user-settings engineer (docs/SOCIAL_UI_DESIGN.md).

use crate::ui::icons::Icon;
use crate::ui::{UiCx, ViewState};
use crate::theme::{GradientStop, Rgba, Theme, ThemeGradient};
use egui::{RichText, Sense, SliderClamping, Stroke, vec2};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// UI-only editor state, kept in egui temp memory.
#[derive(Clone, Default)]
struct EditorState {
    /// Pass the editor was last drawn in; a gap means Settings was closed in between.
    pass: u64,
    /// Installed fonts `(file stem, path)`, scanned on first use per opening of Settings.
    fonts: Option<Arc<Vec<(String, PathBuf)>>>,
    /// "Save as" name, and the theme name it was defaulted from.
    save_name: String,
    save_for: String,
}

/// Returns true when `cx.settings` changed (caller saves settings).
pub fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>) -> bool {
    let id = egui::Id::unique("theme_editor");
    let mut st = ui.ctx().data_mut(|d| d.remove_temp::<EditorState>(id)).unwrap_or_default();
    let pass = ui.ctx().cumulative_pass_nr();
    if st.pass + 1 != pass {
        st.fonts = None; // Settings was reopened: rescan on next use (fonts may have been installed)
    }
    st.pass = pass;
    if st.save_for != cx.theme.name {
        st.save_for = cx.theme.name.clone();
        st.save_name = default_save_name(cx.theme);
    }
    // Against the list settings.rs loads once per opening: this runs every frame, no disk reads.
    let unsaved = cx.view.settings_themes.iter().flatten().find(|t| t.name == cx.theme.name) != Some(&*cx.theme);

    let mut edited = false;
    // Gradient edits recolour the theme from the gradient (`Theme::retint`).
    let mut retint = false;
    ui.add_space(10.0);
    // Wrapping row, not grid rows: keeps the grid narrow enough for 720 px windows at large font sizes.
    ui.horizontal_wrapped(|ui| {
        crate::ui::button_row(ui);
        let t = &mut *cx.theme;
        ui.label("Base");
        retint |= ui.selectable_value(&mut t.dark, true, "Dark").changed();
        retint |= ui.selectable_value(&mut t.dark, false, "Light").changed();
        edited |= retint;
    });
    // Its own row: the toggle stays with its label, the colours follow it.
    let pal = crate::ui::messages::Pal::new(cx.theme);
    ui.horizontal_wrapped(|ui| {
        crate::ui::button_row(ui);
        let t = &mut *cx.theme;
        ui.label("Gradient").on_hover_text("Panels and text are recoloured from the gradient");
        let mut on = t.gradient.is_some();
        let off_on = ui.selectable_value(&mut on, false, "Off").changed() | ui.selectable_value(&mut on, true, "On").changed();
        if off_on {
            let c = &t.colors;
            t.gradient = on.then(|| ThemeGradient { angle: 135.0, stops: spaced(&[c.accent, c.link]), overlay: 1.0 });
            (retint, edited) = (true, true);
        }
        if let Some(g) = &mut t.gradient {
            ui.add_space(16.0);
            for stop in &mut g.stops {
                let mut rgba = stop.color.0;
                if ui.color_edit_button_srgba_unmultiplied(&mut rgba).changed() {
                    stop.color.0 = rgba;
                    retint = true;
                }
            }
            let colors: Vec<Rgba> = g.stops.iter().map(|s| s.color).collect();
            if colors.len() < 5 && crate::ui::messages::vector_button(ui, &pal, Icon::Plus, "Add a colour", 24.0, false, pal.text).clicked() {
                g.stops = spaced(&[&colors[..], &colors[colors.len() - 1..]].concat());
                retint = true;
            }
            if colors.len() > 1 && crate::ui::messages::vector_button(ui, &pal, Icon::Minus, "Remove the last colour", 24.0, false, pal.text).clicked() {
                g.stops = spaced(&colors[..colors.len() - 1]);
                retint = true;
            }
        }
    });
    ui.add_space(4.0);
    // Labels wrap on the narrowest page at large sizes: wider fonts (DejaVu Sans on Linux) overflow it.
    let max_col = (ui.available_width() * 0.45).max(120.0);
    egui::Grid::new("theme_editor").num_columns(2).spacing([16.0, 6.0]).max_col_width(max_col).show(ui, |ui| {
        ui.spacing_mut().slider_width = 90.0; // with the label column, fits 384 px up to font size 24
        let t = &mut *cx.theme;
        let muted = t.colors.text_muted.to_color32();
        let c = &mut t.colors;
        let rows: [(&str, &mut Rgba); 13] = [
            ("Background", &mut c.background),
            ("Sidebar", &mut c.sidebar),
            ("Server bar", &mut c.guild_bar),
            ("Surface", &mut c.surface),
            ("Text", &mut c.text),
            ("Muted text", &mut c.text_muted),
            ("Primary", &mut c.accent),
            ("Text on primary", &mut c.accent_text),
            ("Message hover", &mut c.message_hover),
            ("Mention highlight", &mut c.mention_highlight),
            ("Links", &mut c.link),
            ("Danger", &mut c.danger),
            ("Code background", &mut c.code_background),
        ];
        for (label, color) in rows {
            ui.label(label);
            ui.horizontal(|ui| {
                // Written back only on change: the picker's linear round trip must not mark a theme edited.
                let mut rgba = color.0;
                if ui.color_edit_button_srgba_unmultiplied(&mut rgba).changed() {
                    color.0 = rgba;
                    edited = true;
                }
                // Not monospace: its taller line box leaves the text a few px above the swatch.
                ui.label(RichText::new(color.to_hex()).color(muted));
            });
            ui.end_row();
        }

        ui.label("UI font");
        edited |= font_combo(ui, "theme_ui_font", &mut t.font.ui_font, &mut st, cx.view);
        ui.end_row();
        ui.label("Monospace font");
        edited |= font_combo(ui, "theme_mono_font", &mut t.font.mono_font, &mut st, cx.view);
        ui.end_row();
        ui.label("Font size");
        let size = egui::Slider::new(&mut t.font.size, 10.0..=24.0).step_by(0.5).clamping(SliderClamping::Edits);
        // Applied when the drag ends: a live size change moves the slider under the pointer (feedback loop).
        let r = ui.add(size);
        edited |= r.drag_stopped() || (r.changed() && !r.dragged());
        ui.end_row();
        ui.label("Corner radius");
        let radius = egui::Slider::new(&mut t.layout.corner_radius, 0.0..=16.0).step_by(1.0).clamping(SliderClamping::Edits);
        edited |= ui.add(radius).changed();
        ui.end_row();
        if let Some(g) = &mut t.gradient {
            ui.label("Gradient angle");
            edited |= ui.add(egui::Slider::new(&mut g.angle, 0.0..=360.0).step_by(1.0).suffix("°")).changed();
            ui.end_row();
            ui.label("Panel opacity");
            let opacity = egui::Slider::new(&mut g.overlay, 0.5..=1.0).step_by(0.05).clamping(SliderClamping::Edits);
            retint |= ui.add(opacity).on_hover_text("Lower shows more of the gradient").changed();
            ui.end_row();
        }
    });
    if retint {
        cx.theme.retint();
        edited = true;
    }

    let mut settings_changed = false;
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.add(egui::TextEdit::singleline(&mut st.save_name).hint_text("Theme name").char_limit(64).desired_width(200.0));
        let dir = crate::config::paths().themes_dir();
        if ui.button("Save as…").on_hover_text(format!("Saves a .toml file in {}", dir.display())).clicked() {
            let mut theme = cx.theme.clone();
            theme.name = st.save_name.trim().to_owned();
            let name = &theme.name;
            // Only the theme being edited may be overwritten; save_user refuses empty and built-in names.
            let taken = *name != cx.theme.name && !theme.is_builtin() && cx.view.settings_themes.iter().flatten().any(|t| t.name == *name);
            let result = if taken {
                Err(format!("a theme named \"{name}\" already exists. Select it to overwrite it, or choose another name"))
            } else {
                theme.save_user()
            };
            match result {
                Ok(()) => {
                    cx.view.toast(format!("Saved theme \"{}\"", theme.name), false);
                    select(cx, theme);
                    settings_changed = true;
                }
                Err(e) => cx.view.toast(format!("Could not save theme: {e}"), true),
            }
        }
        if ui.button("Import…").clicked()
            && let Some(path) = rfd::FileDialog::new().add_filter("Theme", &["toml"]).pick_file()
        {
            match Theme::import_from(&path) {
                Ok(theme) => {
                    cx.view.toast(format!("Imported theme \"{}\"", theme.name), false);
                    select(cx, theme);
                    settings_changed = true;
                }
                Err(e) => cx.view.toast(e, true),
            }
        }
        if ui.button("Export…").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Theme", &["toml"])
                .set_file_name(format!("{}.toml", crate::theme::sanitize_file_name(&cx.theme.name)))
                .save_file()
        {
            match cx.theme.export_to(&path) {
                Ok(()) => cx.view.toast(format!("Exported to {}", path.display()), false),
                Err(e) => cx.view.toast(format!("Could not export theme: {e}"), true),
            }
        }
    });
    if unsaved {
        ui.horizontal(|ui| {
            // Amber: it needs attention, with its word (never the primary).
            let hint = RichText::new("Unsaved changes").size(cx.theme.font.size - 3.0).color(cx.theme.tokens().attention);
            ui.label(hint).on_hover_text("Kept until restart or until you pick another theme; use Save as… to keep them.");
            if ui.small_button("Revert").clicked() {
                *cx.theme = Theme::find(&cx.theme.name);
                edited = true;
            }
        });
    }

    if edited || settings_changed {
        cx.view.theme_dirty = true;
        ui.ctx().request_repaint();
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, st));
    settings_changed
}

/// Make `theme` the active one and have settings.rs reload its theme list.
fn select(cx: &mut UiCx<'_>, theme: Theme) {
    cx.settings.theme = theme.name.clone();
    *cx.theme = theme;
    cx.view.settings_themes = None;
}

/// Gradient stops of `colors`, evenly spaced from 0 to 100 %.
fn spaced(colors: &[Rgba]) -> Vec<GradientStop> {
    let last = colors.len().saturating_sub(1).max(1) as f32;
    colors.iter().enumerate().map(|(i, &color)| GradientStop { color, at: i as f32 * 100.0 / last }).collect()
}

/// Round swatches of `themes` (flat ones, or the gradient ones) added to `ui` (a wrapping row);
/// the selected theme carries a 2 px `text` ring and a check on `positive` (§12; never the
/// accent). Returns the clicked theme.
pub(super) fn swatches(ui: &mut egui::Ui, themes: &[Theme], current: &Theme, gradients: bool) -> Option<Theme> {
    let c = &current.colors;
    let tk = current.tokens();
    let mut picked = None;
    for t in themes.iter().filter(|t| t.gradient.is_some() == gradients) {
        let selected = t.name == current.name;
        let (rect, resp) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::click());
        if ui.is_rect_visible(rect) {
            let p = ui.painter();
            let (centre, r) = (rect.center(), 19.0);
            match &t.gradient {
                Some(g) => {
                    p.add(crate::theme::swatch_mesh(g, centre, r));
                }
                None => {
                    p.circle_filled(centre, r, t.colors.background.to_color32());
                    p.circle_filled(centre, 6.0, t.colors.accent.to_color32());
                }
            }
            // Also smooths the mesh's edge.
            p.circle_stroke(centre, r, Stroke::new(1.5, c.text_muted.to_color32().gamma_multiply(0.35)));
            if selected || resp.hovered() || resp.has_focus() {
                let ring = if selected { c.text } else { c.text_muted }.to_color32();
                p.circle_stroke(centre, r + 3.5, Stroke::new(2.0, ring));
            }
            if selected {
                let badge = rect.right_top() + vec2(-8.0, 8.0);
                p.circle_filled(badge, 8.0, tk.accent);
                crate::ui::icons::paint(p, Icon::Check, egui::Rect::from_center_size(badge, egui::Vec2::splat(12.0)), tk.on_accent, tk.accent);
            }
        }
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, selected, &t.name));
        if resp.on_hover_text(&t.name).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            picked = Some(t.clone());
        }
    }
    picked
}

fn default_save_name(theme: &Theme) -> String {
    if theme.is_builtin() { format!("{} custom", theme.name) } else { theme.name.clone() }
}

/// Built-in / Browse… / installed fonts. Returns true when `path` changed.
fn font_combo(ui: &mut egui::Ui, salt: &str, path: &mut String, st: &mut EditorState, view: &mut ViewState) -> bool {
    let current = match Path::new(path.as_str()).file_stem() {
        Some(stem) => stem.to_string_lossy().into_owned(),
        None => "Built-in".to_owned(), // empty path
    };
    let mut picked: Option<PathBuf> = None;
    egui::ComboBox::from_id_salt(salt).selected_text(current).width(160.0).icon(super::combo_icon).show_ui(ui, |ui| {
        if ui.selectable_label(path.is_empty(), "Built-in").clicked() {
            picked = Some(PathBuf::new());
        }
        if ui.selectable_label(false, "Browse…").clicked() {
            picked = rfd::FileDialog::new().add_filter("Fonts", &["ttf", "otf"]).pick_file();
        }
        ui.separator();
        let fonts = st.fonts.get_or_insert_with(|| Arc::new(scan_fonts(&font_dirs())));
        for (name, p) in fonts.iter() {
            if ui.selectable_label(Path::new(path.as_str()) == p, name.as_str()).clicked() {
                picked = Some(p.clone());
            }
        }
    });
    let Some(new) = picked else { return false };
    if !new.as_os_str().is_empty() && !crate::theme::is_usable_font(&new) {
        view.toast(format!("{} is not a usable TrueType/OpenType font", new.display()), true);
        return false;
    }
    let new = new.to_string_lossy().into_owned();
    let changed = *path != new;
    *path = new;
    changed
}

/// Folders that hold installed fonts on this OS.
fn font_dirs() -> Vec<PathBuf> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    if cfg!(windows) {
        let mut dirs = vec![env("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()).join("Fonts")];
        dirs.extend(env("LOCALAPPDATA").map(|d| d.join("Microsoft\\Windows\\Fonts"))); // per-user installs
        return dirs;
    }
    let mut dirs: Vec<PathBuf> =
        ["/usr/share/fonts", "/usr/local/share/fonts", "/System/Library/Fonts", "/Library/Fonts"].map(PathBuf::from).into();
    if let Some(home) = env("HOME") {
        dirs.extend([".local/share/fonts", ".fonts", "Library/Fonts"].map(|d| home.join(d)));
    }
    dirs
}

/// `(file stem, path)` of every .ttf/.otf in `dirs` (subfolders a few levels deep), sorted
/// case-insensitively; the first file wins when stems repeat.
fn scan_fonts(dirs: &[PathBuf]) -> Vec<(String, PathBuf)> {
    fn walk(dir: &Path, depth: u8, out: &mut Vec<(String, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
            if path.is_dir() {
                if depth > 0 {
                    walk(&path, depth - 1, out);
                }
            } else if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("ttf") || x.eq_ignore_ascii_case("otf")) {
                out.push((path.file_stem().unwrap_or_default().to_string_lossy().into_owned(), path));
            }
        }
    }
    let mut out = Vec::new();
    for dir in dirs {
        walk(dir, 3, &mut out);
    }
    out.sort_by_cached_key(|(name, _)| name.to_lowercase()); // stable: earlier folders first among equals
    out.dedup_by(|a, b| a.0.to_lowercase() == b.0.to_lowercase());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_finds_fonts_sorted_and_deduped() {
        let root = std::env::temp_dir().join(format!("rustcord-fonts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (a, b) = (root.join("a"), root.join("b"));
        std::fs::create_dir_all(a.join("sub")).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        for f in [a.join("zeta.ttf"), a.join("Alpha.OTF"), a.join("readme.txt"), a.join("sub").join("mid.ttf"), b.join("alpha.ttf")] {
            std::fs::write(f, b"x").unwrap();
        }
        let found = scan_fonts(&[a.clone(), b, root.join("missing")]);
        let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Alpha", "mid", "zeta"]);
        assert_eq!(found[0].1, a.join("Alpha.OTF")); // first folder wins
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fits_the_narrowest_settings_page() {
        // 720 px window - 2 x 12 inset - 208 nav column - 2 x 40 page margin (`settings_layer`).
        const W: f32 = 408.0;
        let ctx = egui::Context::default();
        let mut applier = crate::theme::ThemeApplier::default();
        let (mut state, mut view, mut settings, mut theme) = (Default::default(), ViewState::default(), Default::default(), Theme::nightshade());
        let mut overrides = Default::default();
        let cmd = crate::backend::CommandSender(tokio::sync::mpsc::unbounded_channel().0);
        let mut images = crate::ui::images::ImageCache::new(cmd.clone());
        let mut cx = UiCx { state: &mut state, view: &mut view, images: &mut images, theme: &mut theme, settings: &mut settings, cmd: &cmd, overrides: &mut overrides };
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(W, 2000.0))), ..Default::default() };
        // The gradient rows (5 colour buttons, two more sliders) too.
        for (size, name) in [(14.5, "Nightshade"), (24.0, "Nightshade"), (24.0, "Chroma Glow")] {
            *cx.theme = Theme::find(name);
            cx.theme.font.size = size;
            applier.apply(&ctx, cx.theme);
            let mut width = 0.0;
            for _ in 0..2 {
                let out = ctx.run_ui(input.clone(), |ui| {
                    ui.set_max_width(W);
                    show(ui, &mut cx);
                    width = ui.min_rect().width();
                });
                out.drop_without_applying_deltas();
            }
            assert!(width > 200.0 && width <= W, "editor is {width} px wide at font size {size}");
        }
    }

    #[test]
    fn font_size_slider_holds_still_while_pressed() {
        // Applying each drag step live widened the label column, moved the slider under the
        // held pointer and made the size oscillate.
        let ctx = egui::Context::default();
        let mut applier = crate::theme::ThemeApplier::default();
        let (mut state, mut view, mut settings, mut theme) = (Default::default(), ViewState::default(), Default::default(), Theme::nightshade());
        let mut overrides = Default::default();
        let cmd = crate::backend::CommandSender(tokio::sync::mpsc::unbounded_channel().0);
        let mut images = crate::ui::images::ImageCache::new(cmd.clone());
        let mut cx = UiCx { state: &mut state, view: &mut view, images: &mut images, theme: &mut theme, settings: &mut settings, cmd: &cmd, overrides: &mut overrides };
        applier.apply(&ctx, cx.theme);
        // One frame the way RustcordApp::ui runs it: re-apply the theme when the editor marks it dirty.
        let mut frame = |cx: &mut UiCx<'_>, events: Vec<egui::Event>| {
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 2000.0));
            ctx.run_ui(egui::RawInput { screen_rect: Some(rect), events, ..Default::default() }, |ui| {
                show(ui, cx);
            })
            .drop_without_applying_deltas();
            if std::mem::take(&mut cx.view.theme_dirty) {
                applier.apply(&ctx, cx.theme);
            }
        };
        frame(&mut cx, vec![]);
        // The font size slider: the first of the two 90 px wide draggable rails.
        let slider = ctx.viewport(|v| {
            let mut rails: Vec<egui::Rect> = v.prev_pass.widgets.layers().flat_map(|(_, w)| w).filter(|w| w.sense.senses_drag() && w.rect.width() == 90.0).map(|w| w.rect).collect();
            rails.sort_by(|a, b| a.top().total_cmp(&b.top()));
            rails[0]
        });
        let pos = slider.lerp_inside(egui::vec2(0.85, 0.5));
        let button = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&mut cx, vec![egui::Event::PointerMoved(pos), button(true)]);
        let pressed = cx.theme.font.size;
        assert!(pressed > 14.5, "press at 85% of the rail set {pressed}");
        for _ in 0..10 {
            frame(&mut cx, vec![egui::Event::PointerMoved(pos)]);
            assert_eq!(cx.theme.font.size, pressed);
        }
        frame(&mut cx, vec![button(false)]);
        for _ in 0..3 {
            frame(&mut cx, vec![]);
            assert_eq!(cx.theme.font.size, pressed);
        }
        assert_eq!(ctx.global_style().text_styles[&egui::TextStyle::Body].size, pressed); // applied on release
    }

    #[test]
    fn gradient_stops_are_spaced_evenly() {
        let (a, b) = (Rgba::rgb(1, 2, 3), Rgba::rgb(4, 5, 6));
        let at = |c: &[Rgba]| spaced(c).iter().map(|s| s.at).collect::<Vec<_>>();
        assert_eq!(at(&[a]), [0.0]);
        assert_eq!(at(&[a, b]), [0.0, 100.0]);
        assert_eq!(at(&[a, b, a, b, a]), [0.0, 25.0, 50.0, 75.0, 100.0]);
        assert_eq!(spaced(&[a, b])[1].color, b);
    }

    #[test]
    fn save_name_defaults() {
        assert_eq!(default_save_name(&Theme::nightshade()), "Nightshade custom");
        let mine = Theme { name: "Mine".into(), ..Theme::nightshade() };
        assert_eq!(default_save_name(&mine), "Mine");
    }
}
