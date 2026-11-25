//! My Account in Discord's order: the account card (banner, avatar, name, Edit User Profile;
//! Display Name, Username with the password, Email and Phone masked with Reveal), then
//! Password and Authentication, Account Standing and Account Removal (security.rs). Email and
//! phone changes need Discord's verification flows, so they open discord.com in the browser.
//! And Profiles (avatar and banner upload, pronouns, About Me, banner colour, live preview
//! card). Each change is one `AccountCommand` on a click, progress from `AccountState::ops`;
//! USER_UPDATE then refreshes `state.me`.
//! OWNERSHIP: user-settings engineer (docs/SOCIAL_UI_DESIGN.md).

/// Password and Authentication, Account Standing, Account Removal and the 2FA dialogs.
#[path = "security.rs"]
pub(super) mod security;

use super::{Page, field_label, finish_op, my_op, op_line, op_state, primary_button};
use crate::api::cdn;
use crate::auth::Secret;
use crate::backend::{AccountCommand, Command, OpKind};
use crate::model::User;
use crate::state::account::{AccountState, OpState};
use crate::ui::messages::{Btn, Pal};
use crate::ui::{UiCx, bold, cosmetics, images, profile::cover_uv, rgb, sidebar::user_avatar_at};
use egui::{Align, CornerRadius, Frame, Image, Layout, Margin, Rect, RichText, Sense, Stroke, TextEdit, Vec2, pos2, vec2};
use std::path::PathBuf;
use zeroize::Zeroizing;

/// Same caps as the API (ACCOUNT_SERVER_DESIGN "Caps worth mirroring").
const NAME_MAX: usize = 32;
const PASSWORD_MAX: usize = 128;
const PRONOUNS_MAX: usize = 40;
const BIO_MAX: usize = 190;
const IMAGE_TYPES: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];
const CARD_W: f32 = 300.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    DisplayName,
    Username,
}

/// My Account forms. Dropped (typed secrets wiped) when the layer opens or closes.
#[derive(Default)]
pub(super) struct AccountForm {
    /// The inline editor (one field at a time).
    editor: Editor,
    /// Email / phone shown unmasked (Reveal; this opening only).
    reveal: (bool, bool),
    /// Password and Authentication dialogs and the 2FA prompt.
    sec: security::Form,
}

#[derive(Default)]
struct Editor {
    edit: Option<Field>,
    draft: String,
    /// Current password for a username change; pre-sized so typing never leaves unwiped copies.
    password: Zeroizing<String>,
}

/// Profiles page: what was loaded / last saved, the edits, and what Save sent.
pub(super) struct ProfileForm {
    base: ProfileDraft,
    draft: ProfileDraft,
    sent: ProfileDraft,
    /// The `FetchProfile` answer was applied (or failed: `error`).
    loaded: bool,
    error: Option<String>,
}

#[derive(Clone, Default, PartialEq)]
struct ProfileDraft {
    pronouns: String,
    bio: String,
    /// Banner colour 0xRRGGBB; None = the avatar-derived default.
    accent: Option<u32>,
}

impl ProfileForm {
    /// The layer closed: unsaved edits go.
    pub(super) fn discard(&mut self) {
        self.draft = self.base.clone();
    }

    /// Saved fields become the new base (their Done entries are consumed); failures stay
    /// visible until the next Save.
    pub(super) fn apply_saved(&mut self, account: &mut AccountState) {
        for kind in [OpKind::UpdatePronouns, OpKind::UpdateBio, OpKind::UpdateAccentColor] {
            if let Some(OpState::Done(_)) = account.ops.get(&my_op(kind)) {
                match kind {
                    OpKind::UpdatePronouns => self.base.pronouns = self.sent.pronouns.clone(),
                    OpKind::UpdateBio => self.base.bio = self.sent.bio.clone(),
                    _ => self.base.accent = self.sent.accent,
                }
                account.ops.remove(&my_op(kind));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// My Account
// ---------------------------------------------------------------------------

pub(super) fn my_account(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    let Some(me) = cx.state.me.clone() else { return };
    let pal = Pal::new(cx.theme);
    // Esc closes an open editor first, not the whole layer (a dialog or popup above goes first).
    if cx.view.user_settings_ui.account.editor.edit.is_some()
        && !security::dialog_open(cx)
        && !cx.view.dialog_open()
        && !egui::Popup::is_any_open(ui.ctx())
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        close_editor(cx);
    }
    // One E1 container (docs/DESIGN.md §12): banner, avatar, name and the one primary, then the
    // account rows divided by hairlines. No panel inside the panel.
    let has_banner = me.banner.is_some() || me.accent_color.is_some();
    Frame::new().fill(pal.surface).corner_radius(pal.card_radius()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let w = ui.available_width();
        // Banner strip (or just room for the avatar), avatar overlapping its lower edge, name,
        // Edit profile.
        let top = CornerRadius { nw: pal.card_radius(), ne: pal.card_radius(), sw: 0, se: 0 };
        let (banner, _) = ui.allocate_exact_size(vec2(w, if has_banner { 100.0 } else { 48.0 }), Sense::hover());
        paint_banner(ui, cx, &me, banner, top);
        let avatar = Rect::from_min_size(pos2(banner.left() + 16.0, banner.bottom() - 40.0), Vec2::splat(80.0));
        ui.painter().circle_filled(avatar.center(), 46.0, pal.surface);
        user_avatar_at(ui, cx.images, &cdn::avatar_url(&me, 160, true), avatar, true, 1.0, (me.id, me.display_name()));
        let (head, _) = ui.allocate_exact_size(vec2(w, 56.0), Sense::hover());
        let name_left = avatar.right() + 16.0;
        let bw = 120.0;
        let g = crate::ui::one_line(ui, me.display_name(), crate::ui::semibold(pal.title()), pal.text, (head.right() - 32.0 - bw - name_left).max(40.0));
        ui.painter().galley(pos2(name_left, head.top() + 8.0), g, pal.text);
        let edit_rect = Rect::from_min_size(pos2(head.right() - 16.0 - bw, head.top() + 4.0), vec2(bw, 32.0));
        let mut edit = ui.new_child(egui::UiBuilder::new().max_rect(edit_rect).layout(Layout::right_to_left(Align::Center)));
        if primary_button(&mut edit, cx.theme, "Edit profile", true).clicked() {
            cx.view.settings_page = Page::Profiles;
        }

        Frame::new().inner_margin(Margin { left: 16, right: 16, top: 0, bottom: 16 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            let name = me.global_name.clone().filter(|n| !n.is_empty());
            field(ui, cx, &pal, Field::DisplayName, "Display name", name.as_deref().unwrap_or("Not set"), &me);
            rule(ui, &pal);
            let username = if cx.settings.streamer.hides_personal() { "Hidden by Streamer Mode" } else { me.username.as_str() };
            field(ui, cx, &pal, Field::Username, "Username", username, &me);
            rule(ui, &pal);
            let hide = cx.settings.streamer.hides_personal();
            // The cache never holds them: until READY they are unknown, not missing.
            let online = cx.state.conn == crate::backend::ConnState::Connected && !crate::config::offline_mode();
            let none = |missing| if hide { "Hidden by Streamer Mode" } else if online { missing } else { "Shown once you're connected." };
            let reveal = &mut cx.view.user_settings_ui.account.reveal;
            // Discord's Edit / Add, only once it is known whether there is one.
            let button = |v: Option<&str>| if v.is_some() { Some("Edit") } else { online.then_some("Add") };
            let email = me.email.as_deref().map(String::as_str);
            secret_row(ui, &pal, "Email", email.filter(|_| !hide), mask_email, &mut reveal.0, none("You haven't added an email yet."), button(email));
            rule(ui, &pal);
            let phone = me.phone.as_deref().map(String::as_str);
            secret_row(ui, &pal, "Phone number", phone.filter(|_| !hide), mask_phone, &mut reveal.1, none("You haven't added a phone number yet."), button(phone));
        });
    });
    security::show(ui, cx, &pal, &me);
}

/// The hairline between two rows of one group (`rule`), 12 px of air on each side.
fn rule(ui: &mut egui::Ui, pal: &Pal) {
    ui.add_space(12.0 - ui.spacing().item_spacing.y);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, pal.tk.rule));
    ui.add_space(12.0 - ui.spacing().item_spacing.y);
}

/// Email / phone in the account card: masked, Reveal shows it here (nothing is sent); changing
/// it needs Discord's verification, so Edit opens the browser.
#[allow(clippy::too_many_arguments, reason = "one row's texts and state")]
fn secret_row(ui: &mut egui::Ui, pal: &Pal, label: &str, value: Option<&str>, mask: fn(&str) -> String, reveal: &mut bool, none: &str, button: Option<&str>) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            field_label(ui, pal, label);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let Some(v) = value else {
                    ui.label(RichText::new(none).color(pal.muted));
                    return;
                };
                ui.label(RichText::new(if *reveal { v.to_owned() } else { mask(v) }).color(pal.text));
                let toggle = egui::Button::new(RichText::new(if *reveal { "Hide" } else { "Reveal" }).color(pal.link)).frame(false);
                if ui.add(toggle).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    *reveal = !*reveal;
                }
            });
        });
        let Some(button) = button else { return };
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let edit = crate::ui::messages::button(ui, pal, Btn::Secondary, button, true);
            if edit.on_hover_text("Opens discord.com: changing it needs a verification code").clicked() {
                open_account_page(ui);
            }
        });
    });
}

/// Discord's account settings in the browser (a link the user clicks; Rustcord sends nothing).
fn open_account_page(ui: &egui::Ui) {
    ui.ctx().open_url(egui::OpenUrl::new_tab("https://discord.com/settings/account"));
}

/// `name@example.com` -> `****@example.com`, like Discord's card.
fn mask_email(e: &str) -> String {
    match e.split_once('@') {
        Some((local, domain)) => format!("{}@{domain}", "*".repeat(local.chars().count())),
        None => "*".repeat(e.chars().count()),
    }
}

/// `+491701234567` -> `*********4567`.
fn mask_phone(p: &str) -> String {
    let n = p.chars().count();
    p.chars().enumerate().map(|(i, c)| if i + 4 < n { '*' } else { c }).collect()
}

/// One row of the account panel: label and value with Edit, or its inline editor.
fn field(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, which: Field, label: &str, value: &str, me: &User) {
    let editing = cx.view.user_settings_ui.account.editor.edit == Some(which);
    if !editing {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                field_label(ui, pal, label);
                ui.label(RichText::new(value).color(pal.text));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if crate::ui::messages::button(ui, pal, Btn::Secondary, "Edit", true).clicked() {
                    open_editor(cx, which, me);
                }
            });
        });
        return;
    }
    let kind = op_kind(which);
    let state = op_state(cx, kind);
    if matches!(state, Some(OpState::Done(_))) {
        close_editor(cx);
        return;
    }
    let pending = state == Some(OpState::Pending);
    field_label(ui, pal, label);
    let f = &mut cx.view.user_settings_ui.account.editor;
    let mut enter = false;
    ui.add_enabled_ui(!pending, |ui| {
        let hint = if which == Field::DisplayName { me.username.as_str() } else { "new_username" };
        let r = ui.add(TextEdit::singleline(&mut f.draft).char_limit(NAME_MAX).hint_text(hint).desired_width(f32::INFINITY));
        enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if which == Field::Username {
            field_label(ui, pal, "Current password");
            let id = egui::Id::unique("rc_account_password");
            let r = ui.add(TextEdit::singleline(&mut *f.password).id(id).password(true).char_limit(PASSWORD_MAX).desired_width(f32::INFINITY));
            // egui's undo history would keep plain copies of the password.
            crate::ui::login::forget_history(ui.ctx(), id);
            enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        }
    });
    let draft = f.draft.trim();
    let ok = match which {
        Field::DisplayName => draft != me.global_name.as_deref().unwrap_or_default(),
        Field::Username => (2..=NAME_MAX).contains(&draft.chars().count()) && draft != me.username && !f.password.is_empty(),
    };
    op_line(ui, pal, state.as_ref(), "Saving…");
    let mut save = enter && ok && !pending;
    ui.horizontal(|ui| {
        save |= primary_button(ui, cx.theme, if pending { "Saving…" } else { "Save" }, ok && !pending).clicked();
        if crate::ui::messages::button(ui, pal, Btn::Ghost, "Cancel", true).clicked() {
            close_editor(cx);
        }
    });
    if !save || !cx.state.account.begin(my_op(kind)) {
        return;
    }
    let f = &mut cx.view.user_settings_ui.account.editor;
    let text = f.draft.trim().to_owned();
    let cmd = match which {
        Field::DisplayName => AccountCommand::UpdateGlobalName { name: Some(text).filter(|t| !t.is_empty()) },
        // Move the typed bytes into the Secret (no copy) and keep a fresh pre-sized buffer.
        Field::Username => AccountCommand::UpdateUsername {
            username: text,
            password: Secret::new(std::mem::replace(&mut *f.password, String::with_capacity(4 * PASSWORD_MAX))),
        },
    };
    cx.cmd.send(Command::Account(cmd));
}

fn op_kind(which: Field) -> OpKind {
    match which {
        Field::DisplayName => OpKind::UpdateGlobalName,
        Field::Username => OpKind::UpdateUsername,
    }
}

fn open_editor(cx: &mut UiCx<'_>, which: Field, me: &User) {
    close_editor(cx);
    finish_op(cx, op_kind(which));
    let f = &mut cx.view.user_settings_ui.account.editor;
    f.edit = Some(which);
    f.draft = match which {
        Field::DisplayName => me.global_name.clone().unwrap_or_default(),
        Field::Username => me.username.clone(),
    };
    f.password = Zeroizing::new(String::with_capacity(4 * PASSWORD_MAX));
}

fn close_editor(cx: &mut UiCx<'_>) {
    if let Some(which) = cx.view.user_settings_ui.account.editor.edit.take() {
        finish_op(cx, op_kind(which));
    }
    // Dropping the old buffer wipes the typed password.
    cx.view.user_settings_ui.account.editor = Editor::default();
}

/// Avatar or banner row: Change (file picker, uploaded right away) and Remove (asks first).
fn image_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, kind: OpKind, has_one: bool, allowed: Result<(), &str>) {
    let avatar = kind == OpKind::UpdateAvatar;
    let what = if avatar { "Avatar" } else { "Banner" };
    let state = op_state(cx, kind);
    let pending = state == Some(OpState::Pending);
    let mut pick = false;
    let mut remove = false;
    ui.horizontal(|ui| {
        crate::ui::button_row(ui);
        field_label(ui, pal, if avatar { "Avatar" } else { "Profile banner" });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if has_one {
                let r = ui.add_enabled_ui(!pending, |ui| crate::ui::messages::button(ui, pal, Btn::Ghost, "Remove", true)).inner;
                remove = r.on_hover_text(format!("Remove {}", what.to_lowercase())).clicked();
            }
            let change = crate::ui::messages::button(ui, pal, Btn::Secondary, "Change", !pending && allowed.is_ok());
            pick = match allowed {
                Ok(()) => change.on_hover_text(if avatar { "PNG, JPG, GIF or WebP up to 10 MB, cropped to a square" } else { "Cropped to 1024×409" }).clicked(),
                Err(why) => change.on_disabled_hover_text(why).clicked(),
            };
        });
    });
    op_line(ui, pal, state.as_ref(), "Uploading…");
    if pick && let Some(path) = rfd::FileDialog::new().add_filter("Images", &IMAGE_TYPES).pick_file() {
        send_image(cx, kind, Some(path));
    }
    if remove {
        let then = Command::Account(image_command(kind, None));
        cx.view.confirm(format!("Remove {}?", what.to_lowercase()), format!("Your {} goes back to the default for everyone.", what.to_lowercase()), "Remove", then);
    }
}

fn image_command(kind: OpKind, path: Option<PathBuf>) -> AccountCommand {
    if kind == OpKind::UpdateAvatar { AccountCommand::UpdateAvatar { path } } else { AccountCommand::UpdateBanner { path } }
}

fn send_image(cx: &mut UiCx<'_>, kind: OpKind, path: Option<PathBuf>) {
    if cx.state.account.begin(my_op(kind)) {
        cx.cmd.send(Command::Account(image_command(kind, path)));
    }
}

/// Banner: my banner image (cover-cropped), else my banner colour, else nothing (the
/// container shows through: no flat grey block, docs/DESIGN.md §12).
fn paint_banner(ui: &mut egui::Ui, cx: &mut UiCx<'_>, me: &User, rect: Rect, corners: CornerRadius) {
    if let Some(fill) = me.accent_color.map(rgb) {
        ui.painter().rect_filled(rect, corners, fill);
    }
    if let Some(hash) = &me.banner {
        let animated = hash.starts_with("a_") && ui.rect_contains_pointer(rect);
        if let Some(tex) = cx.images.get(ui.ctx(), &cdn::user_banner_url(me.id, hash, 600), 600, animated) {
            Image::from_texture(tex).uv(cover_uv(tex.size, rect.size())).corner_radius(corners).paint_at(ui, rect);
        }
    }
}

// ---------------------------------------------------------------------------
// Profiles
// ---------------------------------------------------------------------------

pub(super) fn profiles(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    let Some(me) = cx.state.me.clone() else { return };
    if !profile_tabs(ui, cx) {
        return server_profile(ui, cx, &me);
    }
    let pal = Pal::new(cx.theme);
    prefill(cx, &me);
    image_row(ui, cx, &pal, OpKind::UpdateAvatar, me.avatar.is_some(), Ok(()));
    // Uploading a banner needs Nitro (§3.2); removing one never does.
    let banner_ok = if me.premium_type != 0 { Ok(()) } else { Err("Profile banners need Nitro") };
    image_row(ui, cx, &pal, OpKind::UpdateBanner, me.banner.is_some(), banner_ok);
    ui.add_space(12.0);
    // My account's own profile theme for the preview (never my local one).
    let theme = crate::ui::profile_modal::loaded(cx, me.id, None).and_then(|p| p.theme_colors());
    let Some(form) = cx.view.user_settings_ui.profile.as_mut() else { return };
    form.apply_saved(&mut cx.state.account);
    let states = [OpKind::UpdatePronouns, OpKind::UpdateBio, OpKind::UpdateAccentColor].map(|k| cx.state.account.ops.get(&my_op(k)).cloned());
    let pending = states.contains(&Some(OpState::Pending));

    let wide = ui.available_width() >= CARD_W + 280.0;
    let mut save = false;
    let mut form_ui = |ui: &mut egui::Ui, form: &mut ProfileForm| {
        if !form.loaded {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0).color(pal.muted));
                ui.label(RichText::new("Loading your profile…").size(pal.meta()).color(pal.muted));
            });
        }
        if let Some(e) = &form.error {
            ui.label(RichText::new(format!("Your profile didn't load ({e}). Connect, then open this page again.")).size(pal.meta()).color(pal.danger));
        }
        // A failed load leaves the fields blank: editing them would overwrite unseen values.
        let ready = form.loaded && form.error.is_none();
        ui.add_enabled_ui(ready && !pending, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            field_label(ui, &pal, "Pronouns");
            ui.add(TextEdit::singleline(&mut form.draft.pronouns).char_limit(PRONOUNS_MAX).hint_text("Add your pronouns").desired_width(f32::INFINITY));
            ui.add_space(10.0);
            field_label(ui, &pal, "About me");
            ui.add(TextEdit::multiline(&mut form.draft.bio).char_limit(BIO_MAX).desired_rows(4).desired_width(f32::INFINITY));
            let n = form.draft.bio.chars().count();
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| ui.label(RichText::new(format!("{}", BIO_MAX - n)).size(pal.meta()).color(pal.muted)));
            field_label(ui, &pal, "Banner colour");
            ui.horizontal(|ui| {
                let fallback = images::initial_colors(me.id.0, pal.dark).0;
                let current = form.draft.accent.map(rgb).unwrap_or(fallback);
                let mut c = [current.r(), current.g(), current.b()];
                if ui.color_edit_button_srgb(&mut c).changed() {
                    form.draft.accent = Some(u32::from(c[0]) << 16 | u32::from(c[1]) << 8 | u32::from(c[2]));
                }
                ui.label(RichText::new(format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])).color(pal.muted));
                if form.draft.accent.is_some() && crate::ui::messages::button(ui, &pal, Btn::Ghost, "Use default", true).on_hover_text("Use the colour from your avatar").clicked() {
                    form.draft.accent = None;
                }
            });
        });
        for s in &states {
            op_line(ui, &pal, s.as_ref(), "Saving…");
        }
        ui.add_space(12.0);
        let dirty = form.draft != form.base;
        ui.horizontal(|ui| {
            save = primary_button(ui, cx.theme, if pending { "Saving…" } else { "Save changes" }, dirty && !pending && ready).clicked();
            if crate::ui::messages::button(ui, &pal, Btn::Ghost, "Reset", dirty && !pending).clicked() {
                form.discard();
            }
        });
    };
    if wide {
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                // Room for the 24 px gap, the card and the row's two item gaps: no overflow.
                ui.set_width(ui.available_width() - CARD_W - 24.0 - 2.0 * ui.spacing().item_spacing.x);
                form_ui(ui, form);
            });
            ui.add_space(24.0);
            ui.vertical(|ui| {
                field_label(ui, &pal, "Preview");
                preview(ui, cx.images, &pal, &me, &form.draft, theme);
            });
        });
    } else {
        form_ui(ui, form);
        ui.add_space(16.0);
        field_label(ui, &pal, "Preview");
        preview(ui, cx.images, &pal, &me, &form.draft, theme);
    }
    if save {
        let form = cx.view.user_settings_ui.profile.as_mut().expect("shown above");
        form.sent = form.draft.clone();
        let (d, b) = (&form.draft, &form.base);
        let mut cmds = Vec::new();
        if d.pronouns.trim() != b.pronouns.trim() {
            cmds.push(AccountCommand::UpdatePronouns { pronouns: d.pronouns.trim().to_owned() });
        }
        if d.bio.trim() != b.bio.trim() {
            cmds.push(AccountCommand::UpdateBio { bio: d.bio.trim().to_owned() });
        }
        if d.accent != b.accent {
            cmds.push(AccountCommand::UpdateAccentColor { color: d.accent });
        }
        // Whitespace-only edits: nothing to send, the draft just matches again.
        if cmds.is_empty() {
            form.base = form.draft.clone();
        }
        for cmd in cmds {
            if cx.state.account.begin(cmd.op()) {
                cx.cmd.send(Command::Account(cmd));
            }
        }
    }
}

/// User Profile | Server Profiles (Discord's tabs). True while User Profile is shown.
fn profile_tabs(ui: &mut egui::Ui, cx: &mut UiCx<'_>) -> bool {
    let first = cx.state.selected_guild.or_else(|| cx.state.guilds.first().map(|g| g.id));
    let tab = &mut cx.view.user_settings_ui.server_tab;
    let (was, mut server) = (tab.is_some(), tab.is_some());
    let w = ui.available_width().min(320.0);
    ui.allocate_ui(vec2(w, 36.0), |ui| {
        ui.add_enabled_ui(first.is_some(), |ui| crate::ui::login::segmented(ui, cx.theme, &mut server, &[(false, "User profile"), (true, "Server profiles")]));
    });
    if server != was {
        *tab = if server { first.map(|g| (g, None)) } else { None };
    }
    ui.add_space(8.0);
    tab.is_none()
}

/// Server Profiles: the server picked and my nickname there (`SetMyNick`, one request on Save).
fn server_profile(ui: &mut egui::Ui, cx: &mut UiCx<'_>, me: &User) {
    let pal = Pal::new(cx.theme);
    let Some((guild_id, _)) = cx.view.user_settings_ui.server_tab else { return };
    let nick = cx.state.find_member(guild_id, me.id).and_then(|m| m.nick.clone()).unwrap_or_default();
    crate::ui::hint(ui, cx.theme, "Show who you are with a different nickname in each of your servers.");
    ui.add_space(8.0);
    field_label(ui, &pal, "Choose a server");
    let name = |id| cx.state.guilds.iter().find(|g| g.id == id).map_or("", |g| g.name.as_str());
    let mut pick = guild_id;
    egui::ComboBox::from_id_salt("rc_server_profile").width(260.0).selected_text(name(guild_id)).icon(super::combo_icon).show_ui(ui, |ui| {
        for g in &cx.state.guilds {
            ui.selectable_value(&mut pick, g.id, &g.name);
        }
    });
    if pick != guild_id {
        cx.view.user_settings_ui.server_tab = Some((pick, None));
        return;
    }
    ui.add_space(10.0);
    field_label(ui, &pal, "Server nickname");
    let probe = AccountCommand::SetMyNick { guild_id, nick: None };
    let (allowed, op) = (cx.state.account_check(&probe), probe.op());
    let state = cx.state.account.ops.get(&op).cloned();
    let pending = state == Some(OpState::Pending);
    let Some((_, draft)) = &mut cx.view.user_settings_ui.server_tab else { return };
    let draft = draft.get_or_insert_with(|| nick.clone());
    ui.add_enabled(allowed.is_ok() && !pending, TextEdit::singleline(draft).char_limit(NAME_MAX).hint_text(me.display_name()).desired_width(320.0));
    if let Err(why) = allowed {
        crate::ui::hint(ui, cx.theme, why);
    }
    op_line(ui, &pal, state.as_ref(), "Saving…");
    ui.add_space(8.0);
    let text = draft.trim().to_owned();
    let dirty = text != nick;
    let mut save = false;
    ui.horizontal(|ui| {
        save = primary_button(ui, cx.theme, if pending { "Saving…" } else { "Save changes" }, dirty && allowed.is_ok() && !pending).clicked();
        if crate::ui::messages::button(ui, &pal, Btn::Ghost, "Reset", dirty && !pending).clicked() {
            *draft = nick.clone();
        }
    });
    let cmd = AccountCommand::SetMyNick { guild_id, nick: Some(text).filter(|t| !t.is_empty()) };
    if save && cx.state.account.begin(cmd.op()) {
        cx.cmd.send(Command::Account(cmd));
    }
}

/// First showing of Profiles this session: the one `FetchProfile` for me (User has no bio /
/// pronouns), then the drafts from its answer.
fn prefill(cx: &mut UiCx<'_>, me: &User) {
    if cx.view.user_settings_ui.profile.is_none() {
        let base = ProfileDraft { accent: me.accent_color, ..Default::default() };
        cx.view.user_settings_ui.profile = Some(ProfileForm { draft: base.clone(), sent: base.clone(), base, loaded: false, error: None });
        crate::ui::profile_modal::fetch(cx, me.id, None);
    }
    let Some(form) = cx.view.user_settings_ui.profile.as_mut().filter(|f| !f.loaded) else { return };
    let Some(entry) = cx.state.profile(me.id, None).filter(|e| !e.loading) else { return };
    match &entry.result {
        Some(Ok(p)) => {
            let f = &p.user_profile;
            let base = ProfileDraft {
                pronouns: f.pronouns.clone().unwrap_or_default(),
                bio: f.bio.clone().unwrap_or_default(),
                accent: f.accent_color.or(me.accent_color),
            };
            (form.draft, form.base) = (base.clone(), base);
        }
        Some(Err(e)) => form.error = Some(e.clone()),
        None => return,
    }
    form.loaded = true;
}

/// What others see: banner, avatar with its decoration, styled name, pronouns, About Me, in my
/// profile `theme`; my account's own cosmetics through the card's renderers, never local picks.
fn preview(ui: &mut egui::Ui, images_cache: &mut images::ImageCache, pal: &Pal, me: &User, d: &ProfileDraft, theme: Option<(u32, u32)>) {
    let pal = &theme.map_or(*pal, |t| cosmetics::theme_pal(pal, t));
    let real = crate::overrides::real_for(me);
    let stroke = if theme.is_some() { Stroke::NONE } else { Stroke::new(1.0, pal.tk.rule) };
    let r = pal.float_radius();
    Frame::new().fill(pal.surface).corner_radius(r).stroke(stroke).show(ui, |ui| {
        if theme.is_some() {
            cosmetics::theme_visuals(ui, pal);
        }
        let theme_slot = ui.painter().add(egui::Shape::Noop);
        ui.set_width(CARD_W);
        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
        let top = CornerRadius { nw: r, ne: r, sw: 0, se: 0 };
        let h = if me.banner.is_some() { 106.0 } else { 60.0 };
        let (rect, _) = ui.allocate_exact_size(vec2(CARD_W, h), Sense::hover());
        // An uploaded banner fades into the card like on the profile card (profile::paint_banner).
        match me.banner.as_ref().and_then(|hash| images_cache.get(ui.ctx(), &cdn::user_banner_url(me.id, hash, 600), 600, false)) {
            Some(tex) => crate::ui::profile::paint_banner(ui.painter(), rect, top, tex, theme.map(|_| cosmetics::theme_veil(pal.dark))),
            None => {
                let fill = theme.map(|(primary, _)| primary).or(d.accent).map(rgb);
                // No colour: the card itself, like the profile popout (profile::banner).
                ui.painter().rect_filled(rect, top, fill.unwrap_or(pal.surface));
            }
        }
        let center = pos2(rect.left() + 56.0, rect.bottom());
        ui.painter().circle_filled(center, 45.0, theme.map_or(pal.surface, |(primary, _)| cosmetics::theme_ring(primary, pal.dark)));
        let avatar = Rect::from_center_size(center, Vec2::splat(80.0));
        let hovered = ui.rect_contains_pointer(avatar);
        user_avatar_at(ui, images_cache, &cdn::avatar_url(me, 160, true), avatar, true, 1.0, (me.id, me.display_name()));
        images::decoration_at(ui, images_cache, avatar, real.decoration.as_ref(), hovered, 1.0);
        ui.add_space(46.0);
        Frame::new().inner_margin(Margin { left: 16, right: 16, top: 0, bottom: 14 }).show(ui, |ui| {
            ui.set_width(CARD_W - 32.0);
            let style = real.name_style.as_ref();
            let name = cosmetics::name_galley(ui, me.display_name(), bold(pal.title()), style, pal.text, CARD_W - 32.0);
            let (r, _) = ui.allocate_exact_size(name.size(), Sense::hover());
            cosmetics::paint_name(ui.painter(), r.min, name, style, pal.text, pal.dark);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(RichText::new(&me.username).color(pal.muted));
                let pronouns = d.pronouns.trim();
                if !pronouns.is_empty() {
                    ui.label(RichText::new(pronouns).color(pal.muted));
                }
            });
            let bio = d.bio.trim();
            if !bio.is_empty() {
                ui.add_space(12.0);
                ui.label(RichText::new("About me").font(crate::ui::semibold(pal.label())).color(pal.muted));
                ui.add(egui::Label::new(RichText::new(bio).color(pal.text)).wrap());
            }
        });
        if let Some(c) = theme {
            let card = ui.min_rect();
            ui.painter().set(theme_slot, cosmetics::theme_shapes(ui.ctx(), "profiles-preview", card, r, c, rect.bottom(), pal.dark));
            ui.painter().add(cosmetics::theme_outline(card, r, c));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{mask_email, mask_phone};

    #[test]
    fn masks_keep_only_the_domain_and_last_digits() {
        assert_eq!(mask_email("ferris@rust-lang.org"), "******@rust-lang.org");
        assert_eq!(mask_email("john.doe@gmail.com"), "********@gmail.com");
        assert_eq!(mask_email("nodomain"), "********");
        assert_eq!(mask_phone("+491701234567"), "*********4567");
        assert_eq!(mask_phone("123"), "123");
    }
}
