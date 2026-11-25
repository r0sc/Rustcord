# Social / server / settings UI design (who owns which surface)

The UI block on top of the finished backends in `docs/SOCIAL_DESIGN.md` (threads, forums,
friends, DMs, inbox, profiles) and `docs/ACCOUNT_SERVER_DESIGN.md` (settings proto, own profile,
server admin, invites, notifications). Their "UI hook-up guide" tables say which command each
control sends and which state it reads; this page says **which file draws it**. Layout and
behaviour: `docs/FEATURES_NOTES.md` Part 2, Part 3 and Part 4 (layout reference). Rustcord's own
branding (theme palette, own logo), English UI text, icons with tooltips, hints only where a
control needs explaining.

## Status
Built (offline-verified with the test hooks and unit tests; nothing run against the live API
yet): Friends page, group DMs, inbox, profile popout + full profile, server bar folders and drag
reorder, threads (sidebar rows, popout, split panel, Create Thread) and forums (list / gallery,
New Post), server menus and dialogs (create channel, invite, join / create server, kick / ban),
server settings (Overview, Roles + Edit Role, Emoji, Members, Invites, Bans, Edit Channel with
permission overwrites), user settings (My Account, Profiles, Data & Privacy, Notifications,
Voice & Video, Set Custom Status, the account popout's status picker on `SetPersistentStatus`).
Open items: "Not assigned yet" below and each module's doc comment.

## File ownership (one engineer per row; never edit another row's files)
| Owner | Files | Surfaces |
|---|---|---|
| contract owner (shell) | `ui/mod.rs`, `messages.rs` header/menu hooks, `members.rs`, this page | `ViewState` fields, `Modal`, `Confirm` + `ViewState::confirm`, `show_modals`, `dialog*`, `settings_layer`, `nav_*`, `section`, `hint`, `main_layout` routing, test hooks |
| navigation-social (friends / DMs / inbox / profile) | `friends.rs`, `requests.rs`, `inbox.rs`, `profile.rs`, `profile_modal.rs`, `sidebar.rs` | server bar (folders, drag reorder), Friends page (tabs, Add Friend, Pending with Spam, Active Now, the 80013 question), Message requests page (Requests / Spam, previews, Accept, Ignore, batch Ignore) and the request bar, user / DM menu items, New Group DM picker, inbox popout (Mentions, Unreads, Mark all read), profile popout (About Me, Member Since, Note, `FetchProfile` on open, the action row), full profile modal |
| threads / forum | `threads.rs`, `forum.rs`; the thread panel's message list and composer need `messages::show` / `composer::show` for a channel other than the selected one: that change in `messages.rs` / `composer.rs` is theirs too (keep `chat_header` / `message_menu` hooks) | sidebar thread rows, Threads button + popout (Active / Archived, Join), thread split panel, Create Thread modal, forum / media post list (sort, tags, cards, paging), New Post modal; the chat header's breadcrumb for threads and forum glyph |
| server | `server_menu.rs`, `server_settings.rs` | server header + its dropdown, server icon / channel / category / member menu items, Create Channel / Category, Invite, Add a Server (join + create), Kick / Ban modal, server settings layer (Overview, Roles, Emoji, Members, Invites, Bans, Edit Channel incl. permission overwrites) |
| user settings | `user_settings.rs`, `user_settings/theme_editor.rs`, `user_settings/local_profile.rs` | user settings layer (My Account, Profiles, Data & Privacy, Local Profile, Appearance, Notifications, Voice & Video, Log Out), Set Custom Status modal, the account popout's lower part (`account_popout`: status picker, custom status, Edit Profile) |

Each module keeps its own UI state in one `*Ui` struct defined in its file (`friends_ui`,
`inbox_ui`, `threads_ui`, `forum_ui`, `profile_modal_ui`, `server_menu_ui`,
`server_settings_ui`, `user_settings_ui` in `ViewState`): add fields there freely. Private
helpers and new private submodules inside one's own file are fine. A change to `mod.rs`
(new `Modal` variant, `ViewState` field, shared helper) or `messages.rs` goes through the
contract owner, one to `sidebar.rs` through the navigation-social engineer: report it, smallest
additive change.

## Shared contract (ui/mod.rs)
- `ViewState`:
  - `settings_open` + `settings_page: user_settings::Page`: user settings layer (full screen).
  - `server_settings: Option<(Id, server_settings::Page)>`: server settings layer;
    `Page::Channel(id)` is Edit Channel / Category, `Page::Role(id)` Edit Role. Open with
    `server_settings::open` (`first_page(state, g)`: the first page I may see), close with
    `server_settings::close` (drops the bans / invites views).
  - `modal: Option<Modal>`: the one open dialog; `confirm: Option<Confirm>` stacks above it.
    `dialog_open()` = either is open (Esc / clicks belong to it).
  - `thread_panel: Option<Id>`: thread docked right of its parent's chat. The shell loads its
    history, acks it (`AckThread`) and closes it once its parent is not the open channel.
  - `inbox_open: bool`: inbox popout (Ctrl+I). Its mentions view is dropped when it closes.
  - `chat_hidden()`: a settings layer covers the chat (no read acks, no pickers, inbox closed).
- `Modal` (variant = target only, drafts live in the module's `*Ui`): `CreateThread { channel_id,
  from_message }`, `NewForumPost { forum_id }`, `Profile { user_id, guild_id }`,
  `GroupDm { channel_id }`, `CreateChannel { guild_id, parent_id, category }`,
  `Invite { guild_id, channel_id }`, `JoinServer`, `Moderate { guild_id, user_id, ban }`,
  `CustomStatus`.
- `show_modals(ctx, cx)`: the single call site of every modal (after the page and the profile
  card in `RustcordApp::ui`), then the confirmation.
- **Confirm dialog:** `cx.view.confirm(title, body, button, then: Command)`: Cancel + a danger
  button; the command is sent once, on that button only. `danger: false` (set the field) gives
  an accent button, e.g. the 80013 "accept a stranger" question. Use it for every destructive
  action: delete channel / role / emoji / invite, kick, ban (or the Moderate modal), leave
  server, remove friend, block, close / leave group.
- Dialog look: `dialog(ctx, theme, salt, width, title, add)` (442 / 602 / 800 wide, under the
  title bar; Esc, its × and the backdrop make `should_close()` true; `dialog_closable(.., false, ..)`
  hides the × while busy), `dialog_text`, `dialog_buttons(ui, theme, action, danger,
  enabled) -> Option<bool>`, `dialog_footer(ui, add)` (own last row, pinned while the body scrolls).
- Settings layers: `settings_layer(ui, cx, salt, nav, page) -> close?` (nav column, ≤ 740 px page,
  60 px tools column with the round ×), `nav_heading`, `nav_item`, `page_title`, `section`,
  `hint`.
- Reusable elsewhere: `messages::{Pal, icon_button, menu_item, menu_setup, MenuIcon,
  small_button, status_chip, message_card}`, `images::{initial_colors, circle_image, avatar}`,
  `sidebar::{line, row_fills, mark_read, user_avatar_at, user_dot, badge, toggle_voice}`,
  `server_menu::more_button` (painted ⋯: the fonts lack the glyph), `avatar_at`, `one_line`,
  `bold`, `button_row`.
- Module APIs added while building (pub(super), additive): `messages::thread_chat(ui, cx,
  thread)` (the panel's own list + composer); `composer::show(ui, cx, ch, chat_top, panel)` (only
  caller `messages::chat`); `composer::{form_files, form_file_list, clear_form_files}` (a file
  tray for forms, keyed by an id, e.g. New Post); `threads::{track, breadcrumb, header_glyph,
  known, open_in_panel, thread_menu, load, valid_archive}`; `friends::{ask_remove, ask_block,
  ask_stranger}`; `profile_modal::{open_tab, fetch, loaded, note_field, save_note}`;
  `user_settings::restore_voice` (startup: saved devices / input mode to the voice task).
- Voice & Video choices are saved in `Settings` (`voice_input`, `voice_output`,
  `voice_threshold_db`, `push_to_talk`, `push_to_talk_key`, `camera`); the call UI's camera
  button passes `settings.camera` to `VideoCommand::StartCamera`.

## Hooks (where each module is called from)
| Caller | Call | Module |
|---|---|---|
| `main_layout` | DM home with no DM open: `friends::is_open` → `friends::show` (no right column) | friends |
| `main_layout` | `requests::sync` first in every pass; the DM home shows `requests::show` while `requests_ui.open` | requests |
| `composer::show` | a request's DM (`!can_send`): `requests::bar` instead of the composer | requests |
| `main_layout` | open channel is a forum / media channel: `forum::open_forum` → `forum::show` in the chat's place | forum |
| `main_layout` | `thread_panel` → `threads::panel` (full-height right column, 400 px; narrow windows: in the chat's place) | threads |
| `main_layout` | `settings_open` → `user_settings::show`; `server_settings` → `server_settings::show` | settings |
| `RustcordApp::ui` | `show_modals` → `threads::create_modal`, `forum::new_post_modal`, `profile_modal::show`, `friends::group_dm_modal`, `server_menu::{create_channel_modal, invite_modal, join_server_modal, moderate_modal}`, `user_settings::custom_status_modal`; then the confirmation | all |
| `sidebar::guild_bar` | "Add a Server" (+) → `server_menu::open(cx, Modal::JoinServer)`; icon right-click → `server_menu::guild_menu` | server |
| `sidebar::channel_list` | guild header → `server_menu::server_header` (dropdown = `guild_menu`) | server |
| `sidebar::guild_channels` | under each channel: `threads::sidebar_rows(ui, cx, parent, &sidebar_threads(g)[parent])`; category right-click → `server_menu::category_menu` | threads, server |
| `sidebar::channel_menu` | `server_menu::channel_menu_items` (Edit / Delete Channel) before Copy Link | server |
| `sidebar::user_menu` (member list, voice users) | `friends::user_menu_items` (Message, Add Friend), then `server_menu::member_menu_items` (Kick, Ban) | friends, server |
| `sidebar::dm_list` | "Message requests" row (under Quests) → `requests::open`; "Friends" row → `friends::open`; header "+" → `friends::open_group_dm`; DM menu → `friends::dm_menu_items` | friends |
| `messages::chat_header` | `threads::header_button` (text / announcement channels) | threads |
| `sidebar::title_bar` | `inbox::button` (right end; not under a settings layer) | inbox |
| `messages::message_menu` | "Create Thread" (`can_create_thread`) → `threads::open_create(cx, ch, Some(msg))` | threads |
| `profile::show_popup` | others' card: `actions` → `friends::message_user`, `friends::add_friend`, `profile_modal::open`; my account popout: `user_settings::account_popout` | friends, user settings |
| server menu / channel menu | `server_settings::open(cx, g, Page::Overview \| Page::Channel(c))` | server |

## Rules for every module
- **Network only from an explicit user action**; opening a view sets it and sends its one fetch
  (profile, mentions, threads, bans, invites), never on a timer or per frame.
- **Destructive = confirm** (above). Kick / ban go through the Moderate modal (reason, ban
  delete-messages preset).
- **Gate locally before showing / sending**: `account_check(&cmd)` (admin; hide menu items
  without permission, disable form controls with the `Err` text as hover), `can_create_thread`,
  `can_create_post`, `can_manage_threads`, `can_join_thread`, `can_set_archived`,
  `can_list_archived`, `friends::can_add_friend`.
- **Progress of a form:** `if cx.state.account.begin(cmd.op()) { cx.cmd.send(Command::Account(cmd)) }`,
  then show `account.ops[&op]`; remove the entry when the form closes. Errors (captcha, 2FA,
  "too fast") are shown as the text they arrive with: never retried, bypassed or automated; the
  captcha text with the invite link is selectable.
- **Memory** (app ~160 MB live, target < 150 MB): no per-frame clones of big lists (iterate
  `state` or keep ids), lazy lists (`show_rows`, paging with `next_page` / `next_before` /
  `next_after`), drop views when their surface closes (`social.mentions`, `social.threads`,
  `account.bans`, `account.invites`, `account.created_invite`), free banner / thumbnail
  textures with their panel.
- No new crates; clippy clean; existing tests intact. Remove the backend's
  `#[allow(dead_code, reason = "…not built yet")]` markers once their items are used.

## Test hooks (`RUSTCORD_OPEN`, comma list)
The hooks themselves send nothing; a view they open sends its one fetch like a click would
(profile, mentions, threads, forum posts, invites, bans, device lists), which offline mode
answers at once with "Not connected" (no traffic). Screenshots: pin `RUSTCORD_POINTER` (e.g. in
the title bar) so the real mouse cannot hover or click anything.
`settings:<my-account|profiles|cosmetics|content|privacy|family|apps|devices|connections|friend-requests|billing|appearance|accessibility|voice|chat|notifications|keybinds|language|windows|streamer|advanced|activity|games|overlay|plugins|local-profile>`,
`server-settings[:<overview|roles|emoji|members|invites|bans|role-<id>|channel-<id>>]` (open
guild; `role-<id>` Edit Role, `channel-<id>` Edit Channel), `server-menu` (the server header's
dropdown; with `RUSTCORD_POINTER` on a row its submenu opens too), `friends` (demo
relationships from cached users), `inbox`, `threads` (Threads popout),
`thread:<id>` (thread panel; selects its parent), `card:<user id>` (profile card),
`modal:<create-thread|new-post|profile|group-dm|create-channel|create-category|invite|
join-server|kick|ban|custom-status>` (profile / kick / ban: the lowest cached user id other than me
and the guild owner), `confirm` (Leave Server question for the open guild).
`RUSTCORD_NAV_DEMO=<comma list>` (sidebar.rs `nav_demo`, sends nothing): `folders` /
`folder-open` (server bar folders as READY would bring them), `mentions` (inbox mentions from
cached messages), `profile` (loaded profiles: bio, pronouns, mutuals, a note),
`tab:<online|all|pending|blocked|add>` (Friends tab). `RUSTCORD_OPEN=requests[:spam|ticked|confirm|open|spam-open]`
(`requests::demo`, sends nothing): the Message requests page with invented requests, the Spam
segment, two rows ticked, the batch Ignore question, or a request's DM with the bar. With `RUSTCORD_HOME` (a copy of
`tests/fixtures/home`, extended with threads / a forum / roles as needed), `RUSTCORD_OFFLINE=1`,
`RUSTCORD_WINDOW_SIZE=1180x760 | 720x420`, `RUSTCORD_SCREENSHOT=<png>`, `RUSTCORD_POINTER` /
`RUSTCORD_CLICK`. Add a hook for each new page or modal (one match arm in `startup_hooks` or a
`demo_modal` name: ask the contract owner).

## Not assigned yet
The chat header's notification bell (`UpdateChannelOverride`; the channel menu already has Mute ▸
and Notification Settings ▸), quick switcher, channel drag reorder in the sidebar
(`ReorderChannels`), server folder menu (rename / colour / ungroup), Change Nickname as a dialog
(now a field in the member menu's submenu), thread Mute / Notification Settings (the thread menu and
the thread panel's bell show them disabled until backend.rs has a
`PATCH /channels/{t}/thread-members/@me/settings` command), friends in voice on Active Now.
