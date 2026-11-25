use crate::scene::Scene;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};

pub struct ClipFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

struct ClipWindow {
    path: PathBuf,
    width: u32,
    height: u32,
    first_frame: u32,
    last_frame: u32,
}

struct ActiveDecoder {
    width: u32,
    height: u32,
    next_frame_idx: u32,
    rx: Receiver<Vec<u8>>,
    last_rgba: Option<Vec<u8>>,
    _reader: JoinHandle<()>,
    child: Child,
}

impl ActiveDecoder {
    fn spawn(path: &Path, width: u32, height: u32, start_frame: u32, count: u32, fps: u32) -> Result<Self> {
        let start_sec = start_frame as f64 / fps as f64;
        let mut child = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-ss",
                &format!("{:.6}", start_sec),
                "-i",
            ])
            .arg(path)
            .args([
                "-frames:v",
                &count.to_string(),
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to spawn ffmpeg decoder for {}", path.display()))?;

        let mut stdout = child.stdout.take().context("missing decoder stdout")?;
        let frame_bytes = (width as usize) * (height as usize) * 4;
        // Bounded channel (2 frames) keeps RAM minimal while allowing lookahead decode
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(2);
        let reader = thread::spawn(move || {
            for _ in 0..count {
                let mut buf = vec![0u8; frame_bytes];
                if stdout.read_exact(&mut buf).is_err() {
                    break;
                }
                if tx.send(buf).is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            width,
            height,
            next_frame_idx: start_frame,
            rx,
            last_rgba: None,
            _reader: reader,
            child,
        })
    }

    fn advance_to(&mut self, frame_idx: u32) -> Option<&[u8]> {
        while self.next_frame_idx <= frame_idx {
            match self.rx.recv() {
                Ok(buf) => {
                    self.last_rgba = Some(buf);
                    self.next_frame_idx += 1;
                }
                Err(_) => break,
            }
        }
        self.last_rgba.as_deref()
    }
}

impl Drop for ActiveDecoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Manages windowed on-demand decoding across all clips in a scene.
/// Only keeps 1 to `max_concurrent` ffmpeg decoders alive at a time.
pub struct ClipPool {
    fps: u32,
    max_concurrent: usize,
    windows: HashMap<String, ClipWindow>,
    active: HashMap<String, ActiveDecoder>,
    posters: HashMap<String, ClipFrame>,
}

impl ClipPool {
    pub fn new(scene: &Scene, assets_dir: &Path, max_concurrent: usize) -> Result<Self> {
        let total = scene.total_frames();
        let mut ranges: HashMap<String, (u32, u32, PathBuf)> = HashMap::new();

        for tile in &scene.feed.tiles {
            let Some(ref rel_video) = tile.video else {
                continue;
            };
            let mut first: Option<u32> = None;
            let mut last: u32 = 0;
            for f in 0..total {
                let t = f as f64 / scene.fps as f64;
                if scene.is_tile_visible(tile, t) {
                    if first.is_none() {
                        first = Some(f);
                    }
                    last = f;
                }
            }
            if let Some(f0) = first {
                let path = assets_dir.join(rel_video);
                ranges
                    .entry(tile.id.clone())
                    .and_modify(|e| {
                        e.0 = e.0.min(f0);
                        e.1 = e.1.max(last);
                    })
                    .or_insert((f0, last, path));
            }
        }

        // Probe clip dimensions in parallel (drops startup latency from ~1.5s to ~0.15s)
        let items: Vec<(String, u32, u32, PathBuf)> = ranges
            .into_iter()
            .map(|(id, (f0, f1, p))| (id, f0, f1, p))
            .collect();
        let probed: Vec<(String, ClipWindow)> = thread::scope(|scope| {
            let handles: Vec<_> = items
                .into_iter()
                .map(|(id, first_frame, last_frame, path)| {
                    scope.spawn(move || {
                        let (width, height) = probe_dimensions(&path).unwrap_or((720, 1280));
                        (
                            id,
                            ClipWindow {
                                path,
                                width,
                                height,
                                first_frame,
                                last_frame,
                            },
                        )
                    })
                })
                .collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        let windows: HashMap<String, ClipWindow> = probed.into_iter().collect();

        Ok(Self {
            fps: scene.fps,
            max_concurrent,
            windows,
            active: HashMap::new(),
            posters: HashMap::new(),
        })
    }

    /// Retire clips that have scrolled out and pre-spawn clips entering within 2 frames.
    pub fn prepare_frame(&mut self, frame_idx: u32) -> Result<()> {
        let to_drop: Vec<String> = self
            .active
            .keys()
            .filter(|id| {
                self.windows
                    .get(*id)
                    .map(|w| frame_idx > w.last_frame)
                    .unwrap_or(true)
            })
            .cloned()
            .collect();
        for id in to_drop {
            self.active.remove(&id);
        }

        // Spawn clips whose visibility starts within the next 2 frames
        let lookahead = frame_idx + 2;
        let mut candidates: Vec<(&String, &ClipWindow)> = self
            .windows
            .iter()
            .filter(|(id, w)| {
                !self.active.contains_key(*id)
                    && w.first_frame <= lookahead
                    && frame_idx <= w.last_frame
            })
            .collect();
        candidates.sort_by_key(|(_, w)| w.first_frame);

        for (id, w) in candidates {
            if self.active.len() >= self.max_concurrent {
                break;
            }
            let count = (w.last_frame - w.first_frame + 1).max(1);
            let dec = ActiveDecoder::spawn(&w.path, w.width, w.height, w.first_frame, count, self.fps)?;
            self.active.insert(id.clone(), dec);
        }
        Ok(())
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// Fetch the RGBA frame for `tile_id` at `frame_idx`, or fall back to its poster image.
    pub fn get_frame<'a>(
        &'a mut self,
        tile_id: &str,
        poster_path: &Path,
        is_video: bool,
        frame_idx: u32,
    ) -> Result<(u32, u32, &'a [u8])> {
        if is_video {
            if let Some(dec) = self.active.get_mut(tile_id) {
                let w = dec.width;
                let h = dec.height;
                if let Some(rgba) = dec.advance_to(frame_idx) {
                    return Ok((w, h, rgba));
                }
            }
        }
        let key = poster_path.to_string_lossy().to_string();
        if !self.posters.contains_key(&key) {
            let img = image::open(poster_path)
                .with_context(|| format!("failed to load poster {}", poster_path.display()))?
                .to_rgba8();
            let (w, h) = img.dimensions();
            self.posters.insert(
                key.clone(),
                ClipFrame {
                    width: w,
                    height: h,
                    rgba: img.into_raw(),
                },
            );
        }
        let p = &self.posters[&key];
        Ok((p.width, p.height, &p.rgba))
    }
}

fn probe_dimensions(path: &Path) -> Option<(u32, u32)> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0:s=x",
        ])
        .arg(path)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut parts = text.trim().split('x');
    let w = parts.next()?.parse().ok()?;
    let h = parts.next()?.parse().ok()?;
    Some((w, h))
}
