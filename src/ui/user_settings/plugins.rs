//! Settings > Plugins: search, All / On / Off, "3 of 48 turned on", then one divided row per
//! plugin (name, two lines of description, a gear that opens its options under the row, the
//! switch), docs/DESIGN.md §12.
//! Values live in plugins.toml (`crate::plugins`), not in `config::Settings`.
//! OWNERSHIP: plugins engineer.

use super::super::icons::{self, Icon};
use super::super::messages::{Btn, Pal, button, icon_button, small_button, vector_button};
use super::{switch, switch_row};
use crate::plugins::{self, META, Meta, Opt, OptKind};
use crate::ui::{UiCx, hint, semibold};
use egui::{Align, ComboBox, Frame, Label, Layout, Margin, RichText, Sense, Slider, Stroke, TextEdit, vec2};

const FILTERS: [(usize, &str); 3] = [(0, "All"), (1, "On"), (2, "Off")];

fn query_id() -> egui::Id {
    egui::Id::unique("rc_plugins_query")
}

fn filter_id() -> egui::Id {
    egui::Id::unique("rc_plugins_filter")
}

fn open_id(m: &Meta) -> egui::Id {
    egui::Id::unique(("rc_plugin_options", m.name))
}

/// Whether a plugin matches the search text (name or description, any case).
fn matches(m: &Meta, q: &str) -> bool {
    let q = q.trim().to_lowercase();
    q.is_empty() || m.name.to_lowercase().contains(&q) || m.description.to_lowercase().contains(&q)
}

pub fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    let pal = Pal::new(cx.theme);
    hint(ui, cx.theme, "Vencord's plugins, rebuilt into Rustcord. They only change this app; none sends anything on its own.");
    ui.add_space(16.0 - ui.spacing().item_spacing.y);
    // Test hook RUSTCORD_PLUGIN_SEARCH=<text>: the search filled in (screenshots).
    let mut q: String = ui.data(|d| d.get_temp(query_id())).unwrap_or_else(|| std::env::var("RUSTCORD_PLUGIN_SEARCH").unwrap_or_default());
    let mut filter: usize = ui.data(|d| d.get_temp(filter_id())).unwrap_or(0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let seg_w = ui.available_width().min(480.0) * 0.4;
        let search_w = (ui.available_width() - seg_w - 12.0).max(120.0);
        search_field(ui, &pal, cx.theme.colors.code_background.to_color32(), &mut q, search_w);
        ui.allocate_ui(vec2(seg_w, 36.0), |ui| crate::ui::login::segmented(ui, cx.theme, &mut filter, &FILTERS));
    });
    let on_count = META.iter().filter(|m| plugins::enabled(m.id)).count();
    let list: Vec<&Meta> = META
        .iter()
        .filter(|m| matches(m, &q))
        .filter(|m| match filter {
            1 => plugins::enabled(m.id),
            2 => !plugins::enabled(m.id),
            _ => true,
        })
        .collect();
    ui.add_space(4.0);
    hint(ui, cx.theme, &format!("{on_count} of {} turned on", META.len()));
    ui.add_space(4.0);
    if list.is_empty() {
        // §9 empty state: what is missing, what to do, the one button that does it.
        ui.add_space(32.0);
        ui.vertical_centered(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::hover());
            icons::paint(ui.painter(), Icon::Search, r, pal.muted, pal.bg);
            ui.add_space(16.0);
            ui.label(RichText::new("No plugins match").font(semibold(pal.heading())).color(pal.text));
            ui.label(RichText::new("Try another word, or show every plugin.").size(pal.meta()).color(pal.muted));
            ui.add_space(16.0);
            if button(ui, &pal, Btn::Secondary, "Show all plugins", true).clicked() {
                (q, filter) = (String::new(), 0);
            }
        });
    }
    ui.data_mut(|d| {
        d.insert_temp(query_id(), q);
        d.insert_temp(filter_id(), filter);
    });
    // Test hook: a search that leaves one plugin also opens its options (screenshots).
    let open_first = list.len() == 1 && std::env::var_os("RUSTCORD_PLUGIN_SEARCH").is_some();
    for m in list {
        row(ui, cx, &pal, m, open_first);
    }
}

/// The search well (§9 inputs, 32 px for search and filters): `well` fill, 1 px `edge` border,
/// the Search icon inside at the left; the focus ring replaces the border.
fn search_field(ui: &mut egui::Ui, pal: &Pal, well: egui::Color32, q: &mut String, w: f32) {
    let frame = Frame::new().fill(well).corner_radius(pal.radius).inner_margin(Margin { left: 8, right: 12, top: 0, bottom: 0 });
    let out = frame.show(ui, |ui| {
        // Inner width: `w` minus the frame's 20 px margins; the field gets what the icon leaves.
        let inner = (w - 20.0).max(80.0);
        ui.set_min_size(vec2(inner, 36.0));
        ui.set_max_width(inner);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            super::super::messages::vector_icon(ui, Icon::Search, 16.0, pal.muted, well);
            ui.add(TextEdit::singleline(q).hint_text("Search plugins").frame(Frame::NONE).desired_width(inner - 24.0))
        })
        .inner
    });
    let (rect, edit) = (out.response.rect, out.inner);
    if edit.has_focus() {
        super::super::messages::focus_ring(ui, pal, rect, f32::from(pal.radius));
    } else {
        let edge = if edit.hovered() { pal.muted } else { pal.tk.edge };
        ui.painter().rect_stroke(rect, pal.radius, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
    }
}

/// One plugin row: name (15/600) and up to two lines of description on the left, the gear (if it
/// has options) and the switch on the right, a hairline under it; the options open under it.
fn row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, m: &Meta, open_first: bool) {
    let mut open = ui.data(|d| d.get_temp::<bool>(open_id(m))).unwrap_or(open_first);
    let mut on = plugins::enabled(m.id);
    let has_opts = !m.opts.is_empty() || m.id == plugins::Plugin::TextReplace;
    Frame::new().inner_margin(Margin::symmetric(0, 12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            let controls = 40.0 + if has_opts { 24.0 + 8.0 } else { 0.0 };
            let text_w = (ui.available_width() - controls - 16.0).max(80.0);
            ui.allocate_ui_with_layout(vec2(text_w, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.add(Label::new(RichText::new(m.name).font(semibold(pal.body())).color(pal.text)).truncate());
                let mut job = egui::text::LayoutJob::simple(m.description.to_owned(), egui::FontId::proportional(pal.meta()), pal.muted, text_w);
                job.wrap.max_rows = 2;
                let g = ui.painter().layout_job(job);
                let (r, resp) = ui.allocate_exact_size(g.size(), Sense::hover());
                let elided = g.elided;
                ui.painter().galley(r.min, g, pal.muted);
                if elided {
                    resp.on_hover_text(m.description);
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if switch(ui, pal, &mut on, m.name).changed() {
                    if !on && m.id == plugins::Plugin::MessageLogger {
                        plugins::message_logger::purge(cx.state);
                    }
                    save(cx, plugins::set(m.id, "enabled", toml::Value::Boolean(on)));
                }
                if has_opts && vector_button(ui, pal, Icon::Gear, &format!("{} settings", m.name), 24.0, open, pal.text).clicked() {
                    open = !open;
                }
            });
        });
        if open {
            ui.add_space(4.0);
            for o in m.opts {
                option(ui, cx, pal, m, o);
            }
            if m.id == plugins::Plugin::TextReplace {
                text_rules(ui, cx, pal);
            }
            if m.id == plugins::Plugin::Translate {
                translate_key(ui, cx, pal);
            }
        }
    });
    let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(line.x_range(), line.center().y, Stroke::new(1.0, pal.tk.rule));
    ui.data_mut(|d| d.insert_temp(open_id(m), open));
}

/// TextReplace's rules: find, replacement, "Aa" (case-sensitive) and remove per row, then "Add
/// rule". Typing changes them in memory; leaving a field, a toggle, add or remove saves.
fn text_rules(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal) {
    use crate::plugins::compose::{Rule, rules, rules_value};
    let mut list = rules();
    let (mut changed, mut store, mut remove) = (false, false, None);
    let gap = ui.spacing().item_spacing.x;
    let w = ((ui.available_width() - 48.0 - 3.0 * gap) / 2.0).max(60.0);
    if !list.is_empty() {
        // Column labels over the two fields.
        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 14.0), egui::Sense::hover());
        for (k, t) in ["Find", "Replace with"].into_iter().enumerate() {
            ui.painter().text(egui::pos2(head.left() + k as f32 * (w + gap), head.center().y), egui::Align2::LEFT_CENTER, t, semibold(pal.label()), pal.muted);
        }
    }
    for (i, r) in list.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            for (k, (text, hint_text)) in [(&mut r.find, "Find"), (&mut r.replace, "Replace with")].into_iter().enumerate() {
                let resp = ui.add(TextEdit::singleline(text).id_salt(("rc_text_rule", i, k)).hint_text(hint_text).desired_width(w));
                changed |= resp.changed();
                store |= resp.lost_focus();
            }
            // A toggled icon button (§9): on = bright on the `selected` fill; the tooltip says which.
            let tip = if r.case { "Match case: on" } else { "Match case: off" };
            if icon_button(ui, pal, "Aa", tip, 24.0, r.case, pal.text).clicked() {
                r.case = !r.case;
                (changed, store) = (true, true);
            }
            if vector_button(ui, pal, Icon::Close, "Remove rule", 24.0, false, pal.danger).clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        list.remove(i);
        (changed, store) = (true, true);
    }
    if button(ui, pal, Btn::Secondary, "Add rule", true).clicked() {
        list.push(Rule::default());
        (changed, store) = (true, true);
    }
    if changed {
        plugins::put(plugins::Plugin::TextReplace, "rules", rules_value(&list));
    }
    if store {
        save(cx, plugins::save());
    }
    hint(ui, cx.theme, "Plain text, applied once to each message you send. \\n in a replacement is a new line.");
}

/// Translate's DeepL key: a password field (Enter or leaving it saves a non-empty value into the
/// credential store and clears the field), "Remove key" while one is saved. The key is never shown again.
fn translate_key(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal) {
    use crate::plugins::translate;
    let id = egui::Id::unique("rc_deepl_key");
    let mut draft: String = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    let mut error: Option<String> = ui.data(|d| d.get_temp(id.with("error")));
    let saved = translate::has_key();
    let mut resp_slot = None;
    ui.add_space(8.0);
    ui.label(RichText::new("DeepL API key").color(pal.text));
    let mut remove = false;
    ui.horizontal(|ui| {
        let w = if saved { ui.available_width() - 96.0 } else { f32::INFINITY };
        let hint = if saved { "Saved" } else { "Optional" };
        resp_slot = Some(ui.add(TextEdit::singleline(&mut draft).password(true).hint_text(hint).id_salt("rc_deepl_key_field").desired_width(w)));
        remove = saved && small_button(ui, pal, "Remove key", pal.danger).clicked();
    });
    if remove {
        match translate::clear_key() {
            Ok(()) => error = None,
            Err(e) => cx.view.toast(format!("Could not remove the key: {e}"), true),
        }
    }
    let resp = resp_slot.expect("field drawn");
    if resp.changed() {
        error = None;
    }
    let commit = resp.lost_focus() && !draft.trim().is_empty();
    if commit {
        match translate::set_key(&draft) {
            Ok(()) => draft.clear(),
            Err(e) if e.starts_with("That doesn't") => error = Some(e),
            // No key text in it.
            Err(e) => cx.view.toast(format!("Could not save the key: {e}"), true),
        }
    }
    if let Some(e) = &error {
        ui.label(RichText::new(e).size(pal.meta()).color(pal.danger));
    }
    hint(ui, cx.theme, "With a key, DeepL translates instead of Google. The key stays in your system's credential store.");
    ui.data_mut(|d| {
        d.insert_temp(id, draft);
        match error {
            Some(e) => {
                d.insert_temp(id.with("error"), e);
            }
            None => {
                d.remove_temp::<String>(id.with("error"));
            }
        }
    });
}

fn save(cx: &mut UiCx<'_>, r: Result<(), String>) {
    if let Err(e) = r {
        cx.view.toast(format!("Could not save plugin settings: {e}"), true);
    }
}

fn option(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, m: &Meta, o: &Opt) {
    let p = m.id;
    match o.kind {
        OptKind::Bool(_) => {
            let mut v = plugins::opt_bool(p, o.key);
            if switch_row(ui, cx.theme, o.label, None, &mut v) {
                save(cx, plugins::set(p, o.key, toml::Value::Boolean(v)));
            }
        }
        OptKind::Choice(list, _) => {
            let cur = plugins::opt_choice(p, o.key);
            ui.horizontal(|ui| {
                ui.label(RichText::new(o.label).color(pal.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ComboBox::from_id_salt((m.name, o.key)).selected_text(list[cur]).icon(super::combo_icon).show_ui(ui, |ui| {
                        for (i, c) in list.iter().enumerate() {
                            if ui.selectable_label(cur == i, *c).clicked() && cur != i {
                                save(cx, plugins::set(p, o.key, toml::Value::Integer(i as i64)));
                            }
                        }
                    });
                });
            });
        }
        OptKind::Int(lo, hi, _) => {
            let mut v = plugins::opt_int(p, o.key);
            ui.label(RichText::new(o.label).color(pal.text));
            let resp = ui.add(Slider::new(&mut v, lo..=hi));
            if resp.changed() {
                plugins::put(p, o.key, toml::Value::Integer(v));
            }
            // Written once the drag ends, not on every frame of it.
            if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                save(cx, plugins::save());
            }
        }
        OptKind::Text(_, hint_text) => {
            let mut v = plugins::opt_text(p, o.key);
            ui.label(RichText::new(o.label).color(pal.text));
            let resp = ui.add(TextEdit::singleline(&mut v).hint_text(hint_text).desired_width(f32::INFINITY));
            if resp.changed() {
                plugins::put(p, o.key, toml::Value::String(v));
            }
            if resp.lost_focus() {
                save(cx, plugins::save());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_matches_name_or_description() {
        let m = &META[0];
        assert!(matches(m, ""));
        assert!(matches(m, &m.name.to_uppercase()));
        assert!(!matches(m, "zzzz-no-plugin"));
        assert!(META.iter().filter(|m| matches(m, "url")).count() >= 2);
    }
}
