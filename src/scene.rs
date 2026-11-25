use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Scene {
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub duration: f64,
    pub audio: Option<String>,
    #[serde(default)]
    pub motion_blur: MotionBlurConfig,
    pub ground: Ground,
    pub feed: FeedColumn,
    #[serde(default)]
    pub fixed_chrome: FixedChrome,
    #[serde(default)]
    pub cover_overlay: Option<CoverOverlay>,
    #[serde(default)]
    pub post_fx: PostFx,
    #[serde(default)]
    pub headline: Option<Headline>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MotionBlurConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_shutter")]
    pub shutter_angle: f32,
    #[serde(default = "default_samples")]
    pub samples: u32,
}

impl Default for MotionBlurConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            shutter_angle: 60.0,
            samples: 15,
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_shutter() -> f32 {
    60.0
}
fn default_samples() -> u32 {
    15
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Ground {
    Solid { color: [f32; 4] },
    Paper { color: [f32; 4], texture: String, tile_px: f32 },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub corner_radius: f32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FeedColumn {
    pub viewport: Viewport,
    /// Design dimensions of one unscaled reel tile (1080x1740 for v2 IG, 1080x1920 for v1/v3/v4)
    pub tile_width: f32,
    pub tile_height: f32,
    /// Vertical pitch in column coordinates (unscaled for v1/v2/v4, scaled for v3)
    pub pitch: f32,
    /// Column horizontal offset in canvas coordinates
    #[serde(default)]
    pub column_x: f32,
    /// Base scale applied to each tile (e.g. 0.78 for v1, 1.0 for v2, 0.5 for v3)
    #[serde(default = "default_one")]
    pub tile_scale: f32,
    /// Whether tile_scale also scales the pitch/scroll (true for v1 phone screen wrapper)
    #[serde(default)]
    pub scale_wrapper: bool,
    /// Optional animated camera scale around canvas center (used by v4-pullback)
    #[serde(default)]
    pub camera_scale: Option<AnimatedValue>,
    /// Scroll Y timeline (in column coordinates)
    pub scroll_y: AnimatedValue,
    /// Chrome style for tiles
    pub chrome_style: ChromeStyle,
    /// Ordered tiles with their slot index k (can be negative for extra_before)
    pub tiles: Vec<ReelTile>,
}

fn default_one() -> f32 {
    1.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChromeStyle {
    Ig,
    Simple,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReelTile {
    pub slot_k: i32,
    pub id: String,
    /// Video file relative to assets dir (e.g. "clips/garlic.mp4"), or None if poster-only
    pub video: Option<String>,
    /// Fallback / static poster image relative to assets dir (e.g. "posters/garlic.jpg")
    pub poster: String,
    #[serde(default = "default_crop")]
    pub crop: String,
    pub handle: String,
    pub caption: String,
    pub audio_line: String,
    pub likes: String,
    pub comments: String,
    #[serde(default)]
    pub reposts: String,
    #[serde(default)]
    pub shares: String,
    #[serde(default)]
    pub overlay_text: String,
    #[serde(default)]
    pub overlay_style: OverlayStyle,
    #[serde(default)]
    pub avatar_color: [u8; 3],
    /// Optional progress bar animation (v1, v3, v4)
    #[serde(default)]
    pub progress: Option<ProgressSpec>,
    /// Optional blanking overlay animation (v3)
    #[serde(default)]
    pub blank_at: Option<Tween>,
    /// Optional margin duration label (v3)
    #[serde(default)]
    pub margin_label: Option<MarginLabel>,
}

fn default_crop() -> String {
    "full".to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayStyle {
    #[default]
    Normal,
    Quiet,
    Pov,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProgressSpec {
    pub start: f64,
    pub duration: f64,
    pub from: f32,
    pub to: f32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MarginLabel {
    pub text: String,
    pub appear_at: f64,
    pub duration: f64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FixedChrome {
    #[default]
    None,
    IgReels {
        nav_top: f32,
        profile_poster: String,
    },
    PhoneBezel {
        bezel_px: f32,
        outer_radius: f32,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CoverOverlay {
    pub show_at: f64,
    pub ground: Ground,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PostFx {
    pub desaturate: Option<AnimatedValue>,
    pub dim: Option<AnimatedValue>,
    #[serde(default = "default_ink")]
    pub dim_color: [f32; 4],
}

fn default_ink() -> [f32; 4] {
    [17.0 / 255.0, 17.0 / 255.0, 16.0 / 255.0, 1.0]
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Headline {
    pub text: String,
    pub y: f32,
    pub font_size: f32,
    pub color: [u8; 4],
    pub dot_color: [u8; 4],
    pub start: f64,
    pub duration: f64,
    pub from_scale: f32,
    pub ease: Ease,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AnimatedValue {
    pub initial: f32,
    pub tweens: Vec<Tween>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Tween {
    pub start: f64,
    pub duration: f64,
    pub to: f32,
    #[serde(default)]
    pub ease: Ease,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Ease {
    #[default]
    None,
    Power1In,
    Power1Out,
    Power2In,
    Power2Out,
    Power2InOut,
    Power3In,
    Power3Out,
}

impl Ease {
    /// Exact GSAP power easing curve: power1=quad(2), power2=cubic(3), power3=quart(4).
    pub fn apply(self, p: f64) -> f64 {
        let p = p.clamp(0.0, 1.0);
        match self {
            Ease::None => p,
            Ease::Power1In => p * p,
            Ease::Power1Out => 1.0 - (1.0 - p).powi(2),
            Ease::Power2In => p.powi(3),
            Ease::Power2Out => 1.0 - (1.0 - p).powi(3),
            Ease::Power2InOut => {
                if p < 0.5 {
                    0.5 * (2.0 * p).powi(3)
                } else {
                    1.0 - 0.5 * (2.0 - 2.0 * p).powi(3)
                }
            }
            Ease::Power3In => p.powi(4),
            Ease::Power3Out => 1.0 - (1.0 - p).powi(4),
        }
    }
}

impl AnimatedValue {
    /// Evaluate GSAP-style sequential/overlapping `tl.to(...)` tweens at time `t`.
    pub fn eval(&self, t: f64) -> f32 {
        let mut val = self.initial as f64;
        for tw in &self.tweens {
            if t < tw.start {
                break;
            }
            if tw.duration <= 0.0 || t >= tw.start + tw.duration {
                val = tw.to as f64;
            } else {
                let p = (t - tw.start) / tw.duration;
                let e = tw.ease.apply(p);
                val = val + (tw.to as f64 - val) * e;
                // Later tweens starting after this time are not active yet; if one overlaps,
                // keep stepping through sorted tweens just like GSAP overwrite.
            }
        }
        val as f32
    }

    /// Central finite-difference derivative d(val)/dt (units per second) at time `t`.
    pub fn velocity(&self, t: f64) -> f32 {
        let dt = 0.002;
        let t0 = (t - dt).max(0.0);
        let t1 = t + dt;
        let v0 = self.eval(t0);
        let v1 = self.eval(t1);
        (v1 - v0) / (t1 - t0) as f32
    }
}

impl Scene {
    pub fn total_frames(&self) -> u32 {
        (self.duration * self.fps as f64).round() as u32
    }

    /// Compute canvas-space destination rect `(x, y, w, h)` of tile `tile` at time `t`.
    pub fn tile_rect(&self, tile: &ReelTile, t: f64) -> (f32, f32, f32, f32) {
        let f = &self.feed;
        let scroll_y = f.scroll_y.eval(t);
        let cam_scale = f.camera_scale.as_ref().map(|c| c.eval(t)).unwrap_or(1.0);

        let (mut x, mut y, mut w, mut h) = if f.scale_wrapper {
            // v1-phone: entire 1080x1920 column is scaled by tile_scale inside viewport
            let s = f.tile_scale;
            let ty = scroll_y + (tile.slot_k as f32) * f.pitch;
            (
                f.viewport.x + f.column_x * s,
                f.viewport.y + ty * s,
                f.tile_width * s,
                f.tile_height * s,
            )
        } else {
            // v2, v3, v4: tiles positioned at slot_k * pitch, optionally scaled around horizontal center
            let s = f.tile_scale;
            let tw = f.tile_width * s;
            let th = f.tile_height * s;
            let tx = f.column_x + (f.tile_width - tw) * 0.5;
            let ty = scroll_y + (tile.slot_k as f32) * f.pitch;
            (tx, ty, tw, th)
        };

        if (cam_scale - 1.0).abs() > 1e-5 {
            let cx = self.width as f32 * 0.5;
            let cy = self.height as f32 * 0.5;
            x = cx + (x - cx) * cam_scale;
            y = cy + (y - cy) * cam_scale;
            w *= cam_scale;
            h *= cam_scale;
        }
        (x, y, w, h)
    }

    /// Returns true if tile overlaps the feed viewport at time `t`.
    pub fn is_tile_visible(&self, tile: &ReelTile, t: f64) -> bool {
        let (x, y, w, h) = self.tile_rect(tile, t);
        let vp = &self.feed.viewport;
        x + w > vp.x && x < vp.x + vp.width && y + h > vp.y && y < vp.y + vp.height
    }
}
