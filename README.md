# Sprocket

Fast programmatic video renderer built in Rust and `wgpu`.

Sprocket replaces headless Chromium and CPU screenshot loops for short-form reels. It draws frames on the GPU with `wgpu`, shapes text with `cosmic-text`, and pipes raw frames straight to hardware H.264 encoding so a 5-second to 30-second vertical video renders in seconds instead of minutes on a thin laptop.

## Why Sprocket exists

Rendering four 5.6-second 1080x1920 reels through headless Chrome and CPU FFmpeg took 6 to 9 minutes and pinned a 15W laptop CPU at 100 percent. Sprocket splits the work properly:

- GPU compositing and motion blur in Rust with `wgpu`
- Native font shaping with `cosmic-text`
- Hardware video encode via Intel Quick Sync, NVENC, or MediaFoundation
- Declarative scene specs built for both humans and AI agents

## Status

v0.1 prototype in progress. First benchmark target is rendering the four 1080x1920 Blinked reel variations in single-digit seconds with low CPU and RAM load.
