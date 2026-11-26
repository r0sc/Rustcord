# Vencord plugins → native Rustcord features

Research date 2026-11-13. Unverified claims are marked **UNCONFIRMED**. Overlap with other docs: FakeNitro only (COSMETICS_NOTES).
Licence: Vencord and Equicord are GPL-3.0. Re-implement the behaviour; never copy code, keys or assets.

## 0. Editor's corrections (checked against sources on 2026-11-13)

| # | Draft claim | Resolution |
|---|---|---|
| 1 | NoTrack, Settings, SupportHelper and CrashHandler are in `_core` | **Wrong.** `src/plugins/_core` = ConcatenatedComponentExtractor, FixDiscordCss, NoTrack, Settings, SupportHelper (all `required`). CrashHandler is a normal folder with `enabledByDefault: true`. |
| 2 | 173 Vencord plugins | **Confirmed.** 168 folders plus 5 `_core` files. A scripted diff against the §3 table found 0 missing and 0 extra. vencord.dev/plugins returned an empty body to curl. |
| 3 | "40 A plugins", "see §4" | §4 lists **45 specs covering 48 plugins** (S34, S39 and S44 each combine two). Table references now point to the spec IDs S1–S45. |
| 4 | `show_current_game` proto field number UNCONFIRMED | **Resolved: settings proto 1, field 11.3, BoolValue.** Both the proto file and GameActivityToggle (`getUserSettingLazy("status","showCurrentGame")`) agree. |
| 5 | `favorite_emojis` field UNCONFIRMED | **Resolved: frecency proto (type 2), field 5** = `FavoriteEmojis { repeated string emojis = 1 }`. The string format (custom ID vs unicode) is UNCONFIRMED. |
| 6 | `client_status` keys `embedded`/`vr` UNCONFIRMED | **Resolved.** UD documents `desktop`, `mobile`, `web` (the default when unknown), `embedded` (Xbox/PlayStation) and `vr` (Meta Quest, headless sessions only). |
| 7 | Field name for "ignored users" UNCONFIRMED | **Resolved.** The relationship object has `user_ignored: bool` (UD). Relationship removal filter 2 = IGNORED. |
| 8 | `regex` is a new dependency | `regex` 1.13.1 is **already in Cargo.lock**: keyring → `windows-native-keyring-store` pulls it in. A direct dependency compiles nothing new on Windows. It still needs the usual "(approved)" line in Cargo.toml. |
| 9 | "Flag value 4096 in `state.rs`" | **Wrong.** No SUPPRESS_NOTIFICATIONS constant exists. `api/mod.rs::send_body` hard-codes `"flags": 0`. Add a `flags` parameter and `const SUPPRESS_NOTIFICATIONS: u64 = 1 << 12`. |
| 10 | IrcColors uses fixed saturation 100 and lightness 70 | Those are **settings**: `lightness` 70, `saturation` 100, `seed` "", `memberListColors` true, `applyColorOnlyToUsersWithoutColor` false, `applyColorOnlyInDms` false. Hue = xxhash64(id + seed) % 360. No xxhash crate is in the lock; colours need not match Vencord, so any stable hash will do (FNV-1a, about 5 lines). |
| 11 | HideMedia | The plugin is named "HideMedia" but its folder is `hideAttachments`. |
| 12 | Equicord adds "about 204" | `src/equicordplugins` has **205 entries** (Equibored `plugins.json`). Equicord also marks DisableDeepLinks and EquicordHelper as required. |
| 13 | Windows SAPI feature name UNCONFIRMED | **Resolved.** `windows` 0.62.2 has the `Win32_Media_Speech` feature (`= ["Win32_Media"]`). |
| 14 | FriendsSince / MoreUserTags | Confirmed: FriendsSince is absent from both Vencord and Equicord; MoreUserTags exists only in Equicord. |

Verified Rustcord hooks that already exist (by symbol; line numbers drift):

| Hook | Location |
|---|---|
| Who-reacted fetch | `Cmd::FetchReactionUsers {channel_id, message_id, emoji, burst, after}` in `backend.rs` |
| Mark all read | `Cmd::MarkAllRead {acks}` (`POST /read-states/ack-bulk`) in `backend.rs` |
| Suppress embeds | `Cmd::SuppressEmbeds` in `backend.rs` |
| Fetch around a message | `Cmd::FetchAround {channel_id, message_id, limit}` in `backend.rs` |
| Reply ping | `allowed_mentions.replied_user` in `api/mod.rs::send_body` |
| Typing | `POST /channels/{c}/typing` in `api/chat.rs` |
| Online count | `online_count` in `state/memstats.rs` |
| Profile data | `connected_accounts`, `communication_disabled_until` in `model.rs` |
| Settings proto | `edit()` plus `edit_settings()` in `api/settings_proto.rs` and `api/account.rs` |
| Tray icon | `notify.rs` (`Shell_NotifyIconW`) |

**Presence gap:** `gateway/normalize.rs` reduces `client_status` to `mobile = (only key is "mobile")`. PlatformIndicators needs the whole map.

## 1. Architecture: no plugin runtime

1. **Config.** Add `#[serde(default)] pub plugins: Plugins` to `config::Settings` (TOML, so new fields are free). Use one struct per plugin, `{ enabled: bool, ..options }`, defaulting to off. Use `#[serde(default)]` on every struct so old config files keep loading.
2. **Settings → Plugins page** (a new `Page::Plugins` in the APP group; Vencord's layout):
   - a search box and an All / Enabled / Disabled filter;
   - a grid of cards, each showing name, one-line description and a toggle;
   - a gear on each card that opens a hand-written options form;
   - a "restart needed" note only for plugins that hook startup (KeepCurrentChannel).
3. **Hook points** (`if cfg.plugins.x.enabled` at about 9 sites):

| # | Hook | Plugins |
|---|---|---|
| H1 | Outgoing send/edit (`send_body`) | ClearURLs, TextReplace, Translate (auto), SilentMessageToggle (`flags`), NoReplyMention (`replied_user`), AnonymiseFileNames |
| H2 | Gateway event observers | MessageLogger, RelationshipNotifier, VcNarrator, OnePingPerDM |
| H3 | Name decorations (message header, member row, profile badge row) | PlatformIndicators, UserVoiceShow, ShowTimeoutDuration, ShowMeYourName, pronouns |
| H4 | Accessories under a message | MessageLinkEmbeds, Translate, MessageLogger edit history |
| H5 | Hover bar and context menus | ViewRaw, ViewIcons, ReverseImageSearch, PermissionsViewer, Copy* |
| H6 | Composer buttons | SilentMessageToggle, SilentTyping, Translate, CharacterCounter |
| H7 | Above the server list | ReadAllNotificationsButton, ServerListIndicators |
| H8 | User panel (bottom left) | GameActivityToggle, AccountPanelServerProfile |
| H9 | Voice panel | CallTimer |

4. **One shared lazy fetch queue**, used by WhoReacted, MessageLinkEmbeds, ValidUser and ValidReply:
   - newest request first, duplicates dropped;
   - at least 250–300 ms between requests;
   - on 429, wait `retry_after`;
   - failures cached for the session;
   - reuses `api/ratelimit.rs`.

## 2. Sources and legend

- Class **A** = fits a native re-implementation. **B** = possible with the caveat noted. **C** = exclude.
- Settings shown as `key` = default come from the plugin sources at `src/plugins/<folder>/`.
  - Spot-checked by the editor: ircColors, translate, gameActivityToggle, platformIndicators, hideAttachments, crashHandler, alwaysExpandRoles, accountPanelServerProfile, silentMessageToggle.
  - The rest come from the researcher's source reads.

## 3. All 173 Vencord plugins

| Plugin | What it does | Area | Class | Note |
|---|---|---|---|---|
| AccountPanelServerProfile | Right-click account panel → server profile | User panel | A | S1 |
| AddAttachments | Add files while editing | Composer | B | PATCH must re-list kept attachment IDs or they are dropped |
| AlwaysAnimate | Animate avatars, emoji, banners and nameplates without hover | Appearance | A | GIF decode cost |
| AlwaysExpandRoles | Role list never collapsed | Profile | A | S2 (roles bug fix) |
| AlwaysTrust | Remove untrusted-link and file dialogs | Privacy | B | Offer a per-domain trust list instead |
| AnonymiseFileNames | Random or consistent upload names | Composer | A | |
| AutoDNDWhilePlaying | Status change when a game runs | Presence | B | Process detection plus automatic presence writes |
| BetterFolders | Open-folder servers in a second sidebar | Server list | A | S35 |
| BetterGifAltText | GIF alt text from tags or file name | Media | A | |
| BetterGifPicker | GIF picker opens on Favourites | Composer | A | |
| BetterRoleContext | Role menu: copy colour, view icon, edit | Profile | A | "Edit" opens server settings |
| BetterRoleDot | Role dot copies colour; dot and coloured names together | Profile | A | S3 |
| BetterSearch | Extra search options | Search | B | Undocumented search params |
| BetterSessions | Exact session times, names, new-session alerts | Security | B | Alerts need polling `/auth/sessions` |
| BetterSettings | Faster settings menu | — | C | React only |
| BetterUploadButton | Click opens file dialog, right-click opens menu | Composer | A | |
| BiggerStreamPreview | Larger stream preview | Call | A | |
| BlurNSFW | Blur media in NSFW channels | Media | A | |
| CallTimer | Timer in voice panel | Call | A | S40 |
| CharacterCounter | Composer character counter | Composer | A | S31 |
| ClearURLs | Strip tracking params on send | Composer | A | S29 |
| ClientTheme | Custom accent colour | Appearance | A | Belongs in the theme system |
| ColorSighted | Remove colour-blind status shapes | Appearance | A | |
| ConcatenatedComponentExtractor | Webpack dev tool (`_core`) | Dev | C | |
| ConsoleJanitor | Silence console | Dev | C | |
| ConsoleShortcuts | DevTools aliases | Dev | C | |
| CopyEmojiMarkdown | Copy `<:name:id>` | Chat | A | |
| CopyFileContents | Copy text attachment contents | Chat | A | |
| CopyUserURLs | "Copy User URL" | Menu | A | `https://discord.com/users/{id}` |
| CrashHandler | Recover React crashes | Dev | C | Rust panic handling is separate |
| CustomCommands | User-defined slash commands and tags | Composer | B | Only if it inserts text the user then sends |
| CustomIdle | Idle after N min; stay idle | Presence | A | S45 |
| CustomRPC | Fully custom Rich Presence | Presence | B | Can misrepresent apps |
| Dearrow | DeArrow YouTube titles | Media | C | Third-party API per embed |
| Decor | Decor avatar decorations | Cosmetics | C | Third-party OAuth and server |
| DevCompanion | VS Code companion | Dev | C | |
| DirectFileDownload | Direct downloads | Media | A | `rfd` already a dependency |
| DisableCallIdle | No DM-call idle kick / AFK move | Call | B | Defeats intended behaviour |
| DisableDeepLinks | Web → desktop redirect blocker | — | C | Web only |
| DontRoundMyTimestamps | Relative times floor | Chat | A | |
| Experiments | Discord experiments | Dev | C | |
| ExpressionManager | Clone/edit/delete emoji and stickers across servers | Server | B | Uploads; needs permission |
| F8Break | DevTools breakpoint | Dev | C | |
| FakeNitro | Fake Nitro emoji/stickers/quality | — | C | Misrepresents Nitro (COSMETICS_NOTES) |
| FakeProfileThemes | 3y3 profile colours | — | C | Misrepresents Nitro |
| FavoriteEmojiFirst | Favourites first in `:` autocomplete | Composer | A | S32 |
| FixCodeblockGap | CSS fix | — | C | |
| FixDiscordCss | CSS perf fix (`_core`) | — | C | |
| FixImagesQuality | Original-resolution images | Media | A | |
| FixSpotifyEmbeds | Spotify embed volume | — | C | iframe only |
| FixYoutubeEmbeds | Blocked YT embeds | — | C | Electron hack |
| ForceOwnerCrown | Owner crown in big servers | Members | A | `owner_id` |
| FriendInvites | Create/list/revoke friend invites | Social | B | Writes invites |
| FullSearchContext | Full context menu on search results | Search | A | |
| GameActivityToggle | Controller button toggles "show current game" | User panel | A | S10 |
| GifPaste | GIF picker inserts link | Composer | A | |
| GifProviderSwitcher | Tenor/Giphy instead of Klipy | Composer | B | Third-party keys |
| GreetStickerPicker | Choose greet sticker | Composer | B | Sends on user action only |
| HideMedia (`hideAttachments`) | Hide one message's media | Chat | A | |
| IgnoreActivities | Hide chosen own activities | Presence | A | |
| iLoveSpam | Show likely-spammer messages | Chat | A | |
| ImageFilename | File name tooltip | Media | A | |
| ImageLink | Keep image links visible | Chat | A | |
| ImageZoom | Magnifier lens | Media | A | S23 |
| ImplicitRelationships | Implicit friends in Friends tab | Social | B | Undocumented affinities (UNCONFIRMED) |
| IrcColors | Name colour from user-ID hash | Chat | A | §0 #10 |
| JumpToTop | Jump to first message | Chat | A | `GET messages?after=0` |
| KeepCurrentChannel | Reopen last channel | Navigation | A | |
| LoadingQuotes | Custom loading quotes | Appearance | A | |
| MemberCount | Online/total/voice counts | Members | A | S39 |
| MentionAvatars | Avatar or role icon in mentions | Chat | A | S14 |
| MessageClickActions | Double-click edit/reply; Backspace-click delete | Chat | B | Keep a delete confirmation |
| MessageLatency | Mark slow-sent messages | Chat | A | nonce time vs snowflake time |
| MessageLinkEmbeds | Previews of message links | Chat | A | S17 |
| MessageLogger | Keep deleted/edited messages | Chat | A* | S19; opt-in, memory only |
| MoreQuickReactions | More quick-react slots | Chat | A | |
| MusicRichPresence | Last.fm / ListenBrainz presence | Presence | C | Polls a third party |
| MutedServersActivityBadge | Activity badge on muted servers | Server list | A | |
| MutualGroupDMs | Mutual group DMs in profile | Profile | A | S6 |
| NewGuildSettings | Auto-settings on server join | Settings | B | Automated writes |
| NoBlockedMessages | Hide blocked messages fully | Chat | A | S20 |
| NoDevtoolsWarning | — | Dev | C | |
| NoF1 | — | — | C | |
| NoMaskedUrlPaste | URL paste over selection not masked | Composer | A | |
| NoMiddleClickPaste | Linux middle-click | — | C | |
| NoMosaic | No image mosaic | Media | A | |
| NoOnboardingDelay | Skip onboarding delay | Server | B | Circumvents server timing |
| NoPendingCount | Hide request/offer counts | Badges | A | |
| NoProfileThemes | Hide others' profile themes | Profile | A | |
| NoReplyMention | Replies don't ping by default | Composer | A | S26 |
| NoServerEmojis | No server emoji in autocomplete | Composer | A | |
| NoSystemBadge | No taskbar badge | Notifications | A | |
| NotificationVolume | App sound volume | Notifications | A | S44; built in: Notifications > Sounds > Volume |
| NoTrack | No analytics/Sentry (`_core`) | Privacy | A | Already true: never call `/science` |
| NoTypingAnimation | Static typing dots | Appearance | A | |
| NoUnblockToJump | Jump to blocked users' messages | Chat | A | |
| oneko | Cat follows mouse | Fun | A | Toy; skip |
| OnePingPerDM | One sound per unread DM | Notifications | A | S44 |
| OpenInApp | Spotify/Steam/Epic/Tidal URI schemes | Links | A | |
| OverrideForumDefaults | Default forum layout/sort | Forum | A | |
| PauseInvitesForever | Pause invites indefinitely | Server | B | PATCH Discord removed from UI |
| PermissionFreeWill | Remove permission-edit guards | Server | C | Can lock out admins |
| PermissionsViewer | Permissions of user/channel/role | Profile | A | S8 |
| petpet | `/petpet` GIF | Fun | B | Generates and uploads |
| PictureInPicture | PiP video | Media | A | egui viewport |
| PinDMs | Pinned DM categories | DMs | A | S36 |
| PlainFolderIcon | Folder icon without minis | Server list | A | |
| PlatformIndicators | Desktop/mobile/web/console/VR icons | Members/chat/profile | A | S4 |
| PreviewMessage | Preview before sending | Composer | A | |
| QuickMention | Mention button in hover bar | Chat | A | |
| QuickReply | Ctrl+↑/↓ reply, Ctrl+Shift+↑/↓ edit | Composer | A | |
| ReactErrorDecoder | — | Dev | C | |
| ReadAllNotificationsButton | "Read All" above server list | Server list | A | S37 |
| RelationshipNotifier | Unfriend/kick/group-removal notices | Notifications | A | S43 |
| ReplaceGoogleSearch | Pick search engine | Menu | A | Same as settings 6.37/6.38 |
| ReplyTimestamp | Time in reply preview | Chat | A | S15 |
| RevealAllSpoilers | Ctrl-click reveals all | Chat | A | |
| ReverseImageSearch | Image search engines | Media | A | S24 |
| ReviewDB | User reviews | Profile | C | Third-party account |
| RoleColorEverywhere | Role colours in more places | Chat | A | S13 |
| SecretRingToneEnabler (`secretRingTone`) | Alternative ringtone | Call | A | Needs the sound asset |
| SendTimestamps | Insert `<t:unix:style>` | Composer | A | |
| ServerInfo | Server info modal | Server | A | S38 |
| ServerListIndicators | Online friends / server count | Server list | A | S39 |
| Settings | Vencord settings UI (`_core`) | — | C | |
| ShikiCodeblocks | VS Code-style highlighting | Chat | A | `syntect` would be a heavy new dependency |
| ShowAllMessageButtons | Always full hover bar | Chat | A | |
| ShowConnections | Connection icons | Profile | A | S5 |
| ShowHiddenChannels | List channels you can't view | Sidebar | A | S33; display only |
| ShowHiddenThings | Mod-only items | Server | B | Some views call endpoints without permission |
| ShowMeYourName | Username next to nick | Chat | A | S12 |
| ShowTimeoutDuration | Remaining timeout | Chat | A | S22 |
| SilentMessageToggle | Silent-message toggle | Composer | A | S27 |
| SilentTyping | Don't send typing | Composer | A | S28 |
| SortFriendRequests | Requests by date | Social | A | relationship `since` |
| SpotifyControls | Spotify player | User panel | B | Spotify Web API with connection token |
| SpotifyCrack | Free listen-along | — | C | Circumvents Premium |
| SpotifyShareCommands | `/track` etc. | Composer | B | Sends a message |
| StartupTimings | — | Dev | C | |
| StickerPaste | Sticker picker inserts | Composer | A | |
| StreamerModeOnStream | Streamer mode while streaming | Privacy | A | |
| SuperReactionTweaks | Limit super-reaction animation; super by default | Chat | B | "By default" spends paid reactions |
| SupportHelper | Vencord support (`_core`) | — | C | |
| TextReplace | Find/replace rules on send | Composer | A | S30 |
| ThemeAttributes | CSS data attributes | — | C | |
| Translate | Translate messages | Chat | A manual / B auto | S21 |
| TypingIndicator | Typing in channel list | Sidebar | A | S34 |
| TypingTweaks | Avatars/colours in typing line | Composer | A | S34 |
| Unindent | Trim code-block indent | Chat | A | |
| UnlockedAvatarZoom | More avatar-crop zoom | Settings | A | Only with an avatar editor |
| UnsuppressEmbeds | Restore suppressed embeds | Chat | B | PATCH flags; own messages or Manage Messages |
| UserMessagesPronouns | Pronouns in header | Chat | A | `pronounsFormat` LOWERCASE/CAPITALIZED, `showSelf` true; cached profiles only |
| UserVoiceShow | Voice indicator | Profile/members | A | S9 |
| USRBG | Community banners | Cosmetics | C | Third-party server |
| ValidReply | Fix "could not be loaded" replies | Chat | A | Fetch via queue |
| ValidUser | Resolve `@unknown-user` | Chat | A | S18 |
| VcNarrator | TTS for voice events | Call | A | S42 |
| VencordToolbox | Titlebar button | — | C | |
| ViewIcons | Full-size avatars/icons | Profile | A | S7 |
| ViewRaw | Raw JSON | Dev/chat | A | S25 |
| VoiceChatDoubleClick (`vcDoubleClick`) | Double-click to join VC | Sidebar | A | |
| VoiceDownload | Download voice messages | Media | A | |
| VoiceMessages | Record/send voice messages | Composer | B | flag 8192 + `waveform` + `duration_secs`; official desktop doesn't offer it |
| VolumeBooster | >200 % volume | Call | A | S41 |
| WebContextMenus | — | — | C | Web |
| WebKeybinds | — | — | C | Web |
| WebPWA | — | — | C | Web |
| WebRichPresence (`arRPC.web`) | arRPC bridge | — | C | A native RPC server is a separate project |
| WebScreenShare | Screen-share options | — | C | Browser only |
| WebScreenShareFixes | Chromium bitrate cap | — | C | Browser only |
| WhoReacted | Reactor avatars on reactions | Chat | A | S16 |
| XSOverlay | VR notifications | Notifications | A (niche) | Local UDP; port UNCONFIRMED |
| YoutubeAdblock | AdGuard in YT embeds | — | C | iframe only |

## 4. Recommended native set: 45 specs

All settings are written `key` = default.

### Profile and user panel

**S1 AccountPanelServerProfile**
- Behaviour:
  - Right-clicking the account panel opens a menu: "View Server Profile" plus a checkbox "Prioritize Server Profile".
  - With the checkbox on, left-click opens the server profile and the menu item reads "View Account Profile".
  - In DMs the item is disabled.
- Setting: `prioritizeServerProfile` = false.
- Data: existing profile fetch with `guild_id`.

**S2 AlwaysExpandRoles** (no settings): the role list is never collapsed. **This is the fix for the screenshot bug.**
- Give the popout a fixed width: 300 px for the popout, about 600 px for the full profile modal.
- Lay roles out with `ui.horizontal_wrapped`.
- Each pill is a dot plus the name, with `Truncate` and a maximum width.
- Never let a pill take an unbounded column, which is what wrapped names one letter per line.

**S3 BetterRoleDot**
- Behaviour: clicking the role dot copies `#rrggbb` and shows a toast.
- Settings: `bothStyles` = false, `copyRoleColorInProfilePopout` = false.

**S4 PlatformIndicators**
- Settings: `list`, `badges`, `messages`, `colorMobileIndicator` (all true).
- Data:
  - Others: `client_status` map with keys `desktop`, `mobile`, `web`, `embedded`, `vr`.
  - Yourself: READY `sessions[]` plus SESSIONS_REPLACE (`client_info.client`, `status`). Skip `unknown`; online beats idle.
- Display:
  - Icon colour = status colour.
  - Tooltips: "Desktop", "Web", "Mobile", "Embedded (Console or Game)", "VR".
  - Not shown for bots.
- Change: `UserPresence.mobile: bool` becomes a small `[(Platform, Status)]`. The existing phone glyph becomes one case.

**S5 ShowConnections**
- Settings: `iconSize` = 32; `iconSpacing` = Cozy (Compact / Cozy / Roomy = 4 / 6 / 8 px).
- Behaviour:
  - Clicking opens the profile URL if there is one, otherwise copies the name and shows a toast.
  - Verified accounts get a tick in the tooltip.
  - Icons swap with the light/dark theme.
- URL templates (UNCONFIRMED): GitHub `github.com/{name}`, Twitch `twitch.tv/{name}`, YouTube `youtube.com/channel/{id}`, Steam `steamcommunity.com/profiles/{id}`, Reddit `reddit.com/u/{name}`, X `x.com/{name}`, Spotify `open.spotify.com/user/{id}`, TikTok `tiktok.com/@{name}`, Bluesky `bsky.app/profile/{id}`.

**S6 MutualGroupDMs** (no settings)
- A "Mutual Groups" tab: "N Mutual Groups" or "No Mutual Groups".
- Each row shows icon, name and member count; clicking opens the group.
- Data is local: type-3 channels whose recipients include the user. Skip bots and yourself.

**S7 ViewIcons**
- Settings: `format` = webp (webp / png / jpg; animated always webp); `imgSize` = 1024 (128–4096). "Original" uses `size=4096`.
- Menu items:
  - User: View Avatar / Server Avatar / Avatar Decoration.
  - Server: Icon / Banner.
  - Group DM: Icon.
- Profile avatars and banners become clickable. Every open (click and menu entries) goes through `viewer::open_icon`, the in-app lightbox (GIF avatars animate; "Open in browser" stays in its bottom bar).
- CDN paths (an `a_` prefix means animated):
  - `avatars/{uid}/{hash}`
  - `guilds/{gid}/users/{uid}/avatars/{hash}`
  - `banners/{uid}/{hash}`
  - `icons/{gid}/{hash}`
  - `banners/{gid}/{hash}`
  - `channel-icons/{cid}/{hash}`
  - `avatar-decoration-presets/{asset}.png`

**S8 PermissionsViewer**
- Setting: `permissionsSortOrder` = HighestRole (or LowestRole).
- Entry points:
  - a shield button by the roles section, opening a 500 px popout;
  - "View Permissions" in the user, channel and server menus.
- Content:
  - user → roles, plus an owner pseudo-role with all permissions;
  - channel → its overwrites;
  - server → all roles.
  - Each permission is shown as allowed / denied / unset.
- Data is local and reuses Rustcord's permission math.

**S9 UserVoiceShow**
- Settings: `showInUserProfileModal`, `showInMemberList`, `showInMessages` (all true).
- Icon priority: locked (no View Channel or Connect) > deafened > muted > speaker.
- Tooltip: "In Voice Chat", the channel, and up to 13 avatars.
- Clicks:
  - Single click: go to the channel after 250 ms.
  - Double click: join. Without Connect, show the toast "You cannot join the user's Voice Channel".
- Data: GUILD_CREATE voice states plus VOICE_STATE_UPDATE, for shared servers only.

**S10 GameActivityToggle**
- Settings: `oldIcon` = false; `location` = PANEL.
- Toggles **settings proto 11.3 `show_current_game` (BoolValue)** through `edit_settings`. When off, the controller gets a red slash.
- Right-click shows "Share Spotify Activity" when exactly one Spotify connection exists. That uses `PATCH /users/@me/connections/spotify/{id} {"show_activity":bool}` (the same route as Settings → Connections).
- The controller in the reference screenshot is the second-line *activity* icon, not this toggle. Keep the toggle optional.

**S11 FriendsSince** (removed from Vencord; reason UNCONFIRMED)
- "Friends since <date>" next to "Member since", from relationship `since` (READY, RELATIONSHIP_ADD).
- Not shown for pending requests.

### Message list

**S12 ShowMeYourName**
- Settings: `mode` = user-nick (nick-user / user / nick); `friendNicknames` = dms (always / fallback); `displayNames` = false; `inReplies` = false.
- Display:
  - The second name is dimmed, separated by a space, with no brackets.
  - If both names are equal ignoring case, show one.
  - Streamer mode masks the username as its first character followed by "…".

**S13 RoleColorEverywhere**
- Settings: `chatMentions`, `memberList`, `voiceUsers`, `reactorsList`, `pollResults` (all true); `colorChatMessages` = false; `messageSaturation` = 30 (0–100, step 10).
- Message tint = role colour mixed into the text colour in Oklab at N %. No tint on failed sends.
- Gradient roles (`ENHANCED_ROLE_COLORS`): one colour per glyph in the `LayoutJob`, using `theme/gradient.rs`.

**S14 MentionAvatars**
- Setting: `showAtSymbol` = true.
- User mentions: a 16 px round avatar (server avatar if set).
- Role mentions: the role icon (24 px webp) or a default glyph.
- Nothing for @everyone or @here. Cached users only; never fetch.

**S15 ReplyTimestamp** (no settings)
- `[time]` when the reply is on the same day, otherwise a date.
- Skipped when the referenced message is not loaded.

**S16 WhoReacted**
- Setting: `clickableAvatars` = true.
- Fetch: `GET /channels/{c}/messages/{m}/reactions/{emoji}?limit=100&type={0|1}` through the queue, newest first, 250 ms apart.
  - Each (message, emoji, type) is fetched once.
  - Skip messages with more than 10 reaction entries.
  - Only fetch for messages on screen.
  - Update locally on MESSAGE_REACTION_ADD / REMOVE.
- Display: up to 5 avatars at 90 % scale after the count. Blocked users filtered out.
- Backend: `FetchReactionUsers` exists.

**S17 MessageLinkEmbeds**
- Settings:
  - `messageBackgroundColor`
  - `automodEmbeds` = never (prefer / always)
  - `listMode` = blacklist (whitelist)
  - `idList`
  - a "clear cache" button
- Pattern: `https?://(\w+\.)?discord(app)?\.com/channels/(\d{17,20}|@me)/(\d{17,20})/(\d{17,20})`. Ignore links in `<…>`.
- Fetch: `GET /channels/{c}/messages?limit=1&around={m}`, 2 retries; the returned ID must match.
  - `FetchAround` with `limit: 1`.
  - Cache by channel:message, failures included.
  - At most 5 links per message, depth at most 2, no self-links.
  - Skip silently without View Channel.
- Display:
  - Rich: source header and the full message.
  - Compact: text plus images up to 400×300, or "[no content, N attachments]".

**S18 ValidUser** (no settings)
- Hovering an unknown `<@!?id>` calls `GET /users/{id}` through the queue.
- 300 ms apart; on 429 wait `retry_after` (default 1000 ms).
- Failed IDs are not retried this session.

**S19 MessageLogger** (opt-in)
- Settings:

| Key | Default |
|---|---|
| `deleteStyle` | text (or overlay) |
| `logDeletes` | true |
| `collapseDeleted` | false |
| `logEdits` | true |
| `logDeletedAttachments` | true |
| `inlineEdits` | true |
| `ignoreBots` | true |
| `ignoreSelf` | false |
| `ignoreUsers` / `ignoreChannels` / `ignoreGuilds` | "" (IDs; `ignoreChannels` also matches the parent category) |

- Behaviour:
  1. MESSAGE_DELETE / DELETE_BULK → mark `deleted` instead of removing. Remove anyway if ephemeral (flag 64) or ignored.
  2. MESSAGE_UPDATE that carries a changed `content` and `edited_timestamp` → push the old text and time to `edit_history`.
  3. Attachments missing after an edit stay, marked deleted.
  4. Reply previews keep the deleted content.
- Menus:
  - Message: "Toggle Deleted Highlight", "Clear Message History".
  - Channel, user and group DM: "Clear Message Log".
- Storage: memory only.
- Fix over Vencord: split ID lists on commas (Vencord matches substrings).
- Keep the decoded texture: CDN URLs expire. The URL's `ex` query param appears to be the hex unix expiry (UNCONFIRMED).

**S20 NoBlockedMessages**
- Settings: `ignoreMessages` = false; `applyToIgnoredUsers` = true.
- Removes the "N blocked messages" rows.
- With `ignoreMessages` on, MESSAGE_CREATE from relationship type 2 (or `user_ignored`) is dropped before the store and before unread counts.

**S21 Translate**
- Settings:

| Key | Default |
|---|---|
| `service` | google (deepl / deepl-pro / kagi) |
| `deeplApiKey` | "" |
| `kagiSession` | "" |
| `receivedInput` | auto |
| `receivedOutput` | en (Rustcord: system locale) |
| `sentInput` | auto |
| `sentOutput` | en |
| `autoTranslate` | false |
| `showAutoTranslateTooltip` | true |

- The context menu translates the selection, otherwise the whole message.
- Shown as an accessory "translated from X" with Dismiss.
- DeepL (user's own key): `POST https://api-free.deepl.com/v2/translate` (or `api.deepl.com`).
  - Header `Authorization: DeepL-Auth-Key …`; body `{"text":[…],"target_lang":"DE"}`.
  - Response: `translations[0].text` and `.detected_source_language`.
  - 456 = quota, 403 = bad key.
- Google: Vencord calls `translate-pa.googleapis.com/v1/translate` with a key embedded in its source (verified). **Do not copy the key.** The keyless `translate.googleapis.com/translate_a/single?client=gtx&sl=auto&tl=de&dt=t&q=` is unofficial (UNCONFIRMED stability and terms).
- Ship manual translation only (A); auto-translate is a separate opt-in (B). Text goes to a third party.

**S22 ShowTimeoutDuration**
- Setting: `displayStyle` = Inline (or Tooltip).
- Live countdown to `communication_disabled_until`.

**S23 ImageZoom**
- Settings: `saveZoomValues` true, `invertScroll` true, `nearestNeighbour` false, `square` false, `zoom` 2 (1–50), `size` 100 (50–1000), `zoomSpeed` 0.5 (0.1–5).
- Controls: hold left mouse for the lens; wheel zooms; Shift + wheel changes the size.
- egui:
  - Draw the texture again with a UV rect centred on the cursor.
  - A round lens = a textured triangle-fan `Mesh`.
  - Nearest-neighbour needs a second texture with `TextureOptions::NEAREST`.

**S24 ReverseImageSearch** (no settings): append the URL-encoded image URL and open it in the browser. "All" opens every engine.

| Engine | URL prefix |
|---|---|
| Google | `https://lens.google.com/uploadbyurl?url=` |
| Yandex | `https://yandex.com/images/search?rpt=imageview&url=` |
| SauceNAO | `https://saucenao.com/search.php?url=` |
| IQDB | `https://iqdb.org/?url=` |
| Bing | `https://www.bing.com/images/search?view=detailv2&iss=sbi&q=imgurl:` |
| TinEye | `https://www.tineye.com/search?url=` |
| ImgOps | `https://imgops.com/start?url=` |

**S25 ViewRaw**
- Settings: `clickMethod` = Left; `messageContextMenu` = false; `sortKeys` = true; `stripSensitiveData` = true (removes email, phone, storeCountry, personalConnectionId).
- Do not keep raw JSON per message. On click, refetch `around={m}&limit=1` and pretty-print with `serde_json`.

### Composer

**S26 NoReplyMention**
- Settings: `userList`, `roleList` (IDs); `shouldPingListed` = true; `inverseShiftReply` = false.
- Rule: ping = (author listed, or has a listed role) XOR (not `shouldPingListed`). Shift flips it.
- Sets the reply bar's initial `mention_author` → `replied_user`. Match IDs exactly.

**S27 SilentMessageToggle**
- Settings: `persistState` = false; `autoDisable` = true.
- Vencord prepends `@silent ` (verified). Rustcord sends `flags: 1<<12` instead: `send_body` needs a `flags` parameter (§0 #9).
- Whether the server itself parses "@silent" is UNCONFIRMED; the flag works either way.

**S28 SilentTyping**
- Settings: `showIcon` = false; `contextMenu` = true; `isEnabled` = true; `showTypingIn` (channel IDs that still send typing).
- Skips `POST /channels/{id}/typing` in `api/chat.rs`.

**S29 ClearURLs** (no settings; outgoing send/edit only)
- Bundle the ClearURLs `data.min.json` at build time.
- Fields used: `urlPattern`, `exceptions`, `rules` (query param names), `rawRules`.
- Needs `regex`: already in the lock (§0 #8). Rules licence UNCONFIRMED (LGPL reported).

**S30 TextReplace**
- Settings: `stringRules`, `regexRules`, each a list of `{name, find, replace, onlyIfIncludes}`.
- String rules run first, then regex rules. `\n` in `replace` becomes a newline. `/pattern/flags` syntax, default `g`. Empty `find` is skipped. Rule tester in settings.
- Ship string rules first; regex rules use `regex`.

**S31 CharacterCounter**
- Setting: `colorEffects` = true.
- Limit 2000, or 4000 when `premium_type` = 2.
- Colours: <50 % muted, <75 % yellow, <90 % orange, else red.
- Hidden when empty; shows "selected/total" with a selection.

**S32 FavoriteEmojiFirst** (no settings)
- Stable-sort favourites first in `:` autocomplete, then truncate.
- Favourites: `GET /users/@me/settings-proto/2` → field 5 → repeated string (§0 #5).

### Sidebar and server list

**S33 ShowHiddenChannels**
- Settings: `hideUnreads` = true; `showMode` = LockIcon (or HiddenIconWithMutedStyle); `defaultAllowedUsersAndRolesDropdownState` = true.
- Hidden = no View Channel (voice and stage: or no Connect). DMs are never hidden.
- Lock screen shows:
  - title, with an NSFW tag;
  - forum topic;
  - last message time (snowflake of `last_message_id`) and last pin;
  - slowmode and default thread slowmode;
  - bitrate, region ("Automatic" if null) and video quality;
  - archive duration, forum layout and sort;
  - default reaction, required tag and available tags;
  - allowed users and roles from the overwrites (missing members via op 8).
- **Never** fetch messages, subscribe with op 37, or join voice for hidden channels.
- Assumes user accounts get every channel in READY / GUILD_CREATE (UNCONFIRMED for every server type).

**S34 TypingIndicator + TypingTweaks**
- TypingIndicator settings: `includeCurrentChannel` true, `includeMutedChannels` false, `includeIgnoredUsers` false, `includeBlockedUsers` false, `indicatorMode` = dots + avatars.
- TypingIndicator display:
  - Up to 3 avatars on the row.
  - Tooltip: "A is typing…", "A and B are typing…", "A, B and C are typing…", "Several people are typing…".
  - Name order: nickname > global name > username.
- TypingTweaks settings: `showAvatars`, `showRoleColors`, `alternativeFormatting` (all true).
- TypingTweaks display: 16 px avatars, role colours and clickable names. With more than 3: "A, B and N others are typing…" (compute N correctly).
- Limit: server TYPING_START only arrives while op 37 is subscribed with `typing: true`, so this works for the current server and for DMs only.

**S35 BetterFolders**
- Settings: `sidebar` true, `sidebarAnim` true, `closeAllFolders` false, `closeAllHomeButton` false, `closeOthers` false, `closeServerFolder` false, `forceOpen` false, `keepIcons` false, `showFolderIcon` = Always.
- Servers of open folders move to a second 72 px column. Data: `guild_folders` (exists).

**S36 PinDMs**
- Settings: `pinOrder` = most recent (or Custom); `canCollapseDmSection` = false; `userBasedCategoryList` = {user ID → [{id, name, color, collapsed, channels[]}]}.
- Menus: Edit / Move Up / Move Down / Delete on the category; Pin / Unpin on the DM.
- Stored in local config, per account.

**S37 ReadAllNotificationsButton** (no settings)
- Acks every unread server channel, voice channel and joined active thread (no DMs).
- Reuses `MarkAllRead` (ack-bulk, batches of 100; maximum UNCONFIRMED). Respects the `send_read_acks` gate.

**S38 ServerInfo**
- Tabs: Server Info / Friends / Blocked / Ignored.
- Fields:
  - owner with avatar;
  - created (snowflake) and joined (`joined_at`);
  - vanity, locale;
  - verification level;
  - boosts and tier;
  - channel count (−1) and role count (−@everyone).
- Friends / Blocked / Ignored tabs: relationships in the server; missing ones fetched once with op 8 (≤100 IDs).

**S39 MemberCount + ServerListIndicators**
- MemberCount: `toolTip`, `memberList`, `voiceActivity` (all true).
  - Data from `online_count` / `member_count` (memstats).
  - Unsubscribed servers use GUILD_CREATE `member_count`.
- ServerListIndicators: `mode` = online friends (servers / both).
  - Online friends = relationship type 1 with a presence other than offline.

### Calls

**S40 CallTimer**
- Setting: `format` = stopwatch "30d 23:00:42" (or human "30d 23h 00m 42s").
- Starts on join; resets on channel change.

**S41 VolumeBooster**
- Setting: `multiplier` = 2 (1–5, step 0.5). Maximum = 200 % × multiplier.
- Linear gain plus a soft clipper in the mixer.
- When writing proto 16 (per-user volume), clamp to 200 and keep the real value locally.

**S42 VcNarrator**
- Settings:

| Key | Default |
|---|---|
| `voice` | system default |
| `volume` | 1 (0–1) |
| `rate` | 1 (0.1–10) |
| `sayOwnName` | false |
| `latinOnly` | false |
| `joinMessage` | `{{USER}} joined` |
| `leaveMessage` | `{{USER}} left` |
| `moveMessage` | `{{USER}} moved to {{CHANNEL}}` |
| `muteMessage` / `unmuteMessage` | mute / unmute text |
| `deafenMessage` / `undeafenMessage` | deafen / undeafen text |

  Placeholders: `{{USER}}`, `{{DISPLAY_NAME}}`, `{{NICKNAME}}`, `{{CHANNEL}}`. An empty template disables that event.
- Events: VOICE_STATE_UPDATE of others entering, leaving or moving through your channel. Skip stage channels and your own other sessions.
- TTS: SAPI `ISpVoice::Speak(…, SPF_ASYNC)` through `windows` feature **`Win32_Media_Speech`** (exists in 0.62.2; a feature flag, not a new crate).

### Notifications and presence

**S43 RelationshipNotifier**
- Settings: `notices` = false; `offlineRemovals` = true; `friends`, `friendRequestCancels`, `servers`, `groups` (all true).
- Events:
  - RELATIONSHIP_REMOVE type 1 → "X removed you as a friend"; type 3 → request cancelled.
  - GUILD_DELETE without `unavailable` → removed from the server.
  - CHANNEL_DELETE type 3 → removed from the group.
- Skip IDs of your own DELETEs.
- Offline check: save {friends, guilds, groups} to disk and compare on READY.
- Output: OS notification.

**S44 OnePingPerDM + NotificationVolume**
- OnePingPerDM: `channelToAffect` = both; `allowMentions` false; `allowEveryone` false. Sound only for the first unread message of a DM.
- NotificationVolume: `notificationVolume` = 100 (0–100), scales all app sounds.

**S45 CustomIdle**
- Settings: `idleTimeout` = 10 min (0–60, 0 = never); `remainInIdle` = true.
- Shows "Welcome back! Click the button to go online." with an "Exit idle" button.
- Idle detection: `GetLastInputInfo` (the `windows-sys` `Win32_UI_Input_KeyboardAndMouse` feature is enabled).
- Sends op 3 `{status:"idle", since, afk:true}`.

### Cheap extras (one-liners, class A)

DontRoundMyTimestamps, ForceOwnerCrown, RevealAllSpoilers, BlurNSFW, HideMedia, ImageFilename, NoMosaic, Unindent, CopyUserURLs, CopyEmojiMarkdown, CopyFileContents, NoPendingCount, MutedServersActivityBadge, NoServerEmojis, NoTypingAnimation, NoSystemBadge, NoProfileThemes, SendTimestamps, QuickReply, QuickMention, JumpToTop, KeepCurrentChannel, ShowAllMessageButtons, VoiceChatDoubleClick, VoiceDownload, DirectFileDownload, StreamerModeOnStream, AnonymiseFileNames, IrcColors, UserMessagesPronouns, ValidReply, SortFriendRequests, OpenInApp, NoTrack (already true).

### Illegalcord-inspired (built in, 2026-11-26)

Picked from Illegalcord's plugin list. Already in Rustcord: BetterFolders, RelationshipNotifier.

| Name | Kind | What it does | Files |
|---|---|---|---|
| CrashHandlerEnhanced | built in, always on | A panic writes `<config>/crash/crash-<time>.txt` (newest 5 kept); the next start shows a card with Copy report, Report on GitHub, Dismiss and, for a panic in `src/plugins/<file>.rs`, Turn them off | `src/crash.rs`, `src/ui/crash_card.rs`, `plugins::FILES` |
| ClientDiagnostics | plugin, off | Settings → Advanced: frames, slowest sections, memory, connection, Copy report. Times frames only while that section is on screen | `src/plugins/diagnostics.rs`, `src/ui/user_settings/diagnostics.rs`, `ui/frame_stats.rs` |

## 5. Equicord extras (205 in `src/equicordplugins`)

Names and descriptions come from Equibored `plugins.json`; the class is my judgement. Plugins not named are toys or overlap with Vencord.

**A – fits**

| Area | Plugins |
|---|---|
| Profile and members | AlwaysExpandProfiles, MoreUserTags (webhook / owner / admin / mod tags; exact list UNCONFIRMED), BetterActivities, BetterMutualServers, CopyProfileColors, CustomUserColors, ShowRolesInChat, NoRoleHeaders, FriendshipRanks, Streaks, VoiceStats, Timezones (manual only) |
| Channels | **CleanChannelName** (strips decoration; the real □□ fix is font fallback, FONTS_NOTES), ChannelBadges, UnreadCountBadge, ShowResourceChannels, CleanerChannelGroups, ChannelTabs, SidebarChat, CollapsibleUI, HomeTyping, MessagePeek, HideServers, ServerSearch, CustomFolderIcons |
| Chat | MarkdownTables, MessageColors, ToneIndicators, UrlHighlighter, RichMagnetLinks, NormalizeMessageLinks, DecodeBase64, MessageLinkTooltip, FindReply, JumpTo, PinIcon, ReactionTimestamps, BetterBackreading, MessageFetchTimer, ExportMessages, DownloadAllAttachments, FavoriteAnything, SaveFavoriteGIFs, GifCollections, Quoter, BetterAudioPlayer, MediaPlaybackSpeed, VoiceMessagesInBackground |
| Filtering and notifications | BlockKeywords, ClientSideBlock, ContentWarning, StickerBlocker, HideMessages, BetterBlockedUsers, SilenceUsers, ReplyPingControl, IgnoreCalls, KeywordNotify, MessageNotifier, PingNotifications, ToastNotifications, NotificationTitle, VoiceChannelLog |
| UX | CommandPalette, KeyboardNavigation, RecentDMSwitcher, FrequentQuickSwitcher, FriendTags, DraftNavigator, MiddleClickTweaks, CopyStatusUrls, CopyUserMention, BypassPinPrompt, InviteDefaults, BetterInvites, CancelFriendRequest, StatusPresets, CustomStatusTimeouts, HideChatButtons, Declutter, QuickThemeSwitcher, CustomSounds, CustomTimestamps, UnlimitedAccounts (= Rustcord multi-account), NoNitroUpsell (hide upsells only) |
| Composer (local transforms) | WriteUpperCase, SedEnhanced, BetterPlusReacts, FixFileExtensions |
| Voice | FullVCPFP, MicLoopbackTester, VCPanelSettings, DisableCameras, ToggleVideoBind |

**B – needs care**

| Plugin(s) | Why |
|---|---|
| BannersEverywhere, ShowBadgesInChat | Profile fetch per author; rate limits |
| ClickableRoles, InRole, LastActive | Member or search API calls |
| MessageLoggerEnhanced | Persists others' deleted messages |
| ProfileSets, DragFavoriteEmotes | Write profile / proto |
| FontLoader | Google Fonts fetch |
| Translate+ | Third-party translation |
| BetterCommands, MoreCommands | Send or create things |
| SplitLargeMessages, Signature, RepeatMessages | Alter or send messages |
| DuplicateCategory, FastDeleteChannels | Admin writes without confirmation |
| VoiceRejoin, InstantScreenshare | Automatic voice actions |
| RPCEditor, MusicControls, StatusWhileActive | Presence automation |
| GitHubRepos | API call per profile |
| LoginWithQR | Authorises devices |

**C – exclude**

| Reason | Plugins |
|---|---|
| Automation or mass actions | ScheduledMessages, MessageBurst, AtSomeone, FollowVoiceUser, WaitForSlot, RandomVoice, VoiceChatUtilities, Questify, NoPushToTalk |
| Nitro or limit bypass | LimitlessScreenshare, ClipsEnhancements |
| Third-party services | SteamStatusSync, GlobalBadges, UserPFP, TriviaAI, MessageTranslate, OrbolayBridge |
| Hiding content from Discord | InvisibleChat |
| Web/Electron/mod meta | SearchFix, StopAutoUnread, IconViewer, EquicordHelper, EquicordToolbox, WebpackTarball, ThemeLibrary, BlockKrisp |

## 6. Build order

1. **Plugin config and Settings → Plugins page.** S2 AlwaysExpandRoles ships with the popout roles fix.
2. **Data already present:** S16 WhoReacted, S37 ReadAll, S4 PlatformIndicators (keep the full `client_status`), S22, S5, S6, S39, S35, ForceOwnerCrown, S11.
3. **Send hook:**
   - S26 NoReplyMention;
   - S27 SilentMessageToggle (needs the `flags` parameter);
   - S28 SilentTyping;
   - S30 TextReplace (string rules);
   - S31 CharacterCounter.
4. **Lazy fetch queue:** S17, S18, ValidReply.
5. **Gateway observers:** S19, S43, S34, S9, S40, S42.
6. **Dependency approvals:**
   - `regex` (already in the lock): S29, regex rules of S30.
   - `syntect` (new and heavy): ShikiCodeblocks. Defer.

## 7. Still UNCONFIRMED

- Maximum `ack-bulk` batch size (UD gives none).
- Whether the server parses "@silent".
- Whether user accounts receive every server channel.
- Meaning of the `ex` param and the CDN expiry window.
- Connection URL templates.
- ClearURLs rules licence.
- Why FriendsSince was removed.
- MoreUserTags tag list.
- Stability of the keyless Google endpoint.
- XSOverlay port.
- `favorite_emojis` string format.
- The undocumented affinities behind ImplicitRelationships.

## Sources

- **Vencord:**
  - Plugin folder: https://api.github.com/repos/Vendicated/Vencord/contents/src/plugins and `/src/plugins/_core`
  - Plugin sources: `https://raw.githubusercontent.com/Vendicated/Vencord/main/src/plugins/<folder>/index.ts(x)`
  - Plugin list: https://vencord.dev/plugins
- **Equicord:** https://raw.githubusercontent.com/Equicord/Equibored/main/plugins.json (375 entries = 170 `plugins` + 205 `equicordplugins`)
- **Settings protos:** https://github.com/discord-userdoccers/discord-protos (`PreloadedUserSettings.proto`, `FrecencyUserSettings.proto`)
- **UD (userdoccers):** https://github.com/discord-userdoccers/discord-userdoccers, pages `resources/presence.mdx` (client_status), `resources/relationships.mdx` (`user_ignored`, `since`) and `topics/read-state.mdx` (ack-bulk)
- **Windows crate:** local `~/.cargo/registry/src/*/windows-0.62.2/Cargo.toml` (feature `Win32_Media_Speech`)
- **FriendsSince history:** https://code.lgbt/mirror/Vencord/commit/0a598ae9668547ac11e5a8dd1aee645b1916da2b

Not checked: the full source of most "cheap extras" and of most Equicord plugins; those rows rest on the one-line descriptions. Risk: S19 MessageLogger and B-class automation can upset other users or Discord; keep them opt-in, local-only and off by default.
