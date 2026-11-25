mod decoder;
mod encoder;
mod fonts;
mod gpu;
mod scene;
mod ui;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use decoder::ClipPool;
use encoder::{EncoderKind, VideoEncoder};
use fonts::FontStore;
use gpu::{GpuRenderer, GpuTexture};
use scene::{FixedChrome, Ground, Scene};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;
use ui::UiRasterizer;

#[derive(Parser)]
#[command(name = "sprocket", about = "Fast programmatic video renderer in Rust + wgpu")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Render a declarative scene JSON to MP4
    Render {
        /// Path to scene JSON (overrides --variant)
        #[arg(long)]
        scene: Option<PathBuf>,

        /// Variant shorthand under examples/blinked-02/<variant>.json
        #[arg(long, default_value = "v2-fullbleed")]
        variant: String,

        /// Directory containing clips/, posters/, fonts/, paper.png, mix.wav
        #[arg(long)]
        assets_dir: PathBuf,

        /// Output MP4 path (defaults to out/<scene-id>.mp4)
        #[arg(long)]
        out: Option<PathBuf>,

        /// Video encoder backend
        #[arg(long, value_enum, default_value = "auto")]
        encoder: CliEncoder,

        /// Disable directional motion blur
        #[arg(long)]
        no_motion_blur: bool,

        /// Maximum simultaneous ffmpeg video decoders
        #[arg(long, default_value_t = 3)]
        max_decoders: usize,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum CliEncoder {
    Auto,
    Qsv,
    Mf,
    X264,
}

impl From<CliEncoder> for EncoderKind {
    fn from(v: CliEncoder) -> Self {
        match v {
            CliEncoder::Auto => EncoderKind::Auto,
            CliEncoder::Qsv => EncoderKind::Qsv,
            CliEncoder::Mf => EncoderKind::Mf,
            CliEncoder::X264 => EncoderKind::X264,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Render {
            scene,
            variant,
            assets_dir,
            out,
            encoder,
            no_motion_blur,
            max_decoders,
        } => {
            let scene_path = scene.unwrap_or_else(|| {
                PathBuf::from("examples")
                    .join("blinked-02")
                    .join(format!("{}.json", variant))
            });
            let scene_json = std::fs::read_to_string(&scene_path)
                .with_context(|| format!("failed to read scene {}", scene_path.display()))?;
            let scene: Scene = serde_json::from_str(&scene_json)
                .with_context(|| format!("failed to parse scene {}", scene_path.display()))?;

            let out_path = out.unwrap_or_else(|| PathBuf::from("out").join(format!("{}.mp4", scene.id)));
            render_scene(&scene, &assets_dir, &out_path, encoder.into(), !no_motion_blur, max_decoders)
        }
    }
}

fn render_scene(
    scene: &Scene,
    assets_dir: &Path,
    out_path: &Path,
    encoder_kind: EncoderKind,
    enable_motion_blur: bool,
    max_decoders: usize,
) -> Result<()> {
    let t_start = Instant::now();
    let total_frames = scene.total_frames();

    // 1. Initialize GPU (DX12 on Quadro P520)
    let mut gpu = GpuRenderer::new(scene.width, scene.height)?;
    println!(
        "[sprocket] GPU: {} ({}) | {}x{} @ {}fps ({} frames)",
        gpu.adapter_name, gpu.backend_name, scene.width, scene.height, scene.fps, total_frames
    );

    // 2. Load fonts & static textures
    let mut fonts = FontStore::load(assets_dir)?;
    let mut ui = UiRasterizer::new();

    let paper_tex: Option<GpuTexture> = match &scene.ground {
        Ground::Paper { texture, .. } => Some(gpu.load_image_texture(&assets_dir.join(texture), "paper")?),
        Ground::Solid { .. } => scene.cover_overlay.as_ref().and_then(|c| match &c.ground {
            Ground::Paper { texture, .. } => gpu.load_image_texture(&assets_dir.join(texture), "paper").ok(),
            _ => None,
        }),
    };

    let mut fixed_back: Option<GpuTexture> = None;
    let mut fixed_front: Option<GpuTexture> = None;
    match &scene.fixed_chrome {
        FixedChrome::None => {}
        FixedChrome::IgReels {
            nav_top,
            profile_poster,
        } => {
            let rgba = ui.render_ig_fixed(
                &mut fonts,
                scene.width,
                scene.height,
                *nav_top,
                &assets_dir.join(profile_poster),
            );
            fixed_front = Some(gpu.upload_static_rgba(scene.width, scene.height, &rgba, "ig-fixed"));
        }
        FixedChrome::PhoneBezel {
            bezel_px,
            outer_radius,
        } => {
            let vp = &scene.feed.viewport;
            let (back_rgba, front_rgba) = ui.render_phone_bezel(
                &mut fonts,
                scene.width,
                scene.height,
                vp.x,
                vp.y,
                vp.width,
                vp.height,
                *bezel_px,
                *outer_radius,
                vp.corner_radius,
            );
            fixed_back = Some(gpu.upload_static_rgba(scene.width, scene.height, &back_rgba, "phone-back"));
            fixed_front = Some(gpu.upload_static_rgba(scene.width, scene.height, &front_rgba, "phone-front"));
        }
    }

    let headline_tex: Option<(u32, u32, GpuTexture)> = scene.headline.as_ref().map(|hl| {
        let (hw, hh, rgba) = ui.render_headline(&mut fonts, hl, scene.width);
        let tex = gpu.upload_static_rgba(hw, hh, &rgba, "headline");
        (hw, hh, tex)
    });

    // Pre-rasterize small margin labels if present (v3-strip)
    let mut margin_label_texs: HashMap<i32, (u32, u32, GpuTexture)> = HashMap::new();
    for tile in &scene.feed.tiles {
        if let Some(ref ml) = tile.margin_label {
            let (lw, lh, rgba) = ui.render_margin_label(&mut fonts, &ml.text);
            let tex = gpu.upload_static_rgba(lw, lh, &rgba, "margin-label");
            margin_label_texs.insert(tile.slot_k, (lw, lh, tex));
        }
    }

    // 3. Prepare windowed clip pool & start hardware encoder
    let mut clips = ClipPool::new(scene, assets_dir, max_decoders)?;
    let audio_path = scene.audio.as_ref().map(|a| assets_dir.join(a));
    let encoder = VideoEncoder::start(
        scene.width,
        scene.height,
        scene.fps,
        audio_path.as_deref(),
        out_path,
        encoder_kind,
    )?;
    println!(
        "[sprocket] Encoder: {} | Audio: {}",
        encoder.codec_name,
        audio_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "none".to_string())
    );

    let init_ms = t_start.elapsed().as_millis();
    let t_loop = Instant::now();
    let mut peak_decoders = 0usize;

    // 4. Frame loop
    for frame_idx in 0..total_frames {
        let t = frame_idx as f64 / scene.fps as f64;
        clips.prepare_frame(frame_idx)?;
        peak_decoders = peak_decoders.max(clips.active_count());

        // Identify visible tiles & lazily update their GPU textures / UI overlays
        let mut visible_indices: Vec<usize> = Vec::new();
        let mut active_clip_keys: Vec<String> = Vec::new();
        let mut active_slots: Vec<i32> = Vec::new();

        for (idx, tile) in scene.feed.tiles.iter().enumerate() {
            if !scene.is_tile_visible(tile, t) {
                continue;
            }
            visible_indices.push(idx);
            active_slots.push(tile.slot_k);

            let is_video = tile.video.is_some();
            let clip_key = if is_video {
                tile.id.clone()
            } else {
                format!("poster:{}", tile.id)
            };
            active_clip_keys.push(clip_key.clone());

            let poster_path = assets_dir.join(&tile.poster);
            let (fw, fh, rgba) = clips.get_frame(&tile.id, &poster_path, is_video, frame_idx)?;
            gpu.update_clip_texture(&clip_key, fw, fh, rgba);

            if !gpu.has_overlay(tile.slot_k) {
                let tw = scene.feed.tile_width.round() as u32;
                let th = scene.feed.tile_height.round() as u32;
                let ov_rgba = ui.render_reel_overlay(
                    &mut fonts,
                    tile,
                    scene.feed.chrome_style,
                    tw,
                    th,
                    assets_dir,
                );
                gpu.insert_overlay(tile.slot_k, tw, th, &ov_rgba);
            }
        }

        // Release GPU textures for scrolled-out tiles to keep VRAM & RAM low
        gpu.retain_clip_textures(&active_clip_keys);
        gpu.retain_overlays(&active_slots);

        let mut fb = gpu.begin_frame();
        fb.push_ground(&scene.ground, paper_tex.as_ref(), false);
        if let Some(ref back) = fixed_back {
            fb.push_pre_feed_tex(back, [0.0, 0.0, scene.width as f32, scene.height as f32], 1.0);
        }

        // Draw visible tiles into feed pass
        for idx in visible_indices {
            let tile = &scene.feed.tiles[idx];
            let (tx, ty, tw, th) = scene.tile_rect(tile, t);
            let is_video = tile.video.is_some();
            let clip_key = if is_video {
                tile.id.clone()
            } else {
                format!("poster:{}", tile.id)
            };

            // Video / poster layer (full tile or bottom half for AITA story card)
            let media_dst = if tile.crop == "bottom" {
                [tx, ty + th * 0.5, tw, th * 0.5]
            } else {
                [tx, ty, tw, th]
            };
            // Source frame dimensions match what we uploaded
            let (sw, sh) = if tile.crop == "bottom" { (720, 640) } else { (720, 1280) };
            fb.push_feed_clip_cover(&clip_key, sw, sh, media_dst);

            // Cached per-reel UI overlay
            fb.push_feed_overlay(tile.slot_k, [tx, ty, tw, th]);

            // Optional animated progress bar (v1, v3, v4)
            if let Some(ref prog) = tile.progress {
                let frac = if t < prog.start {
                    0.0
                } else if t >= prog.start + prog.duration {
                    prog.to
                } else {
                    let p = ((t - prog.start) / prog.duration) as f32;
                    prog.from + (prog.to - prog.from) * p
                };
                if frac > 0.001 {
                    let bar_h = (7.0 * (th / scene.feed.tile_height)).max(2.0);
                    fb.push_feed_solid([tx, ty + th - bar_h, tw * frac.clamp(0.0, 1.0), bar_h], [1.0, 1.0, 1.0, 1.0]);
                }
            }

            // Optional blanking overlay (v3-strip)
            if let Some(ref bl) = tile.blank_at {
                let a = if t < bl.start {
                    0.0
                } else if bl.duration <= 0.0 || t >= bl.start + bl.duration {
                    bl.to
                } else {
                    let p = (t - bl.start) / bl.duration;
                    (bl.to as f64 * bl.ease.apply(p)) as f32
                };
                if a > 0.001 {
                    fb.push_feed_solid(
                        [tx, ty, tw, th],
                        [230.0 / 255.0, 228.0 / 255.0, 220.0 / 255.0, a],
                    );
                }
            }
        }

        // Fixed chrome above the feed (IG status bar + header + bottom nav, or phone status bar)
        if let Some(ref front) = fixed_front {
            fb.push_post_feed_tex(front, [0.0, 0.0, scene.width as f32, scene.height as f32], 1.0);
        }

        // Optional v3 margin labels drawn in the left margin alongside scrolling tiles
        for tile in &scene.feed.tiles {
            let Some(ref ml) = tile.margin_label else {
                continue;
            };
            if t < ml.appear_at {
                continue;
            }
            let p = ((t - ml.appear_at) / ml.duration.max(0.001)).clamp(0.0, 1.0);
            let e = scene::Ease::Power2Out.apply(p) as f32;
            let (_, ty, _, th) = scene.tile_rect(tile, t);
            if ty + th < -60.0 || ty > scene.height as f32 + 60.0 {
                continue;
            }
            if let Some((lw, lh, ref tex)) = margin_label_texs.get(&tile.slot_k) {
                let x = 34.0 + (-14.0 * (1.0 - e));
                let y = ty + th * 0.5 - 22.0;
                fb.push_post_feed_tex(tex, [x, y, *lw as f32, *lh as f32], e);
            }
        }

        // Optional full-frame paper cover (v1-phone)
        if let Some(ref cov) = scene.cover_overlay {
            if t >= cov.show_at {
                fb.push_ground(&cov.ground, paper_tex.as_ref(), true);
            }
        }

        // Directional motion blur velocity along scroll axis
        let shutter_dt = (scene.motion_blur.shutter_angle / 360.0) / (scene.fps as f32);
        let (blur_dy_px, radial_blur) = if enable_motion_blur && scene.motion_blur.enabled {
            let scroll_vel = scene.feed.scroll_y.velocity(t);
            let scale_mul = if scene.feed.scale_wrapper {
                scene.feed.tile_scale
            } else {
                scene
                    .feed
                    .camera_scale
                    .as_ref()
                    .map(|c| c.eval(t))
                    .unwrap_or(1.0)
            };
            let dy = (scroll_vel * scale_mul * shutter_dt).clamp(-360.0, 360.0);
            let rblur = scene
                .feed
                .camera_scale
                .as_ref()
                .map(|c| {
                    let sv = c.velocity(t);
                    let cur = c.eval(t).max(0.05);
                    ((sv / cur) * shutter_dt).clamp(-0.25, 0.25)
                })
                .unwrap_or(0.0);
            (dy, rblur)
        } else {
            (0.0, 0.0)
        };

        // PostFX grading (desaturate + dim)
        let desat = scene
            .post_fx
            .desaturate
            .as_ref()
            .map(|a| a.eval(t))
            .unwrap_or(0.0);
        let dim_a = scene
            .post_fx
            .dim
            .as_ref()
            .map(|a| a.eval(t))
            .unwrap_or(0.0);

        // Headline ("Name three.") drawn on top of grade
        if let (Some(ref hl), Some((hw, hh, ref tex))) = (&scene.headline, &headline_tex) {
            if t >= hl.start {
                let p = ((t - hl.start) / hl.duration.max(0.001)).clamp(0.0, 1.0);
                let e = hl.ease.apply(p) as f32;
                let scale = hl.from_scale + (1.0 - hl.from_scale) * e;
                let dw = *hw as f32 * scale;
                let dh = *hh as f32 * scale;
                let cx = scene.width as f32 * 0.5;
                let cy = hl.y - 30.0 + *hh as f32 * 0.5;
                fb.push_post_grade_tex(tex, [cx - dw * 0.5, cy - dh * 0.5, dw, dh], e);
            }
        }

        let vp = &scene.feed.viewport;
        let rgba = fb.render_and_readback(
            [vp.x, vp.y, vp.width, vp.height],
            vp.corner_radius,
            blur_dy_px,
            scene.motion_blur.samples,
            radial_blur,
            desat,
            dim_a,
            scene.post_fx.dim_color,
        )?;
        encoder.send_frame(rgba)?;
    }

    encoder.finish()?;

    let render_s = t_loop.elapsed().as_secs_f64();
    let total_s = t_start.elapsed().as_secs_f64();
    let fps_eff = total_frames as f64 / render_s.max(0.001);
    println!(
        "[sprocket] Done: {} | total {:.2}s (init {}ms, render {:.2}s, {:.1} fps) | peak decoders: {}",
        out_path.display(),
        total_s,
        init_ms,
        render_s,
        fps_eff,
        peak_decoders
    );
    Ok(())
}
