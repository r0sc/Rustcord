//! Login screen: a centred panel (app icon, title, a QR code / Email / Token switcher and the
//! active form) on the flat frame colour, above a two-line footer (unofficial-client note, Terms
//! of Service). docs/DESIGN.md §12.
//! QR code (scan with the phone app) is the default; email + password may need a 2FA step;
//! a pasted token is the fallback. "Stay signed in" stores the resulting token in the OS
//! credential store. Sends `Command::{StartQrLogin, LoginPassword, SubmitMfa, Login, ...}`.
//! Passwords and 2FA codes live in `Zeroizing` buffers and leave as `auth::Secret`.
//! OWNERSHIP: ui-shell implementer.

use super::UiCx;
use crate::auth::{Secret, Token};
use crate::backend::{Command, CommandSender, LoginStep, MfaMethod};
use crate::captcha::CaptchaReply;
use crate::theme::Theme;
use egui::emath::GuiRounding;
use egui::{Align, Align2, Color32, FontId, Frame, Key, Layout, Margin, Rect, RichText, Sense, Vec2, vec2};
use qrcodegen::{QrCode, QrCodeEcc};
use zeroize::{Zeroize, Zeroizing};

const CARD_W: f32 = 440.0;
/// Below this panel height the card gets less padding, no subtitle and a smaller icon and QR code.
const COMPACT_BELOW_H: f32 = 600.0;
/// Height of text fields and the primary button.
const FIELD_H: f32 = 40.0;
const SEGMENT_H: f32 = 36.0;
/// Side of the QR square in points.
const QR_SIDE: f32 = 184.0;
const QR_SIDE_COMPACT: f32 = 144.0;
/// Quiet zone around the QR code in modules (what the QR standard asks for).
const QR_QUIET: i32 = 4;
const PASSWORD_LIMIT: usize = 128;
/// hCaptcha's checkbox widget is 303 x 78 CSS pixels; the captcha webview's CSS pixels are egui points.
const CAPTCHA_H: f32 = 78.0;
const CODE_LIMIT: usize = 16;
const OFFLINE_HINT: &str = "Offline mode (RUSTCORD_OFFLINE) never connects.";
const SCAN_HINT: &str = "Open Settings in the Discord app, then Scan QR Code.";
const TERMS_URL: &str = "https://discord.com/terms";
const TOKEN_HELP: &str = "Sign in to Discord in a web browser, open the developer tools (F12), go to Network, \
    select any request to discord.com/api and copy its \"Authorization\" request header.\n\n\
    Your token is the key to your whole account: never share it, never paste it anywhere else.";

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum LoginTab {
    #[default]
    Qr,
    Email,
    Token,
}

#[derive(Default)]
enum QrState {
    /// Nothing running; the QR tab starts a login when shown.
    #[default]
    Idle,
    /// `StartQrLogin` sent, no code yet.
    Waiting,
    Code(QrCode),
    /// Scanned; waiting for the confirmation on the phone (then the token exchange).
    Scanned { username: String, avatar_url: Option<String> },
    Expired,
    Failed(String),
}

/// 2FA step of the email login.
struct Mfa {
    methods: Vec<MfaMethod>,
    method: MfaMethod,
    code: Zeroizing<String>,
    /// Masked phone number once an SMS was sent.
    sms_phone: Option<String>,
}

/// Email / QR login state (`ViewState::login_form`). Secret buffers are wiped on drop.
pub struct LoginForm {
    tab: LoginTab,
    qr: QrState,
    email: String,
    password: Zeroizing<String>,
    show_password: bool,
    mfa: Option<Mfa>,
}

impl Default for LoginForm {
    fn default() -> Self {
        Self {
            tab: LoginTab::default(),
            qr: QrState::Idle,
            email: String::new(),
            password: secret_buf(PASSWORD_LIMIT),
            show_password: false,
            mfa: None,
        }
    }
}

/// Pre-sized for `limit` chars of up to 4 bytes, so typing never reallocates and leaves an
/// unwiped copy behind in freed memory.
fn secret_buf(limit: usize) -> Zeroizing<String> {
    Zeroizing::new(String::with_capacity(4 * limit))
}

fn password_id() -> egui::Id {
    egui::Id::unique("login_password")
}

fn code_id() -> egui::Id {
    egui::Id::unique("login_mfa_code")
}

fn token_id() -> egui::Id {
    egui::Id::unique("login_token")
}

fn icon_id() -> egui::Id {
    egui::Id::unique("login_icon")
}

/// egui's undo history keeps plain copies of what was typed; drop them once the secret is sent.
pub(super) fn forget_history(ctx: &egui::Context, id: egui::Id) {
    if let Some(mut state) = egui::TextEdit::load_state(ctx, id) {
        state.clear_undoer();
        egui::TextEdit::store_state(ctx, id, state);
    }
}

/// Also needed when unsent buffers are discarded (tab switch, Back, login done).
fn forget_all_history(ctx: &egui::Context) {
    for id in [password_id(), code_id(), token_id()] {
        forget_history(ctx, id);
    }
}

pub fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    // One flat frame-coloured page under the panel and the footer alike: no glow, no gradient.
    let page = ui.available_rect_before_wrap();
    ui.painter().rect_filled(page, 0.0, cx.theme.colors.guild_bar.to_color32());
    ui.style_mut().interaction.selectable_labels = false;
    egui::Panel::bottom("login_footer")
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::new().inner_margin(Margin::symmetric(16, 12)))
        .show(ui, |ui| footer(ui, cx.theme));
    egui::CentralPanel::default().frame(Frame::new()).show(ui, |ui| {
        let avail_h = ui.available_height();
        let compact = avail_h < COMPACT_BELOW_H;
        // Centre on the tallest card seen at this panel height, so a tab switch only moves the
        // card's bottom edge, never the header and switcher under the cursor; a resize re-centres.
        // Scroll when the window is too small.
        let height_id = ui.make_persistent_id("login_card_height");
        let seen = ui.ctx().data(|d| d.get_temp::<(f32, f32)>(height_id)).filter(|&(at, _)| at == avail_h);
        let card_h = seen.map_or(avail_h, |(_, h)| h);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.add_space(((avail_h - card_h) / 2.0).max(8.0));
            ui.vertical_centered(|ui| {
                // Short windows: 16 px padding, so the panel fits above the two-line footer.
                let pad: i8 = if compact { 16 } else { 32 };
                let w = CARD_W.min(ui.available_width() - 32.0);
                // A new error shakes the card: 3 swings of 6 px, decaying over LARGE (principle 9).
                let t = super::motion::anim(ui.ctx(), ui.make_persistent_id("rc_login_shake"), cx.view.login_error.is_some(), super::motion::LARGE, 0.0);
                let dx = 6.0 * (t * 6.0 * std::f32::consts::PI).sin() * (1.0 - t);
                let card = ui
                    .with_visual_transform(egui::emath::TSTransform::from_translation(vec2(dx, 0.0)), |ui| {
                        card_frame(ui.ctx(), cx.theme, pad).show(ui, |ui| {
                            ui.set_width(w - 2.0 * f32::from(pad));
                            ui.vertical(|ui| card(ui, cx, compact));
                        })
                    })
                    .inner;
                let tallest = card.response.rect.height().max(seen.map_or(0.0, |(_, h)| h));
                ui.ctx().data_mut(|d| d.insert_temp(height_id, (avail_h, tallest)));
                if tallest != card_h {
                    ui.ctx().request_discard("login card height"); // no frame at the old position
                }
            });
            ui.add_space(8.0);
        });
    });
}

/// The panel (E3, §6): `raised`, a `rule` edge (high contrast: 1.5 px muted, no shadow), the
/// modal shadow and radius (2r).
fn card_frame(ctx: &egui::Context, theme: &Theme, pad: i8) -> Frame {
    let pal = super::messages::Pal::new(theme);
    let hc = pal.tk.mode == crate::theme::Mode::HighContrast;
    let edge = if hc { egui::Stroke::new(1.5, pal.muted) } else { egui::Stroke::new(1.0, pal.tk.rule) };
    Frame::new()
        .fill(pal.tk.raised)
        .stroke(edge)
        .corner_radius(pal.modal_radius())
        .shadow(ctx.global_style().visuals.window_shadow)
        .inner_margin(Margin::same(pad))
}

/// Header, method switcher and the active form (or the 2FA step, which replaces the switcher).
fn card(ui: &mut egui::Ui, cx: &mut UiCx<'_>, compact: bool) {
    ui.spacing_mut().item_spacing.y = 0.0; // explicit 8/12/16/24 gaps below
    let gap = if compact { 16.0 } else { 24.0 };
    let mfa = match (cx.view.login_form.tab, &cx.view.login_form.mfa) {
        (LoginTab::Email, Some(mfa)) => Some(mfa_subtitle(mfa)),
        _ => None,
    };
    if let Some(subtitle) = mfa {
        header(ui, cx.theme, compact, "Two-factor authentication", Some(subtitle.as_str()));
        ui.add_space(gap);
        mfa_tab(ui, cx);
        return;
    }
    header(ui, cx.theme, compact, "Sign in to Rustcord", (!compact).then_some("Use your Discord account."));
    ui.add_space(gap);
    tabs(ui, cx);
    ui.add_space(gap);
    match cx.view.login_form.tab {
        LoginTab::Qr => qr_tab(ui, cx, compact),
        LoginTab::Email => email_tab(ui, cx),
        LoginTab::Token => token_tab(ui, cx),
    }
}

fn header(ui: &mut egui::Ui, theme: &Theme, compact: bool, title: &str, subtitle: Option<&str>) {
    let c = &theme.colors;
    let pal = super::messages::Pal::new(theme);
    // The display role (24/700); a row of icon and title in short windows.
    let font = super::bold(if compact { pal.title() } else { pal.display() });
    let icon = app_icon(ui.ctx()).map(|tex| egui::Image::new(&tex).fit_to_exact_size(Vec2::splat(if compact { 28.0 } else { 56.0 })));
    let title = RichText::new(title).font(font.clone()).color(c.text.to_color32());
    ui.vertical_centered(|ui| {
        if compact {
            // Icon and title share a row so the QR tab fits a 420 pt high window.
            let title_w = ui.painter().layout_no_wrap(title.text().to_owned(), font, Color32::WHITE).size().x;
            centered_row(ui, vec2(28.0 + 10.0 + title_w, 28.0), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                if let Some(icon) = icon {
                    ui.add(icon);
                }
                ui.label(title);
            });
        } else {
            if let Some(icon) = icon {
                ui.add(icon);
                ui.add_space(16.0);
            }
            ui.label(title);
        }
        if let Some(subtitle) = subtitle {
            ui.add_space(8.0);
            ui.label(RichText::new(subtitle).size(pal.ui()).color(c.text_muted.to_color32()));
        }
    });
}

/// The app icon, decoded once and kept in egui's temp memory (dropped again on login). Also
/// the update screen's logo (update.rs).
pub(super) fn app_icon(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    if let Some(tex) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(icon_id())) {
        return Some(tex);
    }
    let img = image::load_from_memory(include_bytes!("../../assets/icons/rustcord-128.png")).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let options = egui::TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear));
    let tex = ctx.load_texture("login_icon", egui::ColorImage::from_rgba_unmultiplied(size, &img), options);
    ctx.data_mut(|d| d.insert_temp(icon_id(), tex.clone()));
    Some(tex)
}

/// Two centred meta lines (§12): what Rustcord is, and the risk with the Terms of Service link.
/// No middle dots.
fn footer(ui: &mut egui::Ui, theme: &Theme) {
    let muted = theme.colors.text_muted.to_color32();
    let size = theme.font.size - 3.0;
    let head = concat!("Rustcord ", env!("CARGO_PKG_VERSION"), " is an unofficial client and is not affiliated with Discord.");
    let (risk, terms, end) = ("Third-party clients can break Discord's ", "Terms of Service", ".");
    let galley = |t: &str| ui.painter().layout_no_wrap(t.to_owned(), FontId::proportional(size), muted).size();
    let (head_w, row_h) = (galley(head).x, galley(head).y);
    let tail_w = galley(risk).x + galley(terms).x + galley(end).x;
    let text = |t: &str| RichText::new(t).size(size).color(muted);
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
        centered_row(ui, vec2(head_w, row_h), |ui| {
            ui.label(text(head));
        });
        centered_row(ui, vec2(tail_w, row_h), |ui| {
            ui.label(text(risk));
            ui.add(egui::Hyperlink::from_label_and_url(RichText::new(terms).size(size), TERMS_URL).open_in_new_tab(true));
            ui.label(text(end));
        });
    });
}

/// A left-to-right row of the given measured size: inside a centred layout, it is centred.
fn centered_row(ui: &mut egui::Ui, size: Vec2, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(size + vec2(1.0, 0.0), Layout::left_to_right(Align::Center), add);
}

fn tabs(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    let before = cx.view.login_form.tab;
    // An email/token request in flight finishes on its own tab.
    let locked = cx.view.login_busy && (before != LoginTab::Qr || cx.view.login_captcha.is_some());
    let options = [(LoginTab::Qr, "QR code"), (LoginTab::Email, "Email"), (LoginTab::Token, "Token")];
    ui.add_enabled_ui(!locked, |ui| segmented(ui, cx.theme, &mut cx.view.login_form.tab, &options));
    if cx.view.login_form.tab != before {
        if before == LoginTab::Qr {
            cancel_qr(&mut cx.view.login_form, cx.cmd);
        }
        cx.view.login_busy = false;
        cx.view.login_error = None;
        cx.view.login_show_token = false;
        cx.view.login_form.show_password = false;
        forget_all_history(ui.ctx());
    }
}

/// Full-width segmented control (docs/DESIGN.md §9): a well track, equal 32 px segments, the
/// selected one lifted (`raised` on light themes, `selected` on dark). Also the settings'
/// one-of-a-few choices (2FA method, plugin filter, profile tabs).
pub(crate) fn segmented<T: Copy + PartialEq>(ui: &mut egui::Ui, theme: &Theme, value: &mut T, options: &[(T, &str)]) {
    let pal = super::messages::Pal::new(theme);
    let tk = pal.tk;
    let radius = f32::from(pal.radius);
    let lifted = if tk.mode == crate::theme::Mode::Light { tk.raised } else { tk.selected };
    let (track, _) = ui.allocate_exact_size(vec2(ui.available_width(), SEGMENT_H), Sense::hover());
    ui.painter().rect_filled(track, radius, theme.colors.code_background.to_color32());
    let inset = 2.0;
    let seg_r = (radius - inset).max(2.0);
    let seg_w = (track.width() - 2.0 * inset) / options.len() as f32;
    // The lifted fill slides to the selected segment (§7).
    let idx = options.iter().position(|(v, _)| v == value).unwrap_or(0) as f32;
    let x = super::motion::value(ui.ctx(), ui.make_persistent_id(("rc_segment", options.first().map_or("", |o| o.1))), idx, super::motion::FAST);
    ui.painter().rect_filled(Rect::from_min_size(track.min + vec2(inset + x * seg_w, inset), vec2(seg_w, SEGMENT_H - 2.0 * inset)), seg_r, lifted);
    for (i, &(v, label)) in options.iter().enumerate() {
        let min = track.min + vec2(inset + i as f32 * seg_w, inset);
        let rect = Rect::from_min_size(min, vec2(seg_w, SEGMENT_H - 2.0 * inset));
        let resp = ui.interact(rect, ui.make_persistent_id(label), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, ui.is_enabled(), *value == v, label));
        if resp.clicked() {
            *value = v;
        }
        let selected = *value == v;
        let hovered = resp.hovered() && ui.is_enabled();
        let painter = ui.painter();
        if hovered && !selected {
            painter.rect_filled(rect, seg_r, tk.hover);
        }
        if resp.has_focus() {
            super::messages::focus_ring(ui, &pal, rect, seg_r);
        }
        if hovered && !selected {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let color = if selected || hovered { pal.text } else { pal.muted };
        let font = FontId::new(pal.ui(), crate::theme::medium_family());
        painter.text(rect.center(), Align2::CENTER_CENTER, label, font, color);
    }
}

// ---------------------------------------------------------------------------
// QR tab
// ---------------------------------------------------------------------------

fn start_qr(cx: &mut UiCx<'_>) {
    cx.cmd.send(Command::StartQrLogin { remember: cx.view.login_remember });
    cx.view.login_form.qr = QrState::Waiting;
}

fn cancel_qr(form: &mut LoginForm, cmd: &CommandSender) {
    if !matches!(form.qr, QrState::Idle) {
        cmd.send(Command::CancelQrLogin);
    }
    form.qr = QrState::Idle;
}

fn qr_tab(ui: &mut egui::Ui, cx: &mut UiCx<'_>, compact: bool) {
    let c = cx.theme.colors.clone();
    let offline = crate::config::offline_mode();
    if matches!(cx.view.login_form.qr, QrState::Idle) && !offline {
        start_qr(cx);
    }
    // Scanned but no longer busy: the token exchange failed (ActionFailed lands in login_error).
    if matches!(cx.view.login_form.qr, QrState::Scanned { .. }) && !cx.view.login_busy {
        let msg = cx.view.login_error.take().unwrap_or_else(|| "The login did not complete.".into());
        cx.view.login_form.qr = QrState::Failed(msg);
    }

    let theme = &*cx.theme;
    let radius = theme.layout.corner_radius * 1.5;
    let side = if compact { QR_SIDE_COMPACT } else { QR_SIDE };
    let muted = c.text_muted.to_color32();
    let mut new_code = false;
    ui.vertical_centered(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
        if !matches!(cx.view.login_form.qr, QrState::Code(_)) || offline {
            ui.painter().rect_filled(rect, radius, c.code_background.to_color32());
        }
        let caption = match &cx.view.login_form.qr {
            _ if offline => {
                square_message(ui, theme, rect, OFFLINE_HINT, false);
                None
            }
            QrState::Idle | QrState::Waiting => {
                let r = Rect::from_center_size(rect.center(), Vec2::splat(28.0));
                egui::Spinner::new().color(muted).paint_at(ui, r);
                Some((SCAN_HINT, muted))
            }
            QrState::Code(code) => {
                paint_qr(ui, rect, code, radius);
                // On the near-white light card the white square needs an edge (inside the quiet zone).
                if !theme.dark {
                    ui.painter().rect_stroke(rect, radius, (1.0, c.text.to_color32().gamma_multiply(0.09)), egui::StrokeKind::Inside);
                }
                Some((SCAN_HINT, muted))
            }
            QrState::Scanned { username, avatar_url } => {
                let d = if compact { 56.0 } else { 72.0 };
                let avatar = Rect::from_center_size(rect.center() - vec2(0.0, 14.0), Vec2::splat(d));
                match avatar_url {
                    Some(url) => super::avatar_at(ui, cx.images, url, avatar, false, 1.0),
                    None => {
                        // A neutral initial, never an accent disc.
                        ui.painter().circle_filled(avatar.center(), d / 2.0, theme.tokens().selected);
                        let initial = username.chars().next().unwrap_or('?').to_uppercase().to_string();
                        let font = super::semibold(d * 0.4);
                        ui.painter().text(avatar.center(), Align2::CENTER_CENTER, initial, font, c.text.to_color32());
                    }
                }
                let font = super::bold(theme.font.size);
                let name = super::one_line(ui, username.as_str(), font, c.text.to_color32(), rect.width() - 24.0);
                let pos = egui::pos2(rect.center().x - name.size().x / 2.0, avatar.bottom() + 10.0);
                ui.painter().galley(pos, name, c.text.to_color32());
                Some(("Confirm the sign-in on your phone", muted))
            }
            QrState::Expired => {
                new_code = square_message(ui, theme, rect, "This code expired or was cancelled.", true);
                None
            }
            QrState::Failed(msg) => {
                // The message can be long: it goes below the square, which stays tidy.
                new_code = square_message(ui, theme, rect, "Sign-in failed", true);
                Some((msg.as_str(), c.danger.to_color32()))
            }
        };
        if let Some((caption, color)) = caption {
            ui.add_space(12.0);
            let font = FontId::proportional(theme.font.size - 1.0);
            // A balanced two-line hint instead of an orphaned last word when one line does not fit.
            let fits = ui.painter().layout_no_wrap(caption.to_owned(), font.clone(), color).size().x <= ui.available_width();
            let caption = if caption == SCAN_HINT && !fits { "Open Settings in the Discord app,\nthen Scan QR Code." } else { caption };
            ui.label(RichText::new(caption).font(font).color(color));
        }
    });
    if new_code {
        cx.view.login_error = None;
        start_qr(cx);
    }
    captcha_slot(ui, cx);

    ui.add_space(16.0);
    ui.vertical_centered(|ui| {
        remember(ui, cx);
        error_line(ui, cx);
    });
}

/// A short centred message, and optionally a "New code" button, inside the QR square. True when
/// the button was clicked.
fn square_message(ui: &mut egui::Ui, theme: &Theme, rect: Rect, msg: &str, button: bool) -> bool {
    let c = &theme.colors;
    let color = c.text_muted.to_color32();
    let width = rect.width() - 32.0;
    let font = FontId::proportional(theme.font.size);
    let text_h = ui.painter().layout(msg.to_owned(), font.clone(), color, width).size().y;
    let button_h = if button { 12.0 + 32.0 } else { 0.0 };
    let content = Rect::from_center_size(rect.center(), vec2(width, text_h + button_h));
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(Layout::top_down(Align::Center)));
    ui.label(RichText::new(msg).font(font).color(color));
    if !button {
        return false;
    }
    ui.add_space(12.0);
    // The QR tab's one primary.
    super::messages::button(&mut ui, &super::messages::Pal::new(theme), super::messages::Btn::Primary, "Get a new code", true).clicked()
}

/// Dark modules on a white rounded square (its margin is the quiet zone), snapped to whole
/// pixels so the code stays crisp at any scale factor.
fn paint_qr(ui: &egui::Ui, rect: Rect, code: &QrCode, radius: f32) {
    let ppp = ui.ctx().pixels_per_point();
    let n = code.size();
    let module = qr_module_px(rect.width() * ppp, n) / ppp;
    let painter = ui.painter();
    painter.rect_filled(rect.round_to_pixels(ppp), radius, Color32::WHITE);
    let origin = (rect.center() - Vec2::splat(module * n as f32 / 2.0)).round_to_pixels(ppp);
    for y in 0..n {
        for (x, len) in dark_runs(code, y) {
            let min = origin + egui::vec2(x as f32, y as f32) * module;
            painter.rect_filled(Rect::from_min_size(min, egui::vec2(len as f32, 1.0) * module), 0.0, Color32::BLACK);
        }
    }
}

/// Largest whole-pixel module size that fits `n` modules plus the quiet zone into `side_px`.
fn qr_module_px(side_px: f32, n: i32) -> f32 {
    (side_px / (n + 2 * QR_QUIET) as f32).floor().max(1.0)
}

/// (start, length) of each run of dark modules in row `y` (one rect per run, not per module).
fn dark_runs(code: &QrCode, y: i32) -> impl Iterator<Item = (i32, i32)> + '_ {
    let n = code.size();
    let mut x = 0;
    std::iter::from_fn(move || {
        while x < n && !code.get_module(x, y) {
            x += 1;
        }
        let start = x;
        while x < n && code.get_module(x, y) {
            x += 1;
        }
        (x > start).then_some((start, x - start))
    })
}

// ---------------------------------------------------------------------------
// Email tab
// ---------------------------------------------------------------------------

fn email_tab(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    let busy = cx.view.login_busy;
    let offline = crate::config::offline_mode();

    let form = &mut cx.view.login_form;
    let email = egui::TextEdit::singleline(&mut form.email).hint_text("Email or phone number").char_limit(320).interactive(!busy);
    let (email, _) = field(ui, cx.theme, email, None, false);
    ui.add_space(12.0);
    let password = egui::TextEdit::singleline(&mut *form.password).id(password_id()).hint_text("Password");
    let password = password.char_limit(PASSWORD_LIMIT).interactive(!busy);
    let password = secret_field(ui, cx.theme, password, &mut form.show_password);
    // Before the auto-focus below: re-focusing would make lost_focus() false.
    let enter = (email.lost_focus() || password.lost_focus()) && ui.input(|i| i.key_pressed(Key::Enter));
    if !busy && !enter && ui.memory(|m| m.focused().is_none()) {
        if form.email.trim().is_empty() { email.request_focus() } else { password.request_focus() }
    }
    if email.changed() || password.changed() {
        cx.view.login_error = None;
    }
    captcha_slot(ui, cx);

    ui.add_space(12.0);
    remember(ui, cx);
    error_line(ui, cx);
    demo_captcha_login(cx);

    ui.add_space(16.0);
    let form = &cx.view.login_form;
    let can_submit = !busy && !offline && !form.email.trim().is_empty() && !form.password.is_empty();
    let resp = primary_button(ui, cx, if busy { "Signing in…" } else { "Sign in" }, can_submit);
    if can_submit && (resp.clicked() || enter) {
        let form = &mut cx.view.login_form;
        // Move the typed bytes into the Secret (no copy) and keep a fresh pre-sized buffer.
        let password = Secret::new(std::mem::replace(&mut *form.password, String::with_capacity(4 * PASSWORD_LIMIT)));
        forget_history(ui.ctx(), password_id());
        let login = form.email.trim().to_owned();
        cx.cmd.send(Command::LoginPassword { login, password, remember: cx.view.login_remember, captcha: None });
        cx.view.login_busy = true;
        cx.view.login_error = None;
    }
}

/// Test hook `RUSTCORD_CAPTCHA_DEMO=auto|open`: signs in once by itself (nothing typed; the backend
/// never sends it to Discord) so a screenshot can show the captcha in the card (`open`: as if its
/// challenge were open).
fn demo_captcha_login(cx: &mut UiCx<'_>) {
    static SENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if std::env::var("RUSTCORD_CAPTCHA_DEMO").is_ok_and(|v| v == "auto" || v == "open") && !cx.view.login_busy && !SENT.swap(true, std::sync::atomic::Ordering::Relaxed) {
        let password = Secret::new("demo-password".into());
        cx.cmd.send(Command::LoginPassword { login: "demo@example.test".into(), password, remember: false, captcha: None });
        cx.view.login_busy = true;
    }
}

fn mfa_label(m: MfaMethod) -> &'static str {
    match m {
        MfaMethod::Totp => "App",
        MfaMethod::Sms => "SMS",
        MfaMethod::Backup => "Backup",
    }
}

fn mfa_subtitle(mfa: &Mfa) -> String {
    match (mfa.method, &mfa.sms_phone) {
        (MfaMethod::Totp, _) => "Enter the code from your authenticator app".into(),
        (MfaMethod::Sms, None) => "Get a code by SMS".into(),
        (MfaMethod::Sms, Some(phone)) => format!("Code sent to {phone}"),
        (MfaMethod::Backup, _) => "Enter one of your backup codes".into(),
    }
}

fn mfa_tab(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    let busy = cx.view.login_busy;
    let offline = crate::config::offline_mode();
    let muted = cx.theme.colors.text_muted.to_color32();
    let theme = &*cx.theme;
    let Some(mfa) = cx.view.login_form.mfa.as_mut() else { return };

    if mfa.methods.len() > 1 {
        let options: Vec<_> = mfa.methods.iter().map(|&m| (m, mfa_label(m))).collect();
        ui.add_enabled_ui(!busy, |ui| segmented(ui, theme, &mut mfa.method, &options));
        ui.add_space(16.0);
    }

    let hint_text = match mfa.method {
        MfaMethod::Totp => "6-digit code",
        MfaMethod::Sms => "Code from the SMS",
        MfaMethod::Backup => "8-character backup code",
    };
    let sms = (mfa.method == MfaMethod::Sms).then_some(if mfa.sms_phone.is_some() { "Resend" } else { "Send code" });
    let edit = egui::TextEdit::singleline(&mut *mfa.code).id(code_id()).hint_text(hint_text).char_limit(CODE_LIMIT).interactive(!busy);
    let (input, send_sms) = field(ui, theme, edit, sms, !busy && !offline);
    let enter = input.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    if !busy && !enter && ui.memory(|m| m.focused().is_none()) {
        input.request_focus();
    }
    // SMS codes only exist once one was sent for this login.
    let ready = mfa.method != MfaMethod::Sms || mfa.sms_phone.is_some();
    let can_submit = !busy && !offline && ready && !mfa.code.trim().is_empty();
    let method = mfa.method;
    if input.changed() {
        cx.view.login_error = None;
    }
    captcha_slot(ui, cx);

    // No "Stay signed in" here: SubmitMfa cannot carry it; the password step's choice applies.
    error_line(ui, cx);

    ui.add_space(16.0);
    let resp = primary_button(ui, cx, if busy { "Verifying…" } else { "Verify" }, can_submit);
    if can_submit && (resp.clicked() || enter) {
        if let Some(mfa) = cx.view.login_form.mfa.as_mut() {
            let code = Secret::new(std::mem::replace(&mut *mfa.code, String::with_capacity(4 * CODE_LIMIT)));
            forget_history(ui.ctx(), code_id());
            cx.cmd.send(Command::SubmitMfa { method, code, captcha: None });
        }
        cx.view.login_busy = true;
        cx.view.login_error = None;
    } else if send_sms {
        cx.cmd.send(Command::SendMfaSms { captcha: None });
        cx.view.login_busy = true;
        cx.view.login_error = None;
    }

    ui.add_space(12.0);
    let back = ui.vertical_centered(|ui| ui.add_enabled(!busy, text_button(RichText::new("Back").color(muted)))).inner;
    if back.clicked() {
        cx.view.login_form.mfa = None; // drop wipes the code buffer
        forget_history(ui.ctx(), code_id());
        cx.view.login_error = None;
    }
}

// ---------------------------------------------------------------------------
// Token tab
// ---------------------------------------------------------------------------

fn token_tab(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    let busy = cx.view.login_busy;
    let offline = crate::config::offline_mode();
    let size = cx.theme.font.size;
    let (text, muted) = (cx.theme.colors.text.to_color32(), cx.theme.colors.text_muted.to_color32());

    let input = egui::TextEdit::singleline(&mut cx.view.login_input).id(token_id()).hint_text("Paste your token");
    let input = secret_field(ui, cx.theme, input.char_limit(600).interactive(!busy), &mut cx.view.login_show_token);
    // Before the auto-focus below: re-focusing would make lost_focus() false.
    let enter = input.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    if !busy && !enter && ui.memory(|m| m.focused().is_none()) {
        input.request_focus();
    }
    if input.changed() {
        cx.view.login_error = None;
    }

    ui.add_space(12.0);
    ui.horizontal(|ui| {
        remember(ui, cx);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // Brighter and underlined on hover (last pass's state), so it reads as hoverable.
            let hovered = ui.ctx().read_response(ui.next_auto_id()).is_some_and(|r| r.hovered());
            let help = RichText::new("Where do I find it?").size(size);
            let help = if hovered { help.color(text).underline() } else { help.color(muted) };
            ui.add(egui::Label::new(help).sense(Sense::hover())).on_hover_cursor(egui::CursorIcon::Help).on_hover_text(TOKEN_HELP);
        });
    });
    error_line(ui, cx);

    ui.add_space(16.0);
    let can_submit = !busy && !offline && !cx.view.login_input.trim().is_empty();
    let resp = primary_button(ui, cx, if busy { "Signing in…" } else { "Sign in" }, can_submit);
    if can_submit && (resp.clicked() || enter) {
        submit_token(ui.ctx(), cx);
    }
}

fn submit_token(ctx: &egui::Context, cx: &mut UiCx<'_>) {
    match Token::parse(&cx.view.login_input) {
        Some(token) => {
            forget_history(ctx, token_id());
            cx.cmd.send(Command::Login { token, remember: cx.view.login_remember });
            cx.view.login_busy = true;
            cx.view.login_error = None;
        }
        None => {
            cx.view.login_error =
                Some("That does not look like an account token. Paste the whole value (bot tokens are not supported).".into());
        }
    }
}

// ---------------------------------------------------------------------------
// Shared pieces
// ---------------------------------------------------------------------------

/// Full-width 40 pt field on the inset color. `action` adds a small text button inside its right
/// edge; returns the field's response and whether that button was clicked.
fn field(ui: &mut egui::Ui, theme: &Theme, edit: egui::TextEdit<'_>, action: Option<&str>, action_enabled: bool) -> (egui::Response, bool) {
    let action_id = egui::IdSalt::new("login_field_action");
    let action_font = FontId::proportional(theme.font.size - 1.0);
    let muted = theme.colors.text_muted.to_color32();
    let edit = edit
        .desired_width(f32::INFINITY)
        .min_size(vec2(0.0, FIELD_H))
        .align(Align2::LEFT_CENTER)
        .background_color(theme.colors.code_background.to_color32());
    let edit = match action {
        Some(label) => {
            let text_w = ui.painter().layout_no_wrap(label.to_owned(), action_font.clone(), muted).size().x;
            let w = text_w + 2.0 * ui.spacing().button_padding.x + 2.0;
            edit.margin(Margin { left: 12, right: 4, top: 0, bottom: 0 }).suffix(egui::Atom::custom(action_id, vec2(w, FIELD_H - 10.0)))
        }
        None => edit.margin(Margin::symmetric(12, 0)),
    };
    // TextEdit always paints its hint in weak_text_color (a RichText colour is overridden).
    let out = ui.scope(|ui| {
        ui.visuals_mut().weak_text_color = Some(hint_color(theme));
        edit.show(ui)
    });
    let out = out.inner;
    let clicked = match (action, out.response.rect(action_id)) {
        (Some(label), Some(rect)) => {
            let button = text_button(RichText::new(label).font(action_font).color(muted)).min_size(rect.size());
            ui.add_enabled_ui(action_enabled, |ui| ui.place(rect, button)).inner.clicked()
        }
        _ => false,
    };
    (out.response.response, clicked)
}

/// Placeholder colour: text_muted, dimmed where it is almost as bright as typed text (High
/// Contrast), so empty fields do not look filled in.
fn hint_color(theme: &Theme) -> Color32 {
    let c = &theme.colors;
    let muted = c.text_muted.to_color32();
    let lum = |color: Color32| egui::Rgba::from(color).intensity() + 0.05;
    let (a, b) = (lum(c.text.to_color32()), lum(muted));
    if a.max(b) / a.min(b) < 1.8 { muted.gamma_multiply(0.7) } else { muted }
}

/// `field`, masked unless `show`, with a Show/Hide toggle inside it.
fn secret_field(ui: &mut egui::Ui, theme: &Theme, edit: egui::TextEdit<'_>, show: &mut bool) -> egui::Response {
    let (resp, toggle) = field(ui, theme, edit.password(!*show), Some(if *show { "Hide" } else { "Show" }), true);
    if toggle {
        *show = !*show;
    }
    resp
}

/// Borderless button that only shows its background on hover (Show/Hide, Back).
fn text_button(text: RichText) -> egui::Button<'static> {
    egui::Button::new(text).frame_when_inactive(false)
}

/// The shared "Stay signed in" checkbox (the token goes to the OS credential store): the §9
/// checkbox (`positive` with a check when on, an `edge` ring when off, never accent).
fn remember(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    let pal = super::messages::Pal::new(cx.theme);
    let label = "Stay signed in";
    let font = FontId::proportional(pal.ui());
    let text_w = ui.painter().layout_no_wrap(label.to_owned(), font.clone(), pal.text).size().x;
    let enabled = !cx.view.login_busy;
    let (rect, resp) = ui.allocate_exact_size(vec2(20.0 + 8.0 + text_w, 24.0), if enabled { Sense::click() } else { Sense::hover() });
    let changed = enabled && resp.clicked();
    if changed {
        cx.view.login_remember = !cx.view.login_remember;
    }
    let on = cx.view.login_remember;
    let check = Rect::from_center_size(egui::pos2(rect.left() + 10.0, rect.center().y), Vec2::splat(20.0));
    super::messages::paint_checkbox(ui.painter(), &pal.tk, check, on, f32::from((pal.radius / 2).max(2)));
    ui.painter().text(egui::pos2(check.right() + 8.0, rect.center().y), Align2::LEFT_CENTER, label, font, if enabled { pal.text } else { pal.muted });
    if resp.has_focus() {
        super::messages::focus_ring(ui, &pal, rect, f32::from((pal.radius / 2).max(2)));
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, enabled, on, label));
    let resp = resp.on_hover_text("Remember this account on this device (saved in the OS credential store).");
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    // A running QR login was started with the old choice: replace it.
    if changed && cx.view.login_form.tab == LoginTab::Qr && matches!(cx.view.login_form.qr, QrState::Waiting | QrState::Code(_)) {
        start_qr(cx);
    }
}

/// Discord's captcha right here in the card (Windows, macOS): a slot for hCaptcha's checkbox that
/// `captcha::Inline::sync` covers with a webview, then a hint and Cancel. Nothing while none is up.
fn captcha_slot(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    if cx.view.login_captcha.is_none() {
        return;
    }
    ui.add_space(12.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), CAPTCHA_H), Sense::hover());
    // egui cannot clip a native child: it only shows while all of its slot is in view.
    cx.view.login_captcha_slot = Some((rect, ui.clip_rect().contains_rect(rect)));
    ui.add_space(8.0);
    let (muted, text) = (cx.theme.colors.text_muted.to_color32(), cx.theme.colors.text.to_color32());
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Discord wants to make sure you're a person.").color(muted));
        let cancel = ui.add(text_button(RichText::new("Cancel").color(text)));
        if cancel.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            cx.cmd.send(Command::Captcha(CaptchaReply::Cancelled));
        }
    });
}

fn error_line(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    if let Some(err) = &cx.view.login_error {
        ui.add_space(12.0);
        ui.label(RichText::new(err).color(cx.theme.colors.danger.to_color32()));
    }
}

/// Full-width primary button (§9, 40 tall); shows a spinner while a login request is in flight.
fn primary_button(ui: &mut egui::Ui, cx: &UiCx<'_>, label: &str, enabled: bool) -> egui::Response {
    let c = &cx.theme.colors;
    let busy = cx.view.login_busy;
    // A faded accent button reads as muddy dark-on-rust; disabled gets the neutral fill instead.
    let neutral = cx.theme.tokens().selected;
    let (fill, text) = if enabled || busy { (c.accent.to_color32(), c.accent_text) } else { (neutral, c.text_muted) };
    let label = RichText::new(label).font(FontId::new(cx.theme.font.size - 1.0, crate::theme::medium_family())).color(text.to_color32());
    let button = egui::Button::new((egui::Atom::grow(), label, egui::Atom::grow()))
        .fill(fill)
        .stroke(egui::Stroke::NONE)
        .corner_radius(cx.theme.layout.corner_radius)
        .min_size(vec2(ui.available_width(), FIELD_H));
    // Not ready or in flight: disabled (not clickable, reported as such) but drawn at full
    // strength; egui's extra fade would turn the neutral or accent fill into a ghost bar.
    let resp = ui.scope(|ui| {
        ui.visuals_mut().disabled_alpha = 1.0;
        ui.add_enabled(enabled, button)
    });
    let resp = resp.inner;
    if busy {
        let r = Rect::from_center_size(egui::pos2(resp.rect.left() + 22.0, resp.rect.center().y), Vec2::splat(18.0));
        egui::Spinner::new().color(c.accent_text.to_color32()).paint_at(ui, r);
    }
    if crate::config::offline_mode() { resp.on_disabled_hover_text(OFFLINE_HINT) } else { resp }
}

// ---------------------------------------------------------------------------
// Backend events
// ---------------------------------------------------------------------------

/// Progress of an email/password or QR login.
pub fn on_step(view: &mut super::ViewState, step: LoginStep) {
    view.login_captcha = None;
    let form = &mut view.login_form;
    // QR events that were already queued when the tab was left (cancel in flight) are stale.
    let qr_live = form.tab == LoginTab::Qr && !matches!(form.qr, QrState::Idle);
    match step {
        // Busy stays on until the captcha ends; the QR tab keeps its scanned state.
        LoginStep::Captcha { sitekey, rqdata } => {
            view.login_captcha = Some((sitekey, rqdata));
            view.login_busy = true;
        }
        LoginStep::QrCode { url } if qr_live => {
            form.qr = match QrCode::encode_text(&url, QrCodeEcc::Medium) {
                Ok(code) => QrState::Code(code),
                Err(_) => QrState::Failed("Could not draw the QR code.".into()),
            };
        }
        LoginStep::QrScanned { username, avatar_url } if qr_live => {
            form.qr = QrState::Scanned { username, avatar_url };
            // The token exchange follows the phone confirmation; busy routes its errors here.
            view.login_busy = true;
        }
        LoginStep::QrExpired if qr_live => {
            form.qr = QrState::Expired;
            view.login_busy = false;
        }
        LoginStep::Failed(msg) if qr_live => {
            form.qr = QrState::Failed(msg);
            view.login_busy = false;
        }
        LoginStep::QrCode { .. } | LoginStep::QrScanned { .. } | LoginStep::QrExpired => {}
        LoginStep::MfaRequired { methods } => {
            let method = methods.first().copied().unwrap_or(MfaMethod::Totp);
            form.mfa = Some(Mfa { methods, method, code: secret_buf(CODE_LIMIT), sms_phone: None });
            view.login_busy = false;
            view.login_error = None;
        }
        LoginStep::SmsSent { phone } => {
            if let Some(mfa) = &mut form.mfa {
                mfa.sms_phone = Some(phone);
                mfa.method = MfaMethod::Sms;
            }
            view.login_busy = false;
        }
        LoginStep::Failed(msg) => {
            // The backend dropped the 2FA ticket: back to the password form (AUTH_NOTES §1.4).
            if msg == crate::api::auth::EXPIRED {
                form.mfa = None;
            }
            view.login_busy = false;
            view.login_error = Some(msg);
        }
    }
}

/// Test hook `RUSTCORD_LOGIN_TAB`: `mfa` shows the 2FA form with every method (no network).
pub fn open_test_tab(view: &mut super::ViewState, tab: &str) {
    view.login_form.tab = match tab {
        "email" | "mfa" => LoginTab::Email,
        "token" => LoginTab::Token,
        _ => LoginTab::Qr,
    };
    if tab == "mfa" {
        on_step(view, LoginStep::MfaRequired { methods: vec![MfaMethod::Totp, MfaMethod::Sms, MfaMethod::Backup] });
    }
}

/// Leaving the login screen: stop a QR login and wipe every login buffer.
pub fn on_logged_in(ctx: &egui::Context, view: &mut super::ViewState, cmd: &CommandSender) {
    forget_all_history(ctx);
    ctx.data_mut(|d| d.remove::<egui::TextureHandle>(icon_id()));
    // A scanned code is what logged us in: cancelling it now could cut its remember step short.
    if !matches!(view.login_form.qr, QrState::Scanned { .. }) {
        cancel_qr(&mut view.login_form, cmd);
    }
    view.login_form = LoginForm::default(); // drop wipes password and 2FA code
    view.login_input.zeroize();
    view.login_show_token = false;
    view.login_busy = false;
    view.login_captcha = None;
    view.login_error = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::ViewState;

    const URL: &str = "https://discord.com/ra/UZ0-kOVzXDZTFVV5_QlpURSO2BQHrtkKWHNpIGoDI0k";

    fn qr_view() -> ViewState {
        let mut v = ViewState::default();
        v.login_form.qr = QrState::Waiting;
        v
    }

    #[test]
    fn the_captcha_step_keeps_the_login_busy_until_another_step_ends_it() {
        let mut v = qr_view();
        on_step(&mut v, LoginStep::Captcha { sitekey: "k".into(), rqdata: None });
        assert!(v.login_captcha == Some(("k".into(), None)) && v.login_busy);
        on_step(&mut v, LoginStep::Failed("The captcha wasn't finished".into()));
        assert!(v.login_captcha.is_none() && !v.login_busy);
        assert_eq!(v.login_error, None); // the QR tab shows it in its own square
        assert!(matches!(v.login_form.qr, QrState::Failed(_)));
    }

    #[test]
    fn dark_runs_cover_exactly_the_dark_modules() {
        let code = QrCode::encode_text(URL, QrCodeEcc::Medium).unwrap();
        for y in 0..code.size() {
            let mut row = vec![false; code.size() as usize];
            for (x, len) in dark_runs(&code, y) {
                assert!(len > 0);
                row[x as usize..(x + len) as usize].iter_mut().for_each(|m| *m = true);
            }
            for (x, &dark) in row.iter().enumerate() {
                assert_eq!(dark, code.get_module(x as i32, y));
            }
        }
    }

    #[test]
    fn module_size_is_whole_pixels_and_fits() {
        let n = QrCode::encode_text(URL, QrCodeEcc::Medium).unwrap().size();
        for ppp in [1.0, 1.25, 1.5, 2.0] {
            let side_px = QR_SIDE * ppp;
            let m = qr_module_px(side_px, n);
            assert_eq!(m.fract(), 0.0);
            assert!(m * (n + 2 * QR_QUIET) as f32 <= side_px);
            assert!((m + 1.0) * (n + 2 * QR_QUIET) as f32 > side_px);
        }
        assert_eq!(qr_module_px(10.0, 41), 1.0);
    }

    #[test]
    fn qr_steps_follow_the_qr_tab() {
        let mut v = qr_view();
        on_step(&mut v, LoginStep::QrCode { url: URL.into() });
        assert!(matches!(v.login_form.qr, QrState::Code(_)));
        on_step(&mut v, LoginStep::QrScanned { username: "a".into(), avatar_url: None });
        assert!(matches!(v.login_form.qr, QrState::Scanned { .. }) && v.login_busy);
        on_step(&mut v, LoginStep::Failed("x".into()));
        assert!(matches!(&v.login_form.qr, QrState::Failed(m) if m == "x"));
        assert!(!v.login_busy && v.login_error.is_none());
        on_step(&mut v, LoginStep::QrExpired);
        assert!(matches!(v.login_form.qr, QrState::Expired));
    }

    #[test]
    fn stale_qr_steps_are_ignored_and_failures_go_inline() {
        let mut v = ViewState::default();
        v.login_form.tab = LoginTab::Email;
        on_step(&mut v, LoginStep::QrCode { url: URL.into() });
        assert!(matches!(v.login_form.qr, QrState::Idle));
        v.login_busy = true;
        on_step(&mut v, LoginStep::Failed("Wrong password".into()));
        assert!(!v.login_busy);
        assert_eq!(v.login_error.as_deref(), Some("Wrong password"));
    }

    #[test]
    fn mfa_steps() {
        let mut v = ViewState::default();
        v.login_form.tab = LoginTab::Email;
        v.login_busy = true;
        on_step(&mut v, LoginStep::MfaRequired { methods: vec![MfaMethod::Sms, MfaMethod::Backup] });
        let mfa = v.login_form.mfa.as_ref().unwrap();
        assert_eq!(mfa.method, MfaMethod::Sms);
        assert!(!v.login_busy);
        v.login_busy = true;
        on_step(&mut v, LoginStep::SmsSent { phone: "+***85".into() });
        assert_eq!(v.login_form.mfa.as_ref().unwrap().sms_phone.as_deref(), Some("+***85"));
        assert!(!v.login_busy);
        on_step(&mut v, LoginStep::Failed("Wrong code. Try again.".into()));
        assert!(v.login_form.mfa.is_some());
        // Ticket gone in the backend: back to the password form.
        on_step(&mut v, LoginStep::Failed(crate::api::auth::EXPIRED.into()));
        assert!(v.login_form.mfa.is_none());
        assert_eq!(v.login_error.as_deref(), Some(crate::api::auth::EXPIRED));
    }

    #[test]
    fn logged_in_wipes_buffers_and_resets_tabs() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cmd = CommandSender(tx);
        let ctx = egui::Context::default();
        let mut v = qr_view();
        v.login_form.password.push_str("hunter2");
        v.login_input.push_str("tok");
        on_logged_in(&ctx, &mut v, &cmd);
        assert!(matches!(rx.try_recv(), Ok(Command::CancelQrLogin)));
        assert!(v.login_form.password.is_empty() && v.login_input.is_empty());
        assert!(matches!(v.login_form.qr, QrState::Idle) && v.login_form.tab == LoginTab::Qr);

        v.login_form.qr = QrState::Scanned { username: "a".into(), avatar_url: None };
        on_logged_in(&ctx, &mut v, &cmd);
        assert!(rx.try_recv().is_err());
    }
}
