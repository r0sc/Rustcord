# Media design (in-app images, GIFs and videos)

Owner: **media** (`src/media/*`, `src/ui/viewer.rs`). Visual rules: `docs/DESIGN.md` §10.3 Media.
Protocol notes: `docs/PROTOCOL_NOTES.md` (embeds, "Played in the app").

## What it does
| Media | Behaviour |
|---|---|
| Image / GIF upload, `image` embed | Unchanged look; a click opens the **lightbox** (`Modal::Media`). The browser only through its "Open in browser" link. GIF uploads move like gifv (below). |
| Video upload (`video/mp4`, `video/quicktime`, `.mp4 .mov .m4v`) | Plays **inline** (poster, play disc, controls); Full screen = the same player in the lightbox. |
| gifv embed with a Discord-proxied `video.proxy_url` | MP4, muted, looping, no controls; autoplay rule below. Click opens the lightbox. |
| `video` embed with a Discord-proxied MP4 | Inline like an upload. |
| YouTube / Twitch / other `video` embeds without a Discord proxy | Unchanged: thumbnail card, click opens the browser. |
| Image or large thumbnail of a rich, article or link embed (proxied) | A click opens it alone in the lightbox. |
| WebM / Matroska (content type or extension) | The can't-play card at once, no request. |

Codecs: MP4/MOV with **H.264** (Baseline/Main/High, 8-bit 4:2:0, progressive) and **AAC-LC**
(HE-AAC plays its LC core; 5.1 / PCE and QuickTime `mp4a` v2 play muted). HEVC, VP8/9, AV1,
interlaced, 10-bit, > 1080p60 → card "Rustcord can't play this video format." / "This video is
too large to play here." with Open in browser + Download.

Not built: picture-in-picture, OS full screen, playback speed, captions, saved volume (session
only), channel-wide gallery (the lightbox steps through one message's media), the P2 items
(file chip Save, right-click menu on inline media, ViewIcons in the lightbox), Media Foundation
HEVC fallback.

## Crates (pinned, approved, pure Rust)
`re_mp4 =0.5.1` (demux: sample tables with sync flag, DTS/CTS; `mdat` skipped by seeking),
`symphonia-core` + `symphonia-codec-aac =0.6.1` (AAC-LC; MPL-2.0 unmodified). symphonia's own MP4
demuxer is not used (it ignores `ctts` and `stss`). H.264: the existing `openh264 =0.9.10` with
`Flush::NoFlush` (the crate default flushes after every decode and puts B-frames out of order).
Measured release delta with the probe: +372 KB.

## Engine (`src/media/`)
| File | Content |
|---|---|
| `mod.rs` | `Player` handle, worker thread (one per player, `rustcord-media`), `State`, `Status`, `Opts`, frame slots |
| `net.rs` | `init(Handle)`, `open(url)`, `RangeSource`, `fetch_all`, `save`, `file_name` |
| `mp4.rs` | pure helpers: `avcc_params`, `to_annexb`, `lc_asc`, `keyframe_for`, `audio_index_for`, `check_boxes`, `pick_tracks` |
| `audio.rs` | `AudioOut`: speaker (Voice & Video output, else default), F32, ring drained with `try_lock`, linear resample, volume in the callback, played-frames counter. Shared with `plugins/voice_messages.rs` and the app sounds (`src/sounds.rs`). |

```
UI (viewer.rs) ──Player::{play,pause,seek,set_volume,set_target,drawn}──► mpsc ──► worker thread
      ▲                                                       net::open: disk cache file, else RangeSource
      │ media::frames(ctx).latest(VideoKey::Media(key))       (block fetch: Handle::block_on; read-ahead: spawn_blocking)
      └──────── FrameWriter::write (wakes the UI) ◄────────── mp4::check_boxes → re_mp4 → pick_tracks
                                                              video: sample → Annex-B → openh264 → RGBA at the target size
                                                              audio: sample → AacDecoder → AudioOut ring ──► cpal
```
- `net::init` runs once in `backend::spawn` with the app's runtime (no new one) and the image
  HTTP client (`cache::images::http_client`). Same host allow-list as images; never an
  Authorization header.
- Frames: one media-own `VideoFrames` (`MAX_FRAME_SLOTS` caps players with frames), keyed by
  `VideoKey::Media(fnv1a64(cache_key(url)))`. RGBA straight at the display size (even, ≤ video
  size, ≤ 1920×1080): openh264's SIMD converter at the native size, else
  `voice::video::codec::i420_to_rgba` (shortcut: BT.601 limited; phone videos are BT.709).
- Safety: the worker runs in `catch_unwind` (re_mp4 parses untrusted files); before re_mp4 sees
  anything, `check_boxes` walks the top-level box headers and refuses header boxes over 32 MiB in
  total or boxes that overrun the file (re_mp4 allocates box contents up front). openh264 is C++
  (a crash there is not catchable; already true for calls).

### Worker loop
1. Open (disk cache or Range reads; offline mode: disk only, else `Failed(Network)`), parse, pick
   the first H.264 track and the first playable AAC track, decoders ready, decode and show the
   first picture, wait paused.
2. Playing: keep ≥ 0.4 s of sound queued; decode video until a picture comes out; its PTS is the
   smallest CTS of the samples fed (min-heap); wait (≤ 10 ms sleeps, checking the channel) until
   the clock reaches it, then publish. Pictures more than 2 frame durations late are decoded, not
   converted. Clock = audio master (`base + played device frames`); without sound (or after it
   ended) an `Instant` since the (re)start.
3. Seek(t): the video sample with the largest CTS ≤ t, back to its last sync sample
   (`keyframe_for`), a new openh264 decoder, pictures before t decoded and dropped; audio from
   the first sample still playing at t, decoder reset, ring cleared.
4. End: gifv loops (seek 0); a video becomes `Ended` (last frame stays, the disc shows Replay).
5. Visibility: the UI calls `drawn()` on every paint. Not drawn for 1 s (scrolled away, other
   channel, window minimized or in the tray) → pause, after a 1 s grace from a (re)start (a slow
   load can outlast the last paint). Exception `Opts::background` (quest videos the user
   started): it does not pause when not drawn; pictures not drawn for 1 s are decoded (H.264
   references) but neither converted to RGBA nor published, so no UI wakes at 30 fps and `pos`
   still advances (`shortcut:` audio-only playback with a keyframe resync when drawn again is not
   built). Paused or ended and not drawn for 30 s → the thread exits,
   decoders and blocks are freed, the frame slot is removed; the UI drops the handle and shows
   the poster again. A video paused mid-way that is still on screen asks for a repaint every
   20 s, so a still window keeps its frame and position. The speaker stream runs only while a video plays (cpal `pause` otherwise),
   so nothing runs while nothing plays (0 % idle CPU).
6. `played_ms` grows only while Playing **and** drawn in the last second (with
   `Opts::background`: while Playing, drawn or not), by the clock's advance
   (≤ real time); a seek adds nothing. `max_ms` = the furthest position reached by playing.

### `RangeSource` (net.rs)
`GET` with `Range: bytes=a-b` (1 MiB blocks, `Accept-Encoding: identity`). `206` +
`Content-Range` (total from the first answer); `200` = no Range support: the whole body if
≤ 32 MiB (in memory), else `CantStream`. `403/404/410` → `Gone`, anything else → `Network`
(Retry button; 429 is not retried on its own). ≤ 8 blocks in memory (LRU); passing half of block
k fetches k+1 and k+2 once in the background; a missing block waits for its in-flight fetch or is
fetched. A file ≤ 8 MiB whose blocks were all read is written to the image disk cache
(`cache_path(url)`, `config::write_atomic`; shares the 300 MB prune): gifv loops and revisits never
refetch, and it plays offline. Range support of `cdn.discordapp.com` / `images-ext-*` is
UNCONFIRMED; the 200 path covers a "no".
`save(url, path)`: streamed to `path.<ext>.part`, renamed when complete, ≤ 1 GiB, the disk cache
first. `fetch_all(url, 32 MiB)`: Copy image.

## For other areas (Quests: video quests)
Documented at the top of `src/media/mod.rs`:
- `Player::open(ctx, url, Opts { no_seek_ahead, output, background, .. })` (`background`: keeps
  playing and counting `played_ms` while not drawn, quest videos only); call `drawn()` and
  `set_target(w, h)` on every paint it is visible; `play`, `pause`, `seek(ms)` (clamped to
  `max_ms` with `no_seek_ahead`), `set_volume`.
- `status()`: `state` (Loading, Paused, Playing, Ended, Failed(why), Freed), `position_ms`,
  `duration_ms`, `played_ms` (the only number quest progress may come from), `max_ms`, `size`,
  `has_audio`, `buffering`. Frames: `media::frames(ctx).latest(&player.key())`.
- Or `ui::viewer::video_slot(ui, cx, url, size, no_seek_ahead)`: the whole player (controls, no
  full screen) in a modal-sized slot, returning its `Status` every paint.

## UI (`src/ui/viewer.rs`)
- Inline video: fitted into 420×320 (`MAX_MEDIA`), radius control; poster = the proxy's still
  (`attachment_preview_url`; `format=webp` on a video is UNCONFIRMED, a failure shows black; a
  file already on disk shows its own first frame). 48 px play disc (spinner while loading,
  Replay at the end), Download disc on hover, 40 px controls bar (play/pause, time, seek with
  ←/→ 5 s, volume with mute and a 64 px slider on hover, Full screen). One video plays at a time
  (starting one pauses the others and stops a voice message); at most 3 gifv players exist.
- gifv: plays in the lightbox, while hovered, or with "Play animated GIFs" on (reduced motion
  turns it off) while the window has focus (`gifv_plays`); else the poster with a "GIF" badge.
- Lightbox (`Modal::Media`, `ViewState::media_ui`): under the title bar; `media_scrim` backdrop;
  author row; Save, Copy image (images), Copy link, Close + "Esc"; arrows and ←/→ with wrap-around
  when there is more than one item; Space plays/pauses a video; images fitted (never upscaled),
  click toggles fit ↔ 1:1 at the click point, wheel zooms about the pointer up to 2× natural,
  drag pans without gaps (`zoom_about`, `clamp_pan`); a sharper texture ≤ 2560 px once zoomed
  (shortcut: one texture, no tiles). Bottom: "Open in browser" + `External` (the explicit way
  out), "2 of 3". Backdrop click or Esc closes. 140 ms fade + 0.97 → 1 scale, none with reduced
  motion. Save / Copy results come back through a channel as toasts.
- `media_items(msg, uploads, links, revealed)`: uploads (images and videos, revealed spoilers
  only, Settings > Chat "When uploaded") then media embeds (`image`, proxied `gifv` / `video`;
  "When posted as links"; not with SUPPRESS_EMBEDS).

## Budgets (this PC)
| | |
|---|---|
| Decode | 360p ≈ 0.5 ms/frame, 1080p ≈ 7 ms/frame (≈ 20–30 % of one core at 30 fps) |
| RAM per playing video | openh264 5 MB (480p) … 35 MB (1080p) + ≤ 8 MiB blocks + 2 RGBA buffers + 0.2 MB audio |
| Limits | 1 video playing, ≤ 3 gifv players, 1 zoom texture; everything freed 30 s after leaving the screen |

Measured 2026-11-14 (**debug** build: C: had too little room for a release rebuild; offline,
1180×760 at 125 %, pointer pinned off the window, the 5 s 1080p probe clip seeded as demo.mp4,
`RUSTCORD_MSG_DEMO=media,viewer-video,play`, lightbox frame 800×450 px):
| | Private | Working set | Threads | CPU |
|---|---|---|---|---|
| Idle, no media demo | 88 MB | 109 MB | 16 | 0 |
| Media demo on screen, nothing playing, poster decoder freed | 94 MB | 113 MB | 18 | 0 |
| 1080p clip playing | 113 MB peak | 139 MB | 24 | 26–60 % of one core (debug) |
| Ended, decoder thread gone (30 s after the last paint) | 102 MB | 136 MB | 17 | 0 between the app's own repaints |

Quest video in the background, **debug** build (2026-11-15, offline, 1180×760, `quests:bg` /
`quests:watch-play`: the 640×360 demo.mp4 looped, muted, `Opts::background`; process CPU over 20 s,
median of 3 runs, 1 run for the modal; % of one core):
| | CPU | Private | Working set |
|---|---|---|---|
| Nothing playing, window in front or minimized | 0.08 % | 90 MB | 107 MB |
| Playing, modal open in front (44 frames/s drawn) | 31 % (1 run) | 106 MB | 136 MB |
| Playing, no modal, Quests page in front (≈ 1.5 frames/s drawn) | 4.5 % | 101 MB | 127 MB |
| Playing, window minimized (only `logic` runs, pictures not converted) | 3.9 % | 101 MB | 123 MB |

The background rows are the decoder plus the 1 s tick; with the modal open the UI redraws at the
video's rate. 1080p in the background was not measured (its decode cost is in the table above).

Playback CPU, **release** (2026-11-14, offline, 1180×760 at 125 %, window in front, process CPU
over 20 s, median of 3 runs; per thread from the same runs; before this tuning: 39–44 % inline,
72 % for 1080p):
| | Total | GL driver thread (atio6axx.dll) | `rustcord-media` | UI thread |
|---|---|---|---|---|
| Inline demo.mp4 (360p, 30 fps) or gifv | 21–22 % | 100–125 ms/s | 35–50 ms/s | 40–60 ms/s |
| 1080p clip in the lightbox | 44–45 % | 120–145 ms/s | 235–255 ms/s (≈ 200 decode) | 55–70 ms/s |

The floor: every new video frame is one GL present, and AMD's OpenGL driver spends ~3.3–4 ms of CPU
per present on its own thread, whatever changed. At 30 fps that is ~100 ms/s (10 % of a core)
before any decode or UI work. Fewer presents would mean dropping video frames.

## Tests and test hooks
Tests: `media::mp4` (avcC → Annex-B on the fixture, `to_annexb`, `lc_asc`, `keyframe_for` with
B-frames, `audio_index_for`, the box budget, the fixture's tracks), `media::net`
(`parse_content_range`, block edges and read-ahead once, ≤ 8 blocks LRU, 200 bodies ≤ / > 32 MiB,
the disk cache write, error kinds, `file_name`), `media` (fixture decoded in display order, AAC
3.0 s, seek to 1.73 s from a keyframe, `played_ms` needs the player on screen, malformed files fail
without panicking), `ui::viewer` (item order and filters, gifv rules, zoom about the pointer, pan
clamp, ←/→ wrap and Esc in headless frames), `model` (`is_video`).
Fixtures (self-generated): `tests/fixtures/media/demo.png` (960×600), `demo.gif` (3 frames),
`demo.mp4` (Media Foundation, 640×360 H.264 High with B-frames + AAC-LC, 3 s, frame n has luma
16 + 2n in its top-left block); `python tests/fixtures/make_fixture.py media` seeds them into the
fixture home's disk cache. `RUSTCORD_MSG_DEMO=media[,viewer|viewer-zoom|viewer-video|player|play]`
(docs/ARCHITECTURE.md).

## Risks
1. HEVC iPhone videos and WebM don't play (card with Open in browser / Download).
2. Range support and video stills on Discord's hosts are UNCONFIRMED (fallbacks above).
3. Signed URLs (`ex`) of old messages expire → "This video is no longer available."
4. A chat video pauses when its window is minimized or hidden (0 % idle CPU rule); Discord keeps
   playing. A quest video the user started keeps playing (`Opts::background`) until it ends, the
   quest is done or the user stops it; that is the one case where media runs with nothing drawn.
5. RAM while a 1080p video plays is above the idle target until 30 s after it stops.
