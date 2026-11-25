mod decoder;
mod encoder;
mod fonts;
mod gpu;
mod graph;
mod graph_render;
mod scene;
mod text;
mod ui;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use decoder::ClipPool;
use encoder::{EncoderKind, VideoEncoder};
use fonts::FontStore;
use gpu::{GpuRenderer, GpuTexture};
use graph::GraphScene;
use scene::{FixedChrome, Ground, Scene};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;
use ui::UiRasterizer;

#[derive(Parser)]
#[command(
    name = "sprocket",
    version,
    about = "Fast programmatic video renderer built in Rust and wgpu",
    long_about = "Sprocket renders declarative JSON reel scenes on the GPU with wgpu and cosmic-text and pipes raw RGBA directly to hardware H.264 encoding (Intel Quick Sync h264_qsv, MediaFoundation h264_mf, or libx264).\n\nRepo: https://github.com/A5harib/sprocket\n\nAgent workflow:\n  1. sprocket doctor --json\n  2. sprocket list --json\n  3. sprocket schema --example\n  4. sprocket validate --scene <path> --assets-dir <dir> --json\n  5. sprocket render --scene <path> --assets-dir <dir> --out <mp4> --json"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Check GPU adapter, wgpu backend, ffmpeg, and available hardware encoders
    Doctor {
        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// List available scene presets in a directory (defaults to built-in examples/blinked-02)
    List {
        /// Directory of scene JSON files to scan
        #[arg(long)]
        dir: Option<PathBuf>,

        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Print the declarative scene JSON schema guide or a starter example
    Schema {
        /// Print a minimal valid starter scene JSON instead of the field guide
        #[arg(long)]
        example: bool,

        /// Show the graph scene format (layers of animated nodes) instead of the feed format
        #[arg(long)]
        graph: bool,
    },

    /// Validate a scene JSON and verify referenced assets exist on disk before rendering
    Validate {
        /// Path to scene JSON (overrides --variant)
        #[arg(long)]
        scene: Option<PathBuf>,

        /// Preset name under examples/blinked-02/<variant>.json
        #[arg(long, default_value = "v2-fullbleed")]
        variant: String,

        /// Assets directory to check referenced clips, posters, paper texture, and audio
        #[arg(long)]
        assets_dir: Option<PathBuf>,

        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
    },

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

        /// Output machine-readable JSON summary to stdout
        #[arg(long)]
        json: bool,
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

fn default_examples_dir() -> PathBuf {
    let cwd_examples = PathBuf::from("examples").join("blinked-02");
    if cwd_examples.is_dir() {
        return cwd_examples;
    }
    if let Ok(home) = std::env::var("SPROCKET_HOME") {
        let p = PathBuf::from(home).join("examples").join("blinked-02");
        if p.is_dir() {
            return p;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("blinked-02")
}

fn resolve_scene_path(scene: Option<PathBuf>, variant: &str) -> PathBuf {
    scene.unwrap_or_else(|| default_examples_dir().join(format!("{}.json", variant)))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Doctor { json } => run_doctor(json),
        Commands::List { dir, json } => run_list(dir, json),
        Commands::Schema { example, graph } => {
            if graph {
                run_graph_schema(example);
            } else {
                run_schema(example);
            }
            Ok(())
        }
        Commands::Validate {
            scene,
            variant,
            assets_dir,
            json,
        } => {
            let scene_path = resolve_scene_path(scene, &variant);
            run_validate(&scene_path, assets_dir.as_deref(), json)
        }
        Commands::Render {
            scene,
            variant,
            assets_dir,
            out,
            encoder,
            no_motion_blur,
            max_decoders,
            json,
        } => {
            let scene_path = resolve_scene_path(scene, &variant);
            let scene = match load_scene(&scene_path)? {
                AnyScene::Graph(g) => {
                    let out_path = out.unwrap_or_else(|| PathBuf::from("out").join(format!("{}.mp4", g.id)));
                    return graph_render::render_graph(
                        &g,
                        &assets_dir,
                        &out_path,
                        &graph_render::RenderOptions {
                            encoder: encoder.into(),
                            motion_blur: !no_motion_blur,
                            max_decoders,
                            json,
                        },
                    );
                }
                AnyScene::Feed(s) => *s,
            };

            let out_path = out.unwrap_or_else(|| PathBuf::from("out").join(format!("{}.mp4", scene.id)));
            render_scene(
                &scene,
                &assets_dir,
                &out_path,
                encoder.into(),
                !no_motion_blur,
                max_decoders,
                json,
            )
        }
    }
}

/// A scene file is either the original feed format or a graph scene (it has `layers`).
enum AnyScene {
    Feed(Box<Scene>),
    Graph(Box<GraphScene>),
}

fn load_scene(path: &Path) -> Result<AnyScene> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("failed to read scene {}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).with_context(|| format!("failed to parse JSON {}", path.display()))?;
    if value.get("layers").is_some() {
        let g: GraphScene = serde_json::from_value(value)
            .with_context(|| format!("failed to parse graph scene {}", path.display()))?;
        Ok(AnyScene::Graph(Box::new(g)))
    } else {
        let s: Scene =
            serde_json::from_value(value).with_context(|| format!("failed to parse scene {}", path.display()))?;
        Ok(AnyScene::Feed(Box::new(s)))
    }
}

fn run_doctor(as_json: bool) -> Result<()> {
    let gpu_probe = GpuRenderer::probe_adapter();
    let (gpu_ok, gpu_name, gpu_backend, gpu_error) = match gpu_probe {
        Ok((name, backend)) => (true, Some(name), Some(backend), None),
        Err(e) => (false, None, None, Some(e.to_string())),
    };
    let ffmpeg_ok = encoder::ffmpeg_available();
    let qsv_ok = ffmpeg_ok && encoder::probe_encoder("h264_qsv");
    let mf_ok = ffmpeg_ok && encoder::probe_encoder("h264_mf");
    let x264_ok = ffmpeg_ok && encoder::probe_encoder("libx264");
    let recommended = if qsv_ok {
        "qsv"
    } else if mf_ok {
        "mf"
    } else if x264_ok {
        "x264"
    } else {
        "none"
    };

    if as_json {
        let out = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "repo": "https://github.com/A5harib/sprocket",
            "examples_dir": default_examples_dir().display().to_string(),
            "gpu": {
                "ok": gpu_ok,
                "adapter": gpu_name,
                "backend": gpu_backend,
                "error": gpu_error,
            },
            "ffmpeg": ffmpeg_ok,
            "encoders": {
                "h264_qsv": qsv_ok,
                "h264_mf": mf_ok,
                "libx264": x264_ok,
                "recommended": recommended,
            },
            "ready": gpu_ok && ffmpeg_ok && recommended != "none",
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("sprocket {}", env!("CARGO_PKG_VERSION"));
        println!("repo: https://github.com/A5harib/sprocket");
        println!("examples_dir: {}", default_examples_dir().display());
        match (gpu_name, gpu_backend) {
            (Some(n), Some(b)) => println!("gpu: {} ({})", n, b),
            _ => println!("gpu: ERROR ({})", gpu_error.unwrap_or_default()),
        }
        println!("ffmpeg: {}", if ffmpeg_ok { "ok" } else { "MISSING" });
        println!(
            "encoders: h264_qsv={} h264_mf={} libx264={} (recommended: {})",
            qsv_ok, mf_ok, x264_ok, recommended
        );
    }

    if !gpu_ok || !ffmpeg_ok {
        anyhow::bail!("doctor check failed");
    }
    Ok(())
}

fn run_list(dir: Option<PathBuf>, as_json: bool) -> Result<()> {
    let dir = dir.unwrap_or_else(default_examples_dir);
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .with_context(|| format!("failed to read scene directory {}", dir.display()))?
    {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        match load_scene(&path) {
            Ok(AnyScene::Feed(scene)) => entries.push(serde_json::json!({
                "variant": stem,
                "format": "feed",
                "id": scene.id,
                "width": scene.width,
                "height": scene.height,
                "fps": scene.fps,
                "duration": scene.duration,
                "frames": scene.total_frames(),
                "tiles": scene.feed.tiles.len(),
                "path": path.display().to_string(),
            })),
            Ok(AnyScene::Graph(g)) => entries.push(serde_json::json!({
                "variant": stem,
                "format": "graph",
                "id": g.id,
                "width": g.width,
                "height": g.height,
                "fps": g.fps,
                "duration": g.duration,
                "frames": g.total_frames(),
                "nodes": g.node_count(),
                "path": path.display().to_string(),
            })),
            Err(_) => {}
        }
    }
    entries.sort_by(|a, b| a["variant"].as_str().cmp(&b["variant"].as_str()));

    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "dir": dir.display().to_string(),
                "count": entries.len(),
                "scenes": entries,
            }))?
        );
    } else {
        println!("Scenes in {}:", dir.display());
        for e in &entries {
            let parts = if e["format"] == "graph" {
                format!("{} nodes", e["nodes"].as_u64().unwrap_or(0))
            } else {
                format!("{} tiles", e["tiles"].as_u64().unwrap_or(0))
            };
            println!(
                "  {}  [{}] {}x{} @ {}fps  {:.2}s ({} frames, {})  {}",
                e["variant"].as_str().unwrap_or(""),
                e["format"].as_str().unwrap_or(""),
                e["width"].as_u64().unwrap_or(0),
                e["height"].as_u64().unwrap_or(0),
                e["fps"].as_u64().unwrap_or(0),
                e["duration"].as_f64().unwrap_or(0.0),
                e["frames"].as_u64().unwrap_or(0),
                parts,
                e["path"].as_str().unwrap_or(""),
            );
        }
    }
    Ok(())
}

fn run_validate(scene_path: &Path, assets_dir: Option<&Path>, as_json: bool) -> Result<()> {
    let scene = match load_scene(scene_path)? {
        AnyScene::Graph(g) => return run_validate_graph(&g, scene_path, assets_dir, as_json),
        AnyScene::Feed(s) => *s,
    };

    let mut errors: Vec<String> = Vec::new();
    if scene.width == 0 || scene.height == 0 {
        errors.push("width and height must be > 0".to_string());
    }
    if scene.fps == 0 || scene.duration <= 0.0 {
        errors.push("fps and duration must be > 0".to_string());
    }

    let mut checked_assets = 0usize;
    if let Some(dir) = assets_dir {
        let mut check_rel = |rel: &str, label: &str| {
            checked_assets += 1;
            let p = dir.join(rel);
            if !p.is_file() {
                errors.push(format!("missing {}: {}", label, p.display()));
            }
        };
        if let Some(ref a) = scene.audio {
            check_rel(a, "audio");
        }
        if let Ground::Paper { ref texture, .. } = scene.ground {
            check_rel(texture, "ground.texture");
        }
        if let Some(ref cov) = scene.cover_overlay {
            if let Ground::Paper { ref texture, .. } = cov.ground {
                check_rel(texture, "cover_overlay.ground.texture");
            }
        }
        if let FixedChrome::IgReels { ref profile_poster, .. } = scene.fixed_chrome {
            check_rel(profile_poster, "fixed_chrome.profile_poster");
        }
        for tile in &scene.feed.tiles {
            check_rel(&tile.poster, &format!("tile[{}].poster", tile.id));
            if let Some(ref v) = tile.video {
                check_rel(v, &format!("tile[{}].video", tile.id));
            }
        }
    }

    let ok = errors.is_empty();
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": ok,
                "scene": scene_path.display().to_string(),
                "id": scene.id,
                "width": scene.width,
                "height": scene.height,
                "fps": scene.fps,
                "duration": scene.duration,
                "frames": scene.total_frames(),
                "tiles": scene.feed.tiles.len(),
                "checked_assets": checked_assets,
                "errors": errors,
            }))?
        );
    } else if ok {
        println!(
            "[sprocket] OK: {} ({}x{} @ {}fps, {} frames, {} tiles, {} assets verified)",
            scene.id,
            scene.width,
            scene.height,
            scene.fps,
            scene.total_frames(),
            scene.feed.tiles.len(),
            checked_assets
        );
    } else {
        for e in &errors {
            eprintln!("[sprocket] ERROR: {}", e);
        }
    }

    if !ok {
        anyhow::bail!("scene validation failed with {} error(s)", errors.len());
    }
    Ok(())
}

fn run_validate_graph(scene: &GraphScene, scene_path: &Path, assets_dir: Option<&Path>, as_json: bool) -> Result<()> {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    if scene.width == 0 || scene.height == 0 {
        errors.push("width and height must be > 0".to_string());
    }
    if scene.fps == 0 || scene.duration <= 0.0 {
        errors.push("fps and duration must be > 0".to_string());
    }
    let flat = graph::FlatScene::new(scene);
    let mut seen = std::collections::HashSet::new();
    for n in &flat.nodes {
        if let Some(id) = &n.node.id {
            if !seen.insert(id.clone()) {
                warnings.push(format!("duplicate node id {id:?} (motion blur tracks the first)"));
            }
        }
    }
    for l in &scene.layers {
        if let graph::Layer::Nodes { motion_blur: graph::LayerBlur::Track { node }, .. } = l {
            if !flat.by_id.contains_key(node) {
                errors.push(format!("motion_blur track node {node:?} does not exist"));
            }
        }
    }
    let refs = scene.asset_refs();
    let mut checked = 0usize;
    if let Some(dir) = assets_dir {
        for (label, rel) in &refs {
            checked += 1;
            let p = dir.join(rel);
            if !p.is_file() {
                errors.push(format!("missing {}: {}", label, p.display()));
            }
        }
    }
    let ok = errors.is_empty();
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": ok,
                "format": "graph",
                "scene": scene_path.display().to_string(),
                "id": scene.id,
                "width": scene.width,
                "height": scene.height,
                "fps": scene.fps,
                "duration": scene.duration,
                "frames": scene.total_frames(),
                "layers": scene.layers.len(),
                "nodes": flat.nodes.len(),
                "checked_assets": checked,
                "errors": errors,
                "warnings": warnings,
            }))?
        );
    } else {
        for w in &warnings {
            eprintln!("[sprocket] WARN: {}", w);
        }
        if ok {
            println!(
                "[sprocket] OK: {} graph ({}x{} @ {}fps, {} frames, {} layers, {} nodes, {} assets verified)",
                scene.id,
                scene.width,
                scene.height,
                scene.fps,
                scene.total_frames(),
                scene.layers.len(),
                flat.nodes.len(),
                checked
            );
        } else {
            for e in &errors {
                eprintln!("[sprocket] ERROR: {}", e);
            }
        }
    }
    if !ok {
        anyhow::bail!("scene validation failed with {} error(s)", errors.len());
    }
    Ok(())
}

fn run_graph_schema(example: bool) {
    if example {
        println!("{}", include_str!("../examples/graph-starter.json"));
    } else {
        println!("{}", include_str!("../docs/graph-scenes.md"));
    }
}

fn run_schema(example: bool) {
    if example {
        println!(
            r#"{{
  "id": "demo-reel",
  "width": 1080,
  "height": 1920,
  "fps": 30,
  "duration": 4.0,
  "audio": "mix.wav",
  "motion_blur": {{ "enabled": true, "shutter_angle": 60.0, "samples": 15 }},
  "ground": {{ "kind": "solid", "color": [0.067, 0.067, 0.063, 1.0] }},
  "feed": {{
    "viewport": {{ "x": 0.0, "y": 0.0, "width": 1080.0, "height": 1920.0, "corner_radius": 0.0 }},
    "tile_width": 1080.0,
    "tile_height": 1920.0,
    "pitch": 1920.0,
    "column_x": 0.0,
    "tile_scale": 1.0,
    "scale_wrapper": false,
    "scroll_y": {{
      "initial": 0.0,
      "tweens": [
        {{ "start": 0.8, "duration": 0.35, "to": -1920.0, "ease": "power2_in_out" }}
      ]
    }},
    "chrome_style": "ig",
    "tiles": [
      {{
        "slot_k": 0,
        "id": "clip-1",
        "video": "clips/sample.mp4",
        "poster": "posters/sample.jpg",
        "crop": "full",
        "handle": "@handle",
        "caption": "Short caption",
        "audio_line": "Original audio",
        "likes": "12K",
        "comments": "402",
        "overlay_text": "",
        "overlay_style": "normal",
        "avatar_color": [240, 56, 31]
      }}
    ]
  }},
  "fixed_chrome": {{ "kind": "none" }},
  "post_fx": {{
    "desaturate": {{ "initial": 0.0, "tweens": [{{ "start": 2.8, "duration": 0.25, "to": 1.0, "ease": "power2_out" }}] }},
    "dim": {{ "initial": 0.0, "tweens": [{{ "start": 2.8, "duration": 0.25, "to": 0.62, "ease": "power2_out" }}] }}
  }},
  "headline": {{
    "text": "Name three",
    "y": 840.0,
    "font_size": 210.0,
    "color": [245, 244, 238, 255],
    "dot_color": [240, 56, 31, 255],
    "start": 3.0,
    "duration": 0.25,
    "from_scale": 0.94,
    "ease": "power2_out"
  }}
}}"#
        );
    } else {
        println!(
            r#"Sprocket Scene JSON Schema (v0.1)
Top-level fields:
  id: string                        Scene identifier (also default output filename)
  width, height, fps: u32           Canvas dimensions and framerate (e.g. 1080, 1920, 30)
  duration: f64                     Total length in seconds
  audio: string | null              Relative path inside --assets-dir (e.g. "mix.wav")
  motion_blur: {{ enabled, shutter_angle, samples }}
  ground:
    {{ "kind": "solid", "color": [r,g,b,a] }}
    {{ "kind": "paper", "color": [r,g,b,a], "texture": "paper.png", "tile_px": 512.0 }}
  feed:
    viewport: {{ x, y, width, height, corner_radius }}
    tile_width, tile_height, pitch, column_x, tile_scale: f32
    scale_wrapper: bool
    camera_scale: AnimatedValue | null
    scroll_y: AnimatedValue ({{ initial, tweens: [{{ start, duration, to, ease }}] }})
    chrome_style: "ig" | "simple"
    tiles: [{{ slot_k, id, video, poster, crop ("full"|"bottom"), handle, caption, audio_line, likes, comments, reposts, shares, overlay_text, overlay_style ("normal"|"quiet"|"pov"), avatar_color [r,g,b], progress, blank_at, margin_label }}]
  fixed_chrome:
    {{ "kind": "none" }}
    {{ "kind": "ig_reels", "nav_top": 1740.0, "profile_poster": "posters/cat.jpg" }}
    {{ "kind": "phone_bezel", "bezel_px": 22.0, "outer_radius": 76.0 }}
  cover_overlay: {{ show_at, ground }} | null
  post_fx: {{ desaturate: AnimatedValue?, dim: AnimatedValue?, dim_color: [r,g,b,a] }}
  headline: {{ text, y, font_size, color [r,g,b,a], dot_color [r,g,b,a], start, duration, from_scale, ease }} | null

Supported easings: none, power1_in, power1_out, power2_in, power2_out, power2_in_out, power3_in, power3_out

Run `sprocket schema --example` for a copy-pasteable starter JSON."#
        );
    }
}

fn render_scene(
    scene: &Scene,
    assets_dir: &Path,
    out_path: &Path,
    encoder_kind: EncoderKind,
    enable_motion_blur: bool,
    max_decoders: usize,
    as_json: bool,
) -> Result<()> {
    let t_start = Instant::now();
    let total_frames = scene.total_frames();

    // 1. Initialize GPU (DX12/Vulkan on dedicated GPU)
    let mut gpu = GpuRenderer::new(scene.width, scene.height)?;
    if !as_json {
        println!(
            "[sprocket] GPU: {} ({}) | {}x{} @ {}fps ({} frames)",
            gpu.adapter_name, gpu.backend_name, scene.width, scene.height, scene.fps, total_frames
        );
    }

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
    if !as_json {
        println!(
            "[sprocket] Encoder: {} | Audio: {}",
            encoder.codec_name,
            audio_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "none".to_string())
        );
    }

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

    let codec_name = encoder.codec_name;
    encoder.finish()?;

    let render_s = t_loop.elapsed().as_secs_f64();
    let total_s = t_start.elapsed().as_secs_f64();
    let fps_eff = total_frames as f64 / render_s.max(0.001);
    let out_bytes = std::fs::metadata(out_path).map(|m| m.len()).unwrap_or(0);

    if as_json {
        let report = serde_json::json!({
            "ok": true,
            "id": scene.id,
            "out": out_path.display().to_string(),
            "bytes": out_bytes,
            "width": scene.width,
            "height": scene.height,
            "fps": scene.fps,
            "frames": total_frames,
            "duration": scene.duration,
            "gpu": gpu.adapter_name,
            "backend": gpu.backend_name,
            "encoder": codec_name,
            "init_ms": init_ms,
            "render_s": (render_s * 100.0).round() / 100.0,
            "total_s": (total_s * 100.0).round() / 100.0,
            "render_fps": (fps_eff * 10.0).round() / 10.0,
            "peak_decoders": peak_decoders,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "[sprocket] Done: {} | total {:.2}s (init {}ms, render {:.2}s, {:.1} fps) | peak decoders: {}",
            out_path.display(),
            total_s,
            init_ms,
            render_s,
            fps_eff,
            peak_decoders
        );
    }
    Ok(())
}
