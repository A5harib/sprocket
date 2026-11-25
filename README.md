# Sprocket

Fast programmatic video renderer built in Rust and `wgpu`.

Repo: https://github.com/A5harib/sprocket

Sprocket replaces headless Chromium and CPU screenshot loops for short-form reels. It composites frames on the GPU with `wgpu`, shapes text with `cosmic-text`, draws crisp vector chrome with `tiny-skia`, and pipes raw RGBA frames straight into hardware H.264 encoding.

## Measured benchmark on Blinked reel 02

Tested on an Intel Core i7-10810U laptop with an NVIDIA Quadro P520 and Intel Quick Sync `h264_qsv` rendering `v2-fullbleed` at 1080x1920, 30fps, 5.6 seconds, 168 frames, muxed with `mix.wav`:

| Pipeline | Render time | Peak RAM | App CPU | Notes |
|---|---|---|---|---|
| HyperFrames local | ~101.0s | 1.5 to 3 GB | 90 to 100% system | CPU `libx264` plus screenshot capture mode |
| Sprocket v0.1 Rust `wgpu` | 9.60s internal, 10.35s wall | 287.8 MB app, 581.7 MB total tree | 4.3% app across 12 threads, 55.3% avg sys | 10x faster than HyperFrames, Quadro P520 Vulkan plus Intel `h264_qsv`, max 2 concurrent decoders |
| WebCodecs plus `mediabunny` | 6.19s | 1,075.4 MB total tree | 61.3% avg sys | 16.3x faster than HyperFrames, runs in-process inside Chrome |

## Install globally on PATH

```powershell
cargo install --path . --force
```

Verify from any folder:

```powershell
sprocket --version
sprocket doctor --json
```

## Agent-friendly CLI commands

Every inspection and render command supports `--json` for structured output.

```powershell
# 1. Check GPU adapter, wgpu backend, ffmpeg, and hardware encoders
sprocket doctor --json

# 2. List built-in or folder scene configs
sprocket list --json

# 3. Read scene schema or print a starter scene JSON
sprocket schema
sprocket schema --example

# 4. Validate JSON and check that all referenced clips, posters, and audio exist
sprocket validate --variant v2-fullbleed `
  --assets-dir C:\Users\HP\.t3\projects\soulscroll-content\reels\02-name-three\assets `
  --json

# 5. Render to MP4
sprocket render --variant v2-fullbleed `
  --assets-dir C:\Users\HP\.t3\projects\soulscroll-content\reels\02-name-three\assets `
  --out out\v2-fullbleed.mp4 `
  --json
```

Available `--variant` presets: `v1-phone`, `v2-fullbleed`, `v3-strip`, `v4-pullback`. Pass `--scene <path>` to render any custom scene JSON, `--encoder auto|qsv|mf|x264` to pick an encoder, `--no-motion-blur` to disable directional blur, or `--max-decoders <N>` to cap concurrent clip decoders.

## Architecture

- `src/scene.rs`: declarative JSON scene format with GSAP-compatible easing curves, timeline velocity queries, and layout math
- `src/gpu.rs` and `src/shaders.wgsl`: `wgpu` offscreen compositing, cover-fit UV mapping, rounded-rect clipping, adaptive up to 64-tap directional motion blur, desaturation, and dim grading
- `src/fonts.rs` and `src/ui.rs`: pure-Rust WOFF2 decompression via `wuff`, text shaping via `cosmic-text`, and cached per-reel vector chrome via `tiny-skia`
- `src/decoder.rs`: windowed on-demand `ffmpeg` clip decoding that spawns only the 1 to 3 clips visible in the current frame window and releases scrolled-out clips immediately
- `src/encoder.rs`: single-pass raw RGBA pipe into `h264_qsv` with fallback to `h264_mf` and `libx264`, muxed directly with audio
