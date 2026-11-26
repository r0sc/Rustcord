"""Retakes the README screenshots (docs/screenshots/) from the offline test fixture, never a real account.

On a scratch copy of tests/fixtures/home: the signed-in demo user is shown as "rosc" (with --avatar,
wearing that picture, which is not stored in the repository), the other demo users get the freely
licensed avatars in docs/screenshot-assets/people, the test messages become a normal chat, and the
shared picture, my local banner and the live stream show a Red Eclipse frame (README "Credits").
Windows; the window is 1280x800 logical, so the PNGs are 1600x1000 at 125 % display scaling.

  python -I scripts/readme-shots.py --exe C:/rc-target/debug/rustcord.exe [--avatar pic.png] [--only main.png,stream.png]
"""
import argparse, json, os, shutil, subprocess, sys, tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FIXTURE = os.path.join(ROOT, "tests", "fixtures", "home")
ASSETS = os.path.join(ROOT, "docs", "screenshot-assets")
OUT = os.path.join(ROOT, "docs", "screenshots")
FRAME = os.path.join(ASSETS, "red-eclipse.jpg")
GENERAL, LOUNGE = "1565443437363200010", "1565443437363200014"
ME, ALICE, SHOWCASE = "1565443437363200001", "1565443437363200002", "1565443437363200012"
AVATARS = {"alice": "alice.png", "bobby": "bobby.png", "cara.k": "cara.png", "dan_ops": "dan.png"}
SHARED_IMAGE = "https://cdn.discordapp.com/attachments/1/1/demo.png"  # RUSTCORD_MSG_DEMO=media's picture
SHOTS = [  # (file, theme, env); every run also gets status:online and the composer without its Offline chip
    ("main.png", None, {"RUSTCORD_OPEN": "members"}),
    ("stream.png", None, {"RUSTCORD_OPEN": f"voice:{LOUNGE},channel:{LOUNGE}",
                          "RUSTCORD_CALL_DEMO": f"people,stream:{FRAME},focus", "RUSTCORD_SCREENSHOT_FRAME": "90"}),
    ("profile.png", None, {"RUSTCORD_OPEN": "members,profile"}),
    ("plugins.png", None, {"RUSTCORD_OPEN": "settings:plugins", "RUSTCORD_PLUGINS": "ShowHiddenChannels,PlatformIndicators,ImageZoom,CopyUserURLs"}),
    ("viewer.png", None, {"RUSTCORD_OPEN": f"channel:{GENERAL}", "RUSTCORD_MSG_DEMO": "media,viewer"}),
    ("light.png", "Lilac", {"RUSTCORD_OPEN": "members"}),
]
# the fixture repeats 11 test messages three times; this is the same chat as a normal conversation
CHAT = [
    "morning! who's on tonight?", "me, after I finish this PR", "tests are green, merging",
    "> merging\nfinally", "gg last night btw", f"<@{ME}> you carried, not gonna lie",
    "server's back up, restarted the bot", "thanks dan", "who took my spot in the lounge", "it was cara lol", "brb dinner",
    "anyone tried the new async closures?", "yes, `tokio::spawn` code is *so* much nicer now",
    "```rust\nlet double = async |x: u32| x * 2;\n```", "> so much nicer\nfewer boxed futures too", "agreed",
    f"<@{ME}> did you finish the client?", "what client?", "his own chat client, written in rust", "no way",
    "show us", "tomorrow, promise",
    "anyone up for a few rounds tonight?", "after 9, still fighting the borrow checker",
    "the fix was one line:\n```rust\nlet name = user.name.clone();\n```", "> fighting the borrow checker\nclassic, just clone it",
    "btw I'm on Rustcord now, my own client. it barely touches my RAM",
    f"<@{ME}> wait, the one you built? post it in <#{SHOWCASE}>", "does it do voice?",
    "voice, video and streams. we used it last night",
    "# Rustcord\n- voice, video & streams\n- 51 built-in plugins\n-# Windows · Linux · macOS",
    "ok that's sick, downloading now", f"let me know what you think <@{ALICE}>!",
]


def fnv1a64(data):  # as src/cache/images.rs
    h = 0xCBF29CE484222325
    for b in data:
        h = ((h ^ b) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def seed(img_dir, url, path):
    shutil.copyfile(path, os.path.join(img_dir, "%016x" % fnv1a64(url.encode())))


def make_home(dest, theme, avatar):
    shutil.copytree(FIXTURE, dest)
    snap_path = os.path.join(dest, "cache", "snapshot.json")
    img_dir = os.path.join(dest, "cache", "images")
    os.makedirs(img_dir, exist_ok=True)
    snap = json.load(open(snap_path, encoding="utf-8"))
    me = snap["me"]
    people = os.path.join(ASSETS, "people")
    pics = {u["id"]: os.path.join(people, AVATARS[u["username"]]) for u in snap["users"] if u["username"] in AVATARS}
    if avatar:
        pics[me["id"]] = avatar
    hashes = {uid: f"0{uid[-3:]}0demo0avatar" for uid in pics}

    def patch(node):  # every embedded copy of a user: users, me, message authors, mentions, members
        if isinstance(node, dict):
            uid = node.get("id")
            if "username" in node:
                if uid == me["id"]:
                    node["username"] = node["global_name"] = "rosc"
                if uid in hashes:
                    node["avatar"] = hashes[uid]
            for v in node.values():
                patch(v)
        elif isinstance(node, list):
            for v in node:
                patch(v)
    patch(snap)
    for _, msgs in snap["recent_messages"]:
        for i, m in enumerate(msgs[:len(CHAT)]):
            m["content"], m["embeds"] = CHAT[i], []
    for uid, path in pics.items():
        for size in (16, 32, 64, 128, 256, 512, 1024, 2048, 4096):
            seed(img_dir, f"https://cdn.discordapp.com/avatars/{uid}/{hashes[uid]}.webp?size={size}", path)
    seed(img_dir, SHARED_IMAGE, FRAME)  # the image cache guesses the format, so a JPEG works
    json.dump(snap, open(snap_path, "w", encoding="utf-8"))
    config = os.path.join(dest, "config")
    overrides = os.path.join(config, "overrides")  # Settings -> Local profile: my banner, only on this device
    os.makedirs(overrides, exist_ok=True)
    shutil.copyfile(FRAME, os.path.join(overrides, "banner.jpg"))
    open(os.path.join(overrides, "overrides.toml"), "w").write('banner = "banner.jpg"\n')
    if theme:
        open(os.path.join(config, "settings.toml"), "w").write(f'theme = "{theme}"\n')


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", required=True)
    ap.add_argument("--avatar", help="rosc's picture (kept out of the repository)")
    ap.add_argument("--only", help="comma list of file names")
    a = ap.parse_args()
    assert fnv1a64(b"a") == 0xAF63DC4C8601EC8C
    assert "," not in FRAME, "RUSTCORD_CALL_DEMO is a comma list: move the checkout to a path without commas"
    tmp = tempfile.mkdtemp(prefix="rc-readme-shots-")
    exe = os.path.join(tmp, "rustcord.exe")
    shutil.copy2(a.exe, exe)  # run a copy: never lock the shared build output
    missing = []
    try:
        for name, theme, extra in SHOTS:
            if a.only and name not in a.only.split(","):
                continue
            home = os.path.join(tmp, name + ".home")
            make_home(home, theme, a.avatar and os.path.abspath(a.avatar))
            out = os.path.join(OUT, name)
            if os.path.exists(out):
                os.remove(out)
            join = lambda key, more: ",".join(filter(None, [extra.get(key), more]))
            env = dict(os.environ, RUSTCORD_HOME=home, RUSTCORD_OFFLINE="1", RUSTCORD_WINDOW_SIZE="1280x800",
                       RUSTCORD_POINTER="2000x2000", RUSTCORD_SCREENSHOT=out, **extra)
            env.update(RUSTCORD_OPEN=join("RUSTCORD_OPEN", "status:online"), RUSTCORD_MSG_DEMO=join("RUSTCORD_MSG_DEMO", "online"))
            r = subprocess.run([exe], env=env, timeout=90)
            ok = os.path.exists(out) and os.path.getsize(out) > 0
            print(name, "exit", r.returncode, os.path.getsize(out) if ok else "MISSING")
            if not ok:
                missing.append(name)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    return 1 if missing else 0


if __name__ == "__main__":
    sys.exit(main())
