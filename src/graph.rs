//! Graph scenes: a general timeline format for any reel.
//!
//! A graph scene is a stack of layers. A `nodes` layer holds a tree of nodes (groups, rects,
//! shapes, SVG, images, video, text and a few prebuilt widgets). Every node has a CSS-like box
//! (`x`, `y`, `width`, `height`), a transform origin, and GSAP-style keyframed props (`x`, `y`,
//! `scale`, `scale_x`, `scale_y`, `rotation`, `opacity`, clip insets, `blur`, `width`, `height`).
//! A `grade` layer desaturates and dims everything drawn below it.

use crate::scene::{ChromeStyle, Ease, ReelTile};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------------------------
// Colour
// ---------------------------------------------------------------------------------------------

/// Straight (non-premultiplied) RGBA, 0..1. JSON accepts `"#RRGGBB"`, `"#RRGGBBAA"`,
/// `[r,g,b]` / `[r,g,b,a]` in 0..1, or the same in 0..255 (any channel above 1 means 0..255).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Rgba(pub [f32; 4]);

impl Rgba {
    pub const BLACK: Rgba = Rgba([0.0, 0.0, 0.0, 1.0]);
    pub const WHITE: Rgba = Rgba([1.0, 1.0, 1.0, 1.0]);
    pub fn premul(self, opacity: f32) -> [f32; 4] {
        let a = self.0[3] * opacity;
        [self.0[0] * a, self.0[1] * a, self.0[2] * a, a]
    }
    pub fn to_u8(self) -> [u8; 4] {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        [c(self.0[0]), c(self.0[1]), c(self.0[2]), c(self.0[3])]
    }
}

impl Default for Rgba {
    fn default() -> Self {
        Rgba::BLACK
    }
}

pub fn parse_hex(s: &str) -> Option<Rgba> {
    let h = s.trim().trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    let nib = |i: usize| {
        u8::from_str_radix(h.get(i..i + 1)?, 16)
            .ok()
            .map(|v| v * 17)
    };
    let (r, g, b, a) = match h.len() {
        3 => (nib(0)?, nib(1)?, nib(2)?, 255),
        4 => (nib(0)?, nib(1)?, nib(2)?, nib(3)?),
        6 => (byte(0)?, byte(2)?, byte(4)?, 255),
        8 => (byte(0)?, byte(2)?, byte(4)?, byte(6)?),
        _ => return None,
    };
    Some(Rgba([
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        a as f32 / 255.0,
    ]))
}

impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Hex(String),
            Arr(Vec<f32>),
        }
        match Repr::deserialize(d)? {
            Repr::Hex(s) => parse_hex(&s).ok_or_else(|| serde::de::Error::custom(format!("bad colour {s:?}"))),
            Repr::Arr(v) => {
                if v.len() < 3 || v.len() > 4 {
                    return Err(serde::de::Error::custom("colour arrays need 3 or 4 numbers"));
                }
                let bytes = v.iter().any(|c| *c > 1.0);
                let k = if bytes { 1.0 / 255.0 } else { 1.0 };
                let a = v.get(3).copied().map(|a| if bytes { a * k } else { a }).unwrap_or(1.0);
                Ok(Rgba([v[0] * k, v[1] * k, v[2] * k, a]))
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Animated values
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Tween {
    pub start: f64,
    #[serde(default)]
    pub duration: f64,
    pub to: f32,
    /// Explicit start value (GSAP `fromTo`). Without it the tween starts from whatever the
    /// earlier tweens left at `start`.
    #[serde(default)]
    pub from: Option<f32>,
    #[serde(default)]
    pub ease: Ease,
}

/// A keyframed scalar. JSON: a plain number, `{ "initial": v, "tweens": [...] }`, or
/// `{ "initial": v, "keys": [[t, v], [t, v, "ease"], ...] }` (each key eases in from the previous).
#[derive(Debug, Clone, Serialize)]
pub struct Track {
    pub initial: f32,
    pub tweens: Vec<Tween>,
    /// Value each tween starts from, worked out once at load time.
    #[serde(skip)]
    froms: Vec<f32>,
}

impl<'de> Deserialize<'de> for Track {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Key {
            Two(f64, f32),
            Three(f64, f32, Ease),
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Const(f32),
            Anim {
                #[serde(default)]
                initial: Option<f32>,
                #[serde(default)]
                tweens: Vec<Tween>,
                #[serde(default)]
                keys: Vec<Key>,
            },
        }
        Ok(match Repr::deserialize(d)? {
            Repr::Const(v) => Track::constant(v),
            Repr::Anim { initial, mut tweens, keys } => {
                let mut init = initial;
                let mut prev: Option<(f64, f32)> = None;
                for k in keys {
                    let (t, v, e) = match k {
                        Key::Two(t, v) => (t, v, Ease::None),
                        Key::Three(t, v, e) => (t, v, e),
                    };
                    match prev {
                        None => {
                            if init.is_none() {
                                init = Some(v);
                            }
                            tweens.push(Tween { start: t, duration: 0.0, to: v, from: None, ease: Ease::None });
                        }
                        Some((pt, pv)) => tweens.push(Tween {
                            start: pt,
                            duration: (t - pt).max(0.0),
                            to: v,
                            from: Some(pv),
                            ease: e,
                        }),
                    }
                    prev = Some((t, v));
                }
                Track::new(init.unwrap_or(0.0), tweens)
            }
        })
    }
}

impl Track {
    pub fn constant(v: f32) -> Self {
        Track { initial: v, tweens: Vec::new(), froms: Vec::new() }
    }

    pub fn new(initial: f32, mut tweens: Vec<Tween>) -> Self {
        // Stable sort keeps the JSON order for tweens that start together: the later one wins,
        // as in a GSAP timeline.
        tweens.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
        let mut tr = Track { initial, tweens, froms: Vec::new() };
        let mut froms = Vec::with_capacity(tr.tweens.len());
        for i in 0..tr.tweens.len() {
            let from = match tr.tweens[i].from {
                Some(f) => f,
                None => tr.eval_upto(tr.tweens[i].start, i, &froms),
            };
            froms.push(from);
        }
        tr.froms = froms;
        tr
    }

    /// Value at `t` using only tweens `0..n`. Among the tweens that have started, the one that
    /// started last sets the value, the way a GSAP timeline renders its children in order.
    fn eval_upto(&self, t: f64, n: usize, froms: &[f32]) -> f32 {
        let mut idx = None;
        for (i, tw) in self.tweens[..n].iter().enumerate() {
            if tw.start <= t {
                idx = Some(i);
            } else {
                break;
            }
        }
        let Some(i) = idx else {
            return self.initial;
        };
        let tw = &self.tweens[i];
        let from = froms[i];
        if tw.duration <= 0.0 || t >= tw.start + tw.duration {
            return tw.to;
        }
        let p = (t - tw.start) / tw.duration;
        (from as f64 + (tw.to - from) as f64 * tw.ease.apply(p)) as f32
    }

    pub fn eval(&self, t: f64) -> f32 {
        if self.tweens.is_empty() {
            return self.initial;
        }
        // Before the first tween starts, a fromTo shows its from value (GSAP immediateRender).
        if t < self.tweens[0].start {
            return self.tweens[0].from.unwrap_or(self.initial);
        }
        self.eval_upto(t, self.tweens.len(), &self.froms)
    }
}

fn track_or(t: &Option<Track>, time: f64, default: f32) -> f32 {
    t.as_ref().map(|tr| tr.eval(time)).unwrap_or(default)
}

// ---------------------------------------------------------------------------------------------
// Scene
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GraphScene {
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub duration: f64,
    #[serde(default)]
    pub audio: Option<String>,
    #[serde(default)]
    pub background: Rgba,
    #[serde(default)]
    pub motion_blur: GraphMotionBlur,
    /// Extra font files (relative to the assets dir). Everything in `<assets>/fonts` loads anyway.
    #[serde(default)]
    pub fonts: Vec<String>,
    pub layers: Vec<Layer>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GraphMotionBlur {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "sixty")]
    pub shutter_angle: f32,
    /// Base tap count for the velocity blur; also the sub-frame count for `accumulate` layers.
    #[serde(default = "fifteen")]
    pub samples: u32,
}

fn yes() -> bool {
    true
}
fn sixty() -> f32 {
    60.0
}
fn fifteen() -> u32 {
    15
}

impl Default for GraphMotionBlur {
    fn default() -> Self {
        Self { enabled: true, shutter_angle: 60.0, samples: 15 }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Layer {
    Nodes {
        #[serde(default)]
        id: String,
        #[serde(default)]
        motion_blur: LayerBlur,
        nodes: Vec<Node>,
    },
    Grade {
        #[serde(default)]
        desaturate: Option<Track>,
        #[serde(default)]
        dim: Option<Track>,
        #[serde(default = "ink")]
        dim_color: Rgba,
    },
}

fn ink() -> Rgba {
    Rgba([17.0 / 255.0, 17.0 / 255.0, 16.0 / 255.0, 1.0])
}

/// How a nodes layer gets motion blur.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum LayerBlur {
    /// No motion blur for this layer.
    #[default]
    None,
    /// Blur along the screen motion of one node (a camera or scrolling column). Cheap and smooth;
    /// covers translation, scale and rotation of that node and everything inside it.
    Track { node: String },
    /// Render several sub-frames across the shutter and average them. Exact for every node,
    /// costs `samples` times the draw work.
    Accumulate {
        #[serde(default)]
        samples: Option<u32>,
    },
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Props {
    pub x: Option<Track>,
    pub y: Option<Track>,
    pub scale: Option<Track>,
    pub scale_x: Option<Track>,
    pub scale_y: Option<Track>,
    /// Degrees, clockwise (CSS rotate).
    pub rotation: Option<Track>,
    pub opacity: Option<Track>,
    /// CSS `clip-path: inset(...)` as fractions of the node's box, 0..1.
    pub inset_top: Option<Track>,
    pub inset_right: Option<Track>,
    pub inset_bottom: Option<Track>,
    pub inset_left: Option<Track>,
    /// Gaussian blur radius in screen px (CSS `filter: blur()`); applies to textured leaves.
    pub blur: Option<Track>,
    /// Animated box size; overrides the static `width` / `height`.
    pub width: Option<Track>,
    pub height: Option<Track>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Node {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(flatten)]
    pub kind: NodeKind,
    /// Box position in the parent's space (CSS left / top).
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default)]
    pub width: f32,
    #[serde(default)]
    pub height: f32,
    /// Transform origin as fractions of the box (CSS default 50% 50%).
    #[serde(default = "half")]
    pub origin: [f32; 2],
    #[serde(default)]
    pub props: Props,
    /// Clip children (and the node itself) to its box, like `overflow: hidden`.
    #[serde(default)]
    pub clip: bool,
    #[serde(default)]
    pub corner_radius: f32,
    /// Only drawn while `show[0] <= t < show[1]` (seconds).
    #[serde(default)]
    pub show: Option<[f64; 2]>,
    /// Fixed raster resolution for text, shapes and SVG, relative to the box. Default: worked
    /// out from the largest on-screen size the node reaches.
    #[serde(default)]
    pub raster_scale: Option<f32>,
    #[serde(default)]
    pub children: Vec<Node>,
}

fn half() -> [f32; 2] {
    [0.5, 0.5]
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    #[default]
    Cover,
    Contain,
    Fill,
    /// Repeat the image at `tile` size (CSS background-repeat), shifted by `tile_offset`.
    Tile,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Stroke {
    pub color: Rgba,
    pub width: f32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Shadow {
    pub color: Rgba,
    #[serde(default)]
    pub dx: f32,
    #[serde(default)]
    pub dy: f32,
    #[serde(default)]
    pub blur: f32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Gradient {
    /// CSS angle in degrees (180 = top to bottom).
    #[serde(default = "deg180")]
    pub angle: f32,
    /// `[[offset 0..1, colour], ...]`
    pub stops: Vec<(f32, Rgba)>,
    /// Radial gradient from the box centre instead of linear.
    #[serde(default)]
    pub radial: bool,
}

fn deg180() -> f32 {
    180.0
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TextBackground {
    pub color: Rgba,
    /// CSS padding: top, right, bottom, left.
    #[serde(default)]
    pub padding: [f32; 4],
    #[serde(default)]
    pub radius: f32,
}

/// A filled square after the last line, sitting on the baseline (a full stop drawn as a block).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TailSquare {
    pub color: Rgba,
    /// Side length in em.
    #[serde(default = "tail_size")]
    pub size: f32,
    /// Gap before it in em.
    #[serde(default = "tail_gap")]
    pub gap: f32,
}

fn tail_size() -> f32 {
    0.17
}
fn tail_gap() -> f32 {
    0.06
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TextSpec {
    pub text: String,
    /// Font family name, e.g. "IBM Plex Mono". Falls back to a system sans if missing.
    #[serde(default = "sans")]
    pub font: String,
    #[serde(default = "w400")]
    pub weight: u16,
    #[serde(default)]
    pub italic: bool,
    pub size: f32,
    /// Line height as a multiple of `size`.
    #[serde(default = "lh")]
    pub line_height: f32,
    #[serde(default = "white")]
    pub color: Rgba,
    #[serde(default)]
    pub align: Align,
    /// Extra space after each character, in em (CSS letter-spacing).
    #[serde(default)]
    pub letter_spacing: f32,
    /// Wrap lines at the box width.
    #[serde(default)]
    pub wrap: bool,
    #[serde(default)]
    pub stroke: Option<Stroke>,
    #[serde(default)]
    pub shadow: Option<Shadow>,
    #[serde(default)]
    pub background: Option<TextBackground>,
    #[serde(default)]
    pub tail_square: Option<TailSquare>,
}

fn sans() -> String {
    "sans-serif".into()
}
fn w400() -> u16 {
    400
}
fn lh() -> f32 {
    1.2
}
fn white() -> Rgba {
    Rgba::WHITE
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeKind {
    Group,
    Rect {
        #[serde(default = "white")]
        color: Rgba,
        #[serde(default)]
        gradient: Option<Gradient>,
        #[serde(default)]
        stroke: Option<Stroke>,
    },
    Ellipse {
        #[serde(default = "white")]
        color: Rgba,
        #[serde(default)]
        gradient: Option<Gradient>,
        #[serde(default)]
        stroke: Option<Stroke>,
    },
    /// Filled polygon in the node's local px.
    Polygon {
        points: Vec<[f32; 2]>,
        #[serde(default = "white")]
        color: Rgba,
    },
    /// Many solid rects in one node: `[x, y, w, h, colour]` in local px. Drawn on the GPU.
    Rects { rects: Vec<(f32, f32, f32, f32, Rgba)> },
    /// SVG file (`src`, relative to the assets dir) or inline markup (`data`), drawn to the box.
    Svg {
        #[serde(default)]
        src: Option<String>,
        #[serde(default)]
        data: Option<String>,
    },
    Image {
        src: String,
        #[serde(default)]
        fit: Fit,
        #[serde(default)]
        tile: Option<[f32; 2]>,
        #[serde(default)]
        tile_offset: [f32; 2],
        #[serde(default = "white")]
        tint: Rgba,
    },
    /// A video clip. Clip time = `t - start + media_start`. `poster` shows until the first
    /// frame decodes and after the clip runs out.
    Video {
        src: String,
        #[serde(default)]
        poster: Option<String>,
        #[serde(default)]
        start: f64,
        #[serde(default)]
        media_start: f64,
        #[serde(default)]
        fit: Fit,
    },
    Text(TextSpec),
    /// Prebuilt Instagram-style reel chrome (shading, rail, handle, caption, meme text).
    IgReel {
        tile: Box<ReelTile>,
        #[serde(default = "ig")]
        chrome: ChromeStyle,
    },
    /// Prebuilt Instagram-style fixed chrome (status bar, Reels header, bottom nav), full canvas.
    IgFixed { nav_top: f32, profile_poster: String },
}

fn ig() -> ChromeStyle {
    ChromeStyle::Ig
}

impl GraphScene {
    pub fn total_frames(&self) -> u32 {
        (self.duration * self.fps as f64).round() as u32
    }

    pub fn node_count(&self) -> usize {
        fn count(n: &[Node]) -> usize {
            n.iter().map(|c| 1 + count(&c.children)).sum()
        }
        self.layers
            .iter()
            .map(|l| match l {
                Layer::Nodes { nodes, .. } => count(nodes),
                _ => 0,
            })
            .sum()
    }

    /// Every asset path the scene reads, with a label for error messages.
    pub fn asset_refs(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Some(a) = &self.audio {
            out.push(("audio".into(), a.clone()));
        }
        for f in &self.fonts {
            out.push(("font".into(), f.clone()));
        }
        fn walk(n: &[Node], out: &mut Vec<(String, String)>) {
            for c in n {
                let label = c.id.clone().unwrap_or_else(|| "node".into());
                match &c.kind {
                    NodeKind::Image { src, .. } => out.push((format!("{label}.src"), src.clone())),
                    NodeKind::Video { src, poster, .. } => {
                        out.push((format!("{label}.src"), src.clone()));
                        if let Some(p) = poster {
                            out.push((format!("{label}.poster"), p.clone()));
                        }
                    }
                    NodeKind::Svg { src: Some(s), .. } => out.push((format!("{label}.src"), s.clone())),
                    NodeKind::IgReel { tile, .. } => out.push((format!("{label}.tile.poster"), tile.poster.clone())),
                    NodeKind::IgFixed { profile_poster, .. } => {
                        out.push((format!("{label}.profile_poster"), profile_poster.clone()))
                    }
                    _ => {}
                }
                walk(&c.children, out);
            }
        }
        for l in &self.layers {
            if let Layer::Nodes { nodes, .. } = l {
                walk(nodes, &mut out);
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------
// Flattened tree and per-frame evaluation
// ---------------------------------------------------------------------------------------------

/// 2D affine map: screen = (a*x + c*y + e, b*x + d*y + f).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Affine {
    pub const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    pub fn then(&self, child: &Affine) -> Affine {
        // self ∘ child: apply child first, then self.
        Affine {
            a: self.a * child.a + self.c * child.b,
            b: self.b * child.a + self.d * child.b,
            c: self.a * child.c + self.c * child.d,
            d: self.b * child.c + self.d * child.d,
            e: self.a * child.e + self.c * child.f + self.e,
            f: self.b * child.e + self.d * child.f + self.f,
        }
    }

    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }

    pub fn det(&self) -> f32 {
        self.a * self.d - self.b * self.c
    }

    pub fn inverse(&self) -> Option<Affine> {
        let det = self.det();
        if det.abs() < 1e-12 {
            return None;
        }
        let id = 1.0 / det;
        let a = self.d * id;
        let b = -self.b * id;
        let c = -self.c * id;
        let d = self.a * id;
        Some(Affine { a, b, c, d, e: -(a * self.e + c * self.f), f: -(b * self.e + d * self.f) })
    }

    pub fn axis_aligned(&self) -> bool {
        self.b.abs() < 1e-6 && self.c.abs() < 1e-6
    }

    /// Screen px per local px along x and y.
    pub fn scale_xy(&self) -> (f32, f32) {
        ((self.a * self.a + self.b * self.b).sqrt(), (self.c * self.c + self.d * self.d).sqrt())
    }

    /// Screen-space bounding box of a local rect.
    pub fn bbox(&self, r: [f32; 4]) -> [f32; 4] {
        let pts = [
            self.apply(r[0], r[1]),
            self.apply(r[0] + r[2], r[1]),
            self.apply(r[0], r[1] + r[3]),
            self.apply(r[0] + r[2], r[1] + r[3]),
        ];
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (x, y) in pts {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        [x0, y0, x1, y1]
    }
}

/// Clip region: a rounded rect in some node's local space.
#[derive(Debug, Clone, Copy)]
pub struct Clip {
    /// Screen -> clip-local.
    pub inv: Affine,
    pub rect: [f32; 4],
    pub radius: f32,
    /// Screen-space bbox, for culling.
    pub bbox: [f32; 4],
}

pub struct FlatNode<'a> {
    pub node: &'a Node,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}

pub struct FlatScene<'a> {
    pub scene: &'a GraphScene,
    pub nodes: Vec<FlatNode<'a>>,
    /// Root node indices per layer (empty for grade layers).
    pub roots: Vec<Vec<usize>>,
    pub by_id: HashMap<String, usize>,
}

impl<'a> FlatScene<'a> {
    pub fn new(scene: &'a GraphScene) -> Self {
        let mut fs = FlatScene { scene, nodes: Vec::new(), roots: Vec::new(), by_id: HashMap::new() };
        for (li, layer) in scene.layers.iter().enumerate() {
            let mut roots = Vec::new();
            if let Layer::Nodes { nodes, .. } = layer {
                for n in nodes {
                    roots.push(fs.push(n, li, None));
                }
            }
            fs.roots.push(roots);
        }
        fs
    }

    fn push(&mut self, n: &'a Node, layer: usize, parent: Option<usize>) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(FlatNode { node: n, parent, children: Vec::new() });
        if let Some(id) = &n.id {
            self.by_id.entry(id.clone()).or_insert(idx);
        }
        for c in &n.children {
            let ci = self.push(c, layer, Some(idx));
            self.nodes[idx].children.push(ci);
        }
        idx
    }
}

/// What a draw item samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrawSource {
    /// Flat colour (premultiplied in `color`).
    Solid,
    /// The node's own rasterised texture (text, shape, svg, widget, image).
    Static(usize),
    /// The node's current video frame (falls back to its poster).
    Video(usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UvMode {
    /// `uv` rect maps across the quad.
    Stretch([f32; 4]),
    /// Repeat every `tile` local px, shifted by `offset`.
    Tile { tile: [f32; 2], offset: [f32; 2] },
    /// Object-fit computed later from the texture's size.
    Fit(Fit),
}

#[derive(Debug, Clone, Copy)]
pub struct DrawItem {
    pub node: usize,
    pub source: DrawSource,
    pub xf: Affine,
    /// Local rect `[x, y, w, h]` the quad covers.
    pub rect: [f32; 4],
    pub uv: UvMode,
    /// Premultiplied colour multiplier (already includes opacity).
    pub color: [f32; 4],
    pub radius: f32,
    /// Blur radius in screen px.
    pub blur: f32,
    pub clip: Option<Clip>,
}

/// Local transform of a node at time t, and its box size.
pub fn local_transform(n: &Node, t: f64) -> (Affine, f32, f32) {
    let p = &n.props;
    let w = track_or(&p.width, t, n.width);
    let h = track_or(&p.height, t, n.height);
    let tx = track_or(&p.x, t, 0.0);
    let ty = track_or(&p.y, t, 0.0);
    let s = track_or(&p.scale, t, 1.0);
    let sx = s * track_or(&p.scale_x, t, 1.0);
    let sy = s * track_or(&p.scale_y, t, 1.0);
    let rot = track_or(&p.rotation, t, 0.0).to_radians();
    let (sin, cos) = rot.sin_cos();
    let ox = n.origin[0] * w;
    let oy = n.origin[1] * h;
    // translate(x + left + o) · rotate · scale · translate(-o)
    let a = cos * sx;
    let b = sin * sx;
    let c = -sin * sy;
    let d = cos * sy;
    let e = n.x + tx + ox - (a * ox + c * oy);
    let f = n.y + ty + oy - (b * ox + d * oy);
    (Affine { a, b, c, d, e, f }, w, h)
}

/// Evaluates the scene at a time and lists what to draw, layer by layer.
pub struct Evaluator<'s, 'a> {
    pub flat: &'s FlatScene<'a>,
    pub width: f32,
    pub height: f32,
}

impl<'s, 'a> Evaluator<'s, 'a> {
    pub fn new(flat: &'s FlatScene<'a>) -> Self {
        Self { flat, width: flat.scene.width as f32, height: flat.scene.height as f32 }
    }

    /// World transform of every node at `t` (visible or not). Used for motion-blur tracking.
    pub fn world_of(&self, idx: usize, t: f64) -> Affine {
        let mut chain = vec![idx];
        let mut cur = idx;
        while let Some(p) = self.flat.nodes[cur].parent {
            chain.push(p);
            cur = p;
        }
        let mut m = Affine::IDENTITY;
        for &i in chain.iter().rev() {
            let (l, _, _) = local_transform(self.flat.nodes[i].node, t);
            m = m.then(&l);
        }
        m
    }

    /// Draw items for one nodes layer at `t`, in paint order.
    pub fn layer_items(&self, layer: usize, t: f64, out: &mut Vec<DrawItem>) {
        for &r in &self.flat.roots[layer] {
            self.visit(r, t, Affine::IDENTITY, 1.0, None, out);
        }
    }

    fn visit(&self, idx: usize, t: f64, parent: Affine, parent_opacity: f32, clip: Option<Clip>, out: &mut Vec<DrawItem>) {
        let n = self.flat.nodes[idx].node;
        if let Some([s, e]) = n.show {
            if t < s || t >= e {
                return;
            }
        }
        let opacity = parent_opacity * track_or(&n.props.opacity, t, 1.0).clamp(0.0, 1.0);
        if opacity <= 0.002 {
            return;
        }
        let (local, w, h) = local_transform(n, t);
        let world = parent.then(&local);
        if world.det().abs() < 1e-9 {
            return;
        }

        // Clip from overflow hidden and/or clip-path inset, in this node's local space.
        let it = track_or(&n.props.inset_top, t, 0.0);
        let ir = track_or(&n.props.inset_right, t, 0.0);
        let ib = track_or(&n.props.inset_bottom, t, 0.0);
        let il = track_or(&n.props.inset_left, t, 0.0);
        let has_inset = n.props.inset_top.is_some()
            || n.props.inset_right.is_some()
            || n.props.inset_bottom.is_some()
            || n.props.inset_left.is_some();
        let mut clip = clip;
        if n.clip || has_inset {
            let rect = [il * w, it * h, (1.0 - il - ir) * w, (1.0 - it - ib) * h];
            if rect[2] <= 0.0 || rect[3] <= 0.0 {
                return;
            }
            let Some(inv) = world.inverse() else {
                return;
            };
            let mine = Clip { inv, rect, radius: if n.clip { n.corner_radius } else { 0.0 }, bbox: world.bbox(rect) };
            clip = Some(match clip {
                None => mine,
                Some(outer) => intersect_clips(outer, mine),
            });
            if let Some(c) = &clip {
                if c.bbox[2] <= c.bbox[0] || c.bbox[3] <= c.bbox[1] {
                    return;
                }
            }
        }

        let blur = track_or(&n.props.blur, t, 0.0).max(0.0);
        let item = |source: DrawSource, rect: [f32; 4], uv: UvMode, color: [f32; 4], radius: f32| DrawItem {
            node: idx,
            source,
            xf: world,
            rect,
            uv,
            color,
            radius,
            blur,
            clip,
        };
        let full = [0.0, 0.0, w, h];
        match &n.kind {
            NodeKind::Group => {}
            NodeKind::Rect { color, gradient, stroke } => {
                if gradient.is_none() && stroke.is_none() {
                    out.push(item(DrawSource::Solid, full, UvMode::Stretch([0.0, 0.0, 1.0, 1.0]), color.premul(opacity), n.corner_radius));
                } else {
                    out.push(item(DrawSource::Static(idx), full, UvMode::Stretch([0.0, 0.0, 1.0, 1.0]), [opacity; 4], 0.0));
                }
            }
            NodeKind::Ellipse { .. } | NodeKind::Polygon { .. } | NodeKind::Svg { .. } | NodeKind::IgReel { .. } => {
                out.push(item(DrawSource::Static(idx), full, UvMode::Stretch([0.0, 0.0, 1.0, 1.0]), [opacity; 4], 0.0));
            }
            NodeKind::IgFixed { .. } => {
                let r = [0.0, 0.0, self.width, self.height];
                out.push(item(DrawSource::Static(idx), r, UvMode::Stretch([0.0, 0.0, 1.0, 1.0]), [opacity; 4], 0.0));
            }
            NodeKind::Text(_) => {
                // The text texture's own placement is filled in by the renderer (it knows the
                // laid-out size); here we pass the box.
                out.push(item(DrawSource::Static(idx), full, UvMode::Stretch([0.0, 0.0, 1.0, 1.0]), [opacity; 4], 0.0));
            }
            NodeKind::Rects { rects } => {
                let vb = [0.0, 0.0, self.width, self.height];
                for (x, y, rw, rh, c) in rects {
                    let r = [*x, *y, *rw, *rh];
                    let bb = world.bbox(r);
                    if !overlaps(bb, vb) {
                        continue;
                    }
                    if let Some(cl) = &clip {
                        if !overlaps(bb, cl.bbox) {
                            continue;
                        }
                    }
                    out.push(item(DrawSource::Solid, r, UvMode::Stretch([0.0, 0.0, 1.0, 1.0]), c.premul(opacity), 0.0));
                }
            }
            NodeKind::Image { fit, tile, tile_offset, tint, .. } => {
                let uv = match (fit, tile) {
                    (Fit::Tile, Some(ts)) => UvMode::Tile { tile: *ts, offset: *tile_offset },
                    _ => UvMode::Fit(*fit),
                };
                out.push(item(DrawSource::Static(idx), full, uv, tint.premul(opacity), n.corner_radius));
            }
            NodeKind::Video { fit, .. } => {
                out.push(item(DrawSource::Video(idx), full, UvMode::Fit(*fit), [opacity; 4], n.corner_radius));
            }
        }

        for &c in &self.flat.nodes[idx].children {
            self.visit(c, t, world, opacity, clip, out);
        }
    }
}

fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    a[2] > b[0] && a[0] < b[2] && a[3] > b[1] && a[1] < b[3]
}

/// Intersect two clips. Exact when both are axis-aligned in screen space (the usual case);
/// otherwise the inner clip wins and the outer one only culls.
fn intersect_clips(outer: Clip, inner: Clip) -> Clip {
    let bbox = [
        outer.bbox[0].max(inner.bbox[0]),
        outer.bbox[1].max(inner.bbox[1]),
        outer.bbox[2].min(inner.bbox[2]),
        outer.bbox[3].min(inner.bbox[3]),
    ];
    if outer.inv.axis_aligned() && inner.inv.axis_aligned() && outer.radius == 0.0 && inner.radius == 0.0 {
        return Clip {
            inv: Affine::IDENTITY,
            rect: [bbox[0], bbox[1], (bbox[2] - bbox[0]).max(0.0), (bbox[3] - bbox[1]).max(0.0)],
            radius: 0.0,
            bbox,
        };
    }
    Clip { bbox, ..inner }
}

/// Screen-space motion of a node: screen velocity at point q is `l * (q - t0) + v` px/s.
pub struct MotionField {
    pub l: [f32; 4],
    pub t0: [f32; 2],
    pub v: [f32; 2],
}

pub fn motion_field(ev: &Evaluator, node: usize, t: f64) -> MotionField {
    let h = 0.5 / ev.flat.scene.fps.max(1) as f64;
    let m0 = ev.world_of(node, (t - h).max(0.0));
    let m1 = ev.world_of(node, t + h);
    let m = ev.world_of(node, t);
    let dt = ((t + h) - (t - h).max(0.0)) as f32;
    // A' (finite difference) times A^-1
    let da = [(m1.a - m0.a) / dt, (m1.b - m0.b) / dt, (m1.c - m0.c) / dt, (m1.d - m0.d) / dt];
    let inv = m.inverse().unwrap_or(Affine::IDENTITY);
    // L = A' * A^-1 (2x2, column-major a b c d)
    let l = [
        da[0] * inv.a + da[2] * inv.b,
        da[1] * inv.a + da[3] * inv.b,
        da[0] * inv.c + da[2] * inv.d,
        da[1] * inv.c + da[3] * inv.d,
    ];
    MotionField { l, t0: [m.e, m.f], v: [(m1.e - m0.e) / dt, (m1.f - m0.f) / dt] }
}
