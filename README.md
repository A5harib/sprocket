# Sprocket

Fast programmatic video renderer built in Rust and `wgpu`.

Sprocket replaces headless Chromium and CPU screenshot loops for short-form reels. It composites frames on the GPU with `wgpu`, shapes text with `cosmic-text`, draws crisp vector chrome with `tiny-skia`, and pipes raw RGBA frames straight into hardware H.264 encoding.

## First benchmark on Blinked reel 02

Tested on an Intel Core i7-10810U laptop with an NVIDIA Quadro P520 and Intel Quick Sync `h264_qsv` rendering `v2-fullbleed` at 1080x1920, 30fps, 5.6 seconds, 168 frames:

- Sprocket total render time: 9.60s internal, 10.35s wall clock including process launch, 20.3 fps render loop
- HyperFrames headless Chromium baseline: ~101s for `v2-fullbleed` alone
- Speedup: ~10x faster
- Peak RAM: 287.8 MB for `sprocket.exe`, 581.7 MB peak across `sprocket.exe` plus child `ffmpeg` encoder and clip decoders, max 2 concurrent clip decoders
- CPU load: 5.34s CPU time for `sprocket.exe`, 4.3% average across 12 logical cores, 55.3% average system CPU

## Architecture

- `src/scene.rs`: declarative JSON scene format with GSAP-compatible easing curves, timeline velocity queries, and layout math
- `src/gpu.rs` and `src/shaders.wgsl`: `wgpu` offscreen compositing, cover-fit UV mapping, rounded-rect clipping, adaptive up to 64-tap directional motion blur, desaturation, and dim grading
- `src/fonts.rs` and `src/ui.rs`: pure-Rust WOFF2 decompression via `wuff`, text shaping via `cosmic-text`, and cached per-reel vector chrome via `tiny-skia`
- `src/decoder.rs`: windowed on-demand `ffmpeg` clip decoding that spawns only the 1 to 3 clips visible in the current frame window and releases scrolled-out clips immediately
- `src/encoder.rs`: single-pass raw RGBA pipe into `h264_qsv` with fallback to `h264_mf` and `libx264`, muxed directly with `mix.wav`

## Usage

Build the release binary:

```powershell
cargo build --release
```

Render one of the four Blinked reel 02 declarative configs under `examples/blinked-02/`:

```powershell
.\target\release\sprocket.exe render `
  --variant v2-fullbleed `
  --assets-dir C:\Users\HP\.t3\projects\soulscroll-content\reels\02-name-three\assets `
  --out out\v2-fullbleed.mp4
```

Available `--variant` presets: `v1-phone`, `v2-fullbleed`, `v3-strip`, `v4-pullback`. Pass `--scene <path>` to render any custom scene JSON, `--encoder auto|qsv|mf|x264` to pick an encoder, or `--no-motion-blur` to disable directional blur.
