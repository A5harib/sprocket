"""Export Blinked reel 02 "Name three" (the full 20.45 s reel) as a Sprocket graph scene.

It reads the geometry and timing from the reel's own build script (scripts/build_reel.py and
scripts/reel_timing.py) and the reel chrome text from v2-fullbleed.json, so the scene matches
the HyperFrames page frame for frame.

    python examples/blinked-02/export_reel02.py <reel dir> [out.json]

<reel dir> is soulscroll-content/reels/02-name-three. Render with
    sprocket render --scene examples/blinked-02/reel02.json --assets-dir <reel dir>/assets
"""
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REEL = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "..", "..", "..", "soulscroll-content", "reels", "02-name-three"))
OUT = sys.argv[2] if len(sys.argv) > 2 else os.path.join(HERE, "reel02.json")
sys.path.insert(0, os.path.join(REEL, "scripts"))

import build_reel as B  # noqa: E402
import reel_timing as T  # noqa: E402
from feed import FEED, FREEZE  # noqa: E402

S, G, P, N, W, H = B.S, B.G, B.P, B.N, B.W, B.H
TX, TY, CY = B.TX, B.TY, B.CY
HAIR, GAP = B.HAIR, B.GAP
INK, PAPER, ASH, TEAL, STAMP = B.INK, B.PAPER, B.ASH, B.TEAL, B.STAMP
LIME = "#D4F02B"
SLOTS = B.SLOTS


def tw(start, duration, to, ease="none", frm=None):
    t = {"start": round(start, 4), "duration": round(duration, 4), "to": round(to, 4), "ease": ease}
    if frm is not None:
        t["from"] = frm
    return t


def track(initial, *tweens):
    return {"initial": initial, "tweens": list(tweens)}


# Reel chrome text (handles, captions, avatar colours, meme text) as drawn by the feed variants.
with open(os.path.join(HERE, "v2-fullbleed.json"), encoding="utf-8") as fh:
    TILES = {t["id"]: t for t in json.load(fh)["feed"]["tiles"]}


def build():
    # --- the slip's geometry, as in build_reel.build() ----------------------------------
    y_end_rewind = B.col_y_centring(0)
    bar0 = B.screen_to_col(560, y_end_rewind)
    first, y = [], bar0
    for _ in range(N):
        first.append(y)
        y += HAIR + GAP
    rest, rest_h = B.bars()
    slip_top = y
    slip_end = slip_top + rest_h
    mark_y = slip_end + 110 / S
    mark_h = 92 / S
    tear_y = mark_y + mark_h + 70 / S
    end_top = tear_y + 1150 / S
    paper_top = B.reel_top(0) - 2400
    paper_left, paper_w = -TX / S - 400, W / S + 800
    y_run_end = (CY - 60 - TY) / S - (mark_y + mark_h / 2)
    reveal_px = B.screen_to_col(H + 40, y_end_rewind) - slip_top

    def bar_y(n):
        return first[n] if n < N else slip_top + rest[n - N][0]

    clocks = []
    for label, minute in [("23:40", 0), ("00:00", 20), ("00:30", 50), ("01:00", 80), ("01:30", 110), ("02:00", 140), ("02:14", 154)]:
        n = min(B.NBARS - 1, round(minute / B.MINUTES * (B.NBARS - 1)))
        clocks.append((label, bar_y(n)))

    PB, PD = T.PULL[0], T.PULL[1] - T.PULL[0]
    C0, CD = T.COLLAPSE[0], T.COLLAPSE[1] - T.COLLAPSE[0]
    Z0, ZD = T.ZOOM[0], T.ZOOM[1] - T.ZOOM[0]
    first_rel = [first[k] - k * P for k in range(N)]

    # --- a reel in the column -------------------------------------------------------------
    def reel(k, i, video=True, end_card=False):
        cid, _slug, _off, crop, _overlay, _sound = FEED[i]
        tile = dict(TILES[cid])
        tile["slot_k"] = k
        still = f"stills/{cid}-0.jpg" if end_card else f"stills/{cid}_d.jpg"
        media_h = 870 if crop == "bottom" else 1740
        media = [{"type": "image", "src": still, "fit": "cover", "width": 1080, "height": media_h}]
        if video:
            s0, d = SLOTS[i]
            vis = FREEZE + 0.3 if i == N - 1 else s0 + d + 0.25
            media.append({"type": "video", "src": f"clips/{cid}.mp4", "start": 0, "fit": "cover",
                          "width": 1080, "height": media_h, "show": [0, round(vis, 3)]})
        if crop == "bottom":
            media = [{"type": "group", "y": 870, "width": 1080, "height": 870, "clip": True, "children": media}]
        children = [{"type": "rect", "width": 1080, "height": 1740, "color": "#000000"}] + media
        children.append({"type": "ig_reel", "width": 1080, "height": 1740, "tile": tile})
        node = {"id": "end" if end_card else f"r{k}", "type": "group", "y": round(end_top, 1) if end_card else k * P,
                "width": 1080, "height": 1740, "origin": [0.5, 0], "clip": True, "children": children}
        if end_card:
            return node
        cover_at = C0 + CD * 0.55 + k * 0.012
        children.append({"id": f"cov{k}", "type": "rect", "width": 1080, "height": 1740,
                         "color": TEAL if cid == "dog" else INK,
                         "props": {"opacity": track(0, tw(cover_at, CD * 0.35, 1, "power2_in")) if k < N else 0}})
        ys = [tw(PB, PD, (k - 15) * G, "power2_in_out")]
        props = {}
        if k < N:
            c = C0 + k * 0.012
            ys.append(tw(c, CD, first_rel[k], "power3_in_out"))
            props["scale_y"] = track(1, tw(c, CD, HAIR / P, "power3_in_out"))
        else:
            props["opacity"] = track(1, tw(PB, PD * 0.5, 0, "power2_in"))
        props["y"] = track(0, *ys)
        node["props"] = props
        return node

    reels = [reel(k, k) for k in range(N)] + [reel(N, 0, video=False)]

    # --- paper, torn edge and stub -----------------------------------------------------------
    paper_h = tear_y - paper_top
    stub_top = tear_y - 46
    stub_h = max(2600 / S, B.reel_top(N) + P + 3000 - stub_top)
    tile_px = 512 / S
    fade_in = track(0, tw(PB + 0.15, PD * 0.6, 1, "power2_out"))
    teeth = 24 * 6
    step = paper_w / teeth
    zig = [[0, 46]] + [[round(j * step, 2), 0 if j % 2 == 0 else 46] for j in range(teeth + 1)] + [[round(paper_w, 2), 46]]
    paper = {"id": "paper", "type": "group", "x": round(paper_left, 1), "y": round(paper_top, 1),
             "width": round(paper_w, 1), "height": round(paper_h, 1), "props": {"opacity": fade_in},
             "children": [
                 {"type": "rect", "width": round(paper_w, 1), "height": round(paper_h, 1), "color": PAPER},
                 {"type": "image", "src": "paper.png", "fit": "tile", "tile": [tile_px, tile_px],
                  "width": round(paper_w, 1), "height": round(paper_h, 1)},
                 # the teeth: CSS clips the paper with a zigzag polygon; the ink under it shows through
                 {"id": "teeth", "type": "polygon", "y": round(paper_h - 46, 1), "width": round(paper_w, 1), "height": 46,
                  "points": zig, "color": INK},
             ]}
    stub = {"id": "stub", "type": "group", "x": round(paper_left, 1), "y": round(stub_top, 1),
            "width": round(paper_w, 1), "height": round(stub_h, 1),
            "props": {"opacity": track(0, tw(PB + 0.15, PD * 0.6, 1, "power2_out"), tw(T.TEAR_AT, 0.28, 0, "power3_in")),
                      "y": track(0, tw(T.TEAR_AT, 0.28, 320 / S, "power3_in"))},
            "children": [
                {"type": "rect", "width": round(paper_w, 1), "height": round(stub_h, 1), "color": PAPER},
                {"type": "image", "src": "paper.png", "fit": "tile", "tile": [tile_px, tile_px],
                 "tile_offset": [0, round(stub_top - paper_top, 1)], "width": round(paper_w, 1), "height": round(stub_h, 1)},
            ]}

    # --- time labels, clock times, the barcode, the wordmark ---------------------------------
    label_right = B.LABEL_RIGHT
    labels = []
    for k, (_s0, d) in enumerate(SLOTS):
        at = PB + PD - 0.25 + abs(15 - k) * 0.03
        labels.append({"id": f"lab{k}", "type": "text", "text": f"{d:.2f}s",
                       "x": round(label_right - 1400, 1), "y": round(B.reel_top(k) + P / 2 - 75, 1), "width": 1400,
                       "align": "right", "font": "IBM Plex Mono", "weight": 600, "size": 125, "line_height": 1.3,
                       "letter_spacing": -0.01, "color": TEAL if FEED[k][0] == "dog" else INK,
                       "props": {"opacity": track(0, tw(at, 0.25, 1, "expo_out", 0), tw(C0, 0.15, 0, "power2_in")),
                                 "x": track(0, tw(at, 0.25, 0, "expo_out", -30))}})
    clock_nodes = []
    for j, (lab, cy) in enumerate(clocks):
        op = track(0, tw(C0 + CD - 0.1, 0.2, 1, "power2_out", 0)) if j == 0 else track(0, tw(T.RUN[0], 0, 1))
        clock_nodes.append({"id": f"clk{j}", "type": "text", "text": lab,
                            "x": round(label_right - 1400, 1), "y": round(cy - 70, 1), "width": 1400, "align": "right",
                            "font": "IBM Plex Mono", "weight": 400, "size": 119, "line_height": 1.3, "color": ASH,
                            "props": {"opacity": op}})
    slip = {"id": "slip", "type": "rects", "y": round(slip_top, 1), "width": 1080, "height": round(rest_h, 1),
            "rects": [[0, round(by, 2), 1080, round(bh, 2), c] for by, bh, c in rest],
            "props": {"inset_bottom": track(1, tw(T.REVEAL[0], T.REVEAL[1] - T.REVEAL[0], 1 - reveal_px / rest_h),
                                            tw(T.REVEAL[1] + 0.02, 0, 0))}}
    mark = {"id": "mark", "type": "text", "text": "BLINKED", "x": 0, "y": round(mark_y, 1), "width": 1080,
            "height": round(mark_h, 1), "align": "center", "font": "Big Shoulders Display", "weight": 800,
            "size": round(mark_h, 2), "line_height": 1.0, "letter_spacing": 0.02, "color": INK,
            "tail_square": {"color": STAMP, "size": 0.17, "gap": 0.06},
            "props": {"inset_bottom": track(1, tw(T.MARK_AT, 0.3, 0))}}
    end_card = reel(0, 0, video=False, end_card=True)

    # --- the camera and the column -----------------------------------------------------------
    col_y = []
    for i, (s, d) in enumerate(SLOTS):
        if i:
            col_y.append(tw(s, min(0.16, d * 0.55), -i * P, "power3_out"))
    col_y.append(tw(FREEZE - 0.07, 0.07, B.FREEZE_Y, "power2_in"))
    rew, t = [], T.REWIND_START
    while t < T.REWIND_END + 1 / T.FPS:
        rew.append((round(t, 4), B.col_y_centring(T.rewind_index(t))))
        t += 1 / T.FPS
    for (t0, _), (t1, y1) in zip(rew, rew[1:]):
        col_y.append(tw(t0, t1 - t0, y1))
    col_y.append(tw(T.RUN[0], T.RUN[1] - T.RUN[0], y_run_end, "power2_in_out"))
    col_y.append(tw(Z0, ZD, -end_top, "power3_in_out"))

    col = {"id": "col", "type": "group", "origin": [0, 0], "width": 1080, "props": {"y": track(0, *col_y)},
           "children": [paper, stub] + reels + labels + [slip] + clock_nodes + [mark, end_card]}
    cam = {"id": "cam", "type": "group", "origin": [0, 0], "width": 1080, "height": 1920,
           "props": {"x": track(0, tw(PB, PD, TX, "power2_in_out"), tw(Z0, ZD, 0, "power3_in_out")),
                     "y": track(0, tw(PB, PD, TY, "power2_in_out"), tw(Z0, ZD, 0, "power3_in_out")),
                     "scale": track(1, tw(PB, PD, S, "power2_in_out"), tw(Z0, ZD, 1, "power3_in_out"))},
           "children": [col]}

    # --- fixed chrome, grade, the line and the captions ------------------------------------
    chrome = {"id": "igfixed", "type": "ig_fixed", "nav_top": 1740, "profile_poster": "posters/coffee.jpg",
              "props": {"opacity": track(1, tw(PB, 0.3, 0, "power2_in"), tw(Z0 + ZD - 0.22, 0.22, 1, "power2_out"))}}
    grade = {"kind": "grade", "dim_color": INK,
             "desaturate": track(0, tw(FREEZE + 0.02, 0.12, 1), tw(PB, PD * 0.8, 0, "power2_in_out")),
             "dim": track(0, tw(FREEZE + 0.06, 0.2, 0.74, "power2_out"), tw(PB, PD * 0.8, 0, "power2_in_out"))}
    line = {"id": "line", "type": "text", "text": "Name three", "x": 0, "y": 1072, "width": 1080, "height": 196 * 0.9,
            "align": "center", "font": "Big Shoulders Display", "weight": 800, "size": 196, "line_height": 0.9,
            "letter_spacing": 0.005, "color": PAPER, "tail_square": {"color": STAMP, "size": 0.17, "gap": 0.06},
            "props": {"opacity": track(0, tw(T.NAME3_AT - 0.04, 0.12, 1, "power3_out", 0), tw(T.LINE_OUT, 0.2, 0, "power2_in")),
                      "scale": track(1.08, tw(T.NAME3_AT - 0.04, 0.12, 1, "power3_out", 1.08)),
                      "blur": track(0, tw(T.LINE_OUT, 0.2, 8, "power2_in"))}}

    def caption(j, word, a, b, box):
        return dict(box, **{
            "id": f"cap{j}", "type": "text", "text": word, "font": "IBM Plex Sans", "weight": 500, "size": 66,
            "line_height": 1.1, "color": INK, "background": {"color": LIME, "padding": [6, 24, 10, 24]},
            "props": {"opacity": track(0, tw(a, 0.13, 1, "power2_out", 0), tw(b, 0.15, 0, "power2_in")),
                      "blur": track(6, tw(a, 0.13, 0, "power2_out", 6), tw(b, 0.15, 6, "power2_in"))}})

    (w0, a0, b0), (w1, a1, b1) = T.CAPTIONS
    caps = [caption(0, w0, a0, b0, {"x": 0, "y": 1222, "width": 1080, "align": "center"}),
            caption(1, w1, a1, b1, {"x": 0, "y": CY + 62, "width": TX - 55, "align": "right"})]

    return {
        "id": "reel02-name-three",
        "width": W, "height": H, "fps": T.FPS, "duration": T.END,
        "audio": "mix-reel.wav",
        "background": INK,
        "motion_blur": {"enabled": True, "shutter_angle": 60, "samples": 15},
        "layers": [
            {"kind": "nodes", "id": "world", "motion_blur": {"mode": "track", "node": "col"}, "nodes": [cam]},
            {"kind": "nodes", "id": "chrome", "nodes": [chrome]},
            grade,
            {"kind": "nodes", "id": "top", "nodes": [line] + caps},
        ],
    }


if __name__ == "__main__":
    scene = build()
    with open(OUT, "w", encoding="utf-8") as fh:
        json.dump(scene, fh, ensure_ascii=False, separators=(",", ":"))
    print("wrote", OUT)
