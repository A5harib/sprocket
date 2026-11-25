Sprocket graph scenes

A graph scene is any JSON file with a top-level "layers" array. Sprocket renders it with the
graph renderer; files without "layers" use the original feed format (`sprocket schema`).

Top level
  id, width, height, fps, duration   as in the feed format
  audio: "mix.wav"                   relative to --assets-dir, muxed into the MP4
  background: "#111110"              canvas colour under every layer
  motion_blur: { enabled, shutter_angle (deg, default 60), samples (default 15) }
  fonts: ["fonts/Extra.ttf"]         extra font files; everything in <assets>/fonts loads anyway
  layers: [ ... ]                    painted in order

Layers
  { "kind": "nodes", "id": "world", "motion_blur": <blur>, "nodes": [ <node>, ... ] }
  { "kind": "grade", "desaturate": <track>, "dim": <track>, "dim_color": "#111110" }
      A grade layer changes everything painted below it (CSS saturation blend with grey, then
      a flat dim colour at the given opacity).

  Motion blur per nodes layer:
    { "mode": "none" }                         default
    { "mode": "track", "node": "<node id>" }   blur along that node's screen motion (camera or
                                               scrolling column): translation, scale, rotation
    { "mode": "accumulate", "samples": 8 }     average sub-frames across the shutter; exact for
                                               every node, costs `samples` times the draws

Node (every type)
  id            optional name (needed for motion-blur tracking)
  type          group | rect | ellipse | polygon | rects | svg | image | video | text | ig_reel | ig_fixed
  x, y          box position in the parent's space (CSS left / top)
  width, height box size
  origin        transform origin as fractions of the box, default [0.5, 0.5]
  props         keyframed values (all optional):
                  x, y           translation added to the box position (GSAP x / y)
                  scale, scale_x, scale_y, rotation (degrees, clockwise)
                  opacity        multiplies down the tree
                  inset_top, inset_right, inset_bottom, inset_left
                                 clip-path inset as fractions of the box (0..1)
                  blur           CSS filter blur, screen px (textured nodes)
                  width, height  animated box size
  clip          true = clip children to the box (overflow hidden), rounded by corner_radius
  corner_radius rounded corners for rect, image, video and clips
  show          [start, end] seconds; the node and its children draw only inside it
  raster_scale  fixed raster resolution for text / shapes / svg; default is the largest
                on-screen scale the node reaches (capped at 2)
  children      nested nodes, painted after the parent

Track (animated number)
  5                                              constant
  { "initial": 0, "tweens": [ { "start": 1.0, "duration": 0.4, "to": 1, "from": 0, "ease": "power2_out" } ] }
  { "keys": [[0, 0], [0.5, 1, "back_out"], [1.0, 0.2]] }   each key eases from the one before
  Tweens follow GSAP timeline rules: the tween that started last sets the value, a tween without
  "from" starts at whatever the earlier tweens left, and a first tween with "from" shows that
  value before it starts (fromTo). duration 0 is tl.set.
  Easings: none (linear), power1..power4 _in/_out/_in_out, sine_*, expo_*, circ_*, back_*,
  elastic_*, bounce_*, step. Bare names (power2, expo, back ...) mean _out, as in GSAP.

Node types
  group     no drawing, just a transform / opacity / clip for its children
  rect      color, optional gradient { angle (CSS deg), stops [[0,"#f00"],[1,"#00f"]], radial },
            optional stroke { color, width }
  ellipse   same fields as rect
  polygon   points [[x,y], ...] in local px, color
  rects     rects [[x, y, w, h, "#colour"], ...]; thousands are fine (GPU quads, culled)
  svg       src (file under assets) or data (inline markup), scaled to the box
  image     src, fit cover|contain|fill|tile, tile [w,h] and tile_offset [x,y] for fit tile
            (CSS background-size / negative background-position), tint
  video     src, poster, start (scene time the clip starts), media_start (seconds into the file),
            fit. Clip time = t - start + media_start. Decoded on demand only while on screen.
  text      text, font (family name; "Plex Mono" finds "IBM Plex Mono"; sans-serif, serif,
            monospace, display work too), weight, italic, size, line_height (multiple of size),
            color, align left|center|right inside width (width 0 = anchor at x), letter_spacing
            (em), wrap, stroke { color, width }, shadow { color, dx, dy, blur },
            background { color, padding [t, r, b, l], radius } (a boxed caption),
            tail_square { color, size em, gap em } (a square full stop on the baseline)
  ig_reel   tile { feed-format reel tile fields }, chrome ig|simple: Instagram-style reel chrome
  ig_fixed  nav_top, profile_poster: Instagram-style status bar, header and bottom nav

Colours: "#rgb", "#rrggbb", "#rrggbbaa", [r,g,b(,a)] in 0..1, or the same in 0..255.

Run `sprocket schema --graph --example` for a starter scene that needs no assets.
