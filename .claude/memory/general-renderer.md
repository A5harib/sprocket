---
name: general-renderer
description: Sprocket must be able, in principle, to render any Moxel, Hillcrest or Blinked video, not one reel's layout
metadata:
  type: feedback
---

Sprocket is meant to render any video the user makes for Moxel, Hillcrest or Blinked, not only
the reel it was first built for. When a reel needs something Sprocket can't draw, add a general
feature (a node kind, a transform, an effect) to the graph scene format instead of a one-off
field for that reel.

**Why:** the user said on 2026-11-26 that Sprocket "should on paper work with everything,
especially with any kind of moxel hillcrest or blinked video".

**How to apply:** prefer generic graph nodes (group, rect, shape, svg, image, video, text,
rects) with keyframed props over reel-specific config. Keep the legacy feed format working.
