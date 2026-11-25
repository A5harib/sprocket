use anyhow::{bail, Context, Result};
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, SyncSender};
use std::thread::{self, JoinHandle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderKind {
    Auto,
    Qsv,
    Mf,
    X264,
}

pub struct VideoEncoder {
    pub codec_name: &'static str,
    tx: Option<SyncSender<Vec<u8>>>,
    writer: Option<JoinHandle<Result<()>>>,
    child: Child,
}

impl VideoEncoder {
    pub fn start(
        width: u32,
        height: u32,
        fps: u32,
        audio_path: Option<&Path>,
        output_path: &Path,
        kind: EncoderKind,
    ) -> Result<Self> {
        if let Some(parent) = output_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }

        let order: &[&'static str] = match kind {
            EncoderKind::Auto => &["h264_qsv", "h264_mf", "libx264"],
            EncoderKind::Qsv => &["h264_qsv"],
            EncoderKind::Mf => &["h264_mf"],
            EncoderKind::X264 => &["libx264"],
        };

        let selected = order
            .iter()
            .copied()
            .find(|c| probe_encoder(c))
            .unwrap_or("libx264");

        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-s",
            &format!("{}x{}", width, height),
            "-r",
            &fps.to_string(),
            "-i",
            "pipe:0",
        ]);

        if let Some(audio) = audio_path {
            cmd.arg("-i").arg(audio);
            cmd.args(["-map", "0:v:0", "-map", "1:a:0"]);
        } else {
            cmd.args(["-map", "0:v:0"]);
        }

        match selected {
            "h264_qsv" => {
                cmd.args([
                    "-c:v",
                    "h264_qsv",
                    "-preset",
                    "faster",
                    "-global_quality",
                    "22",
                    "-pix_fmt",
                    "nv12",
                ]);
            }
            "h264_mf" => {
                cmd.args([
                    "-c:v",
                    "h264_mf",
                    "-rate_control",
                    "quality",
                    "-quality",
                    "80",
                    "-pix_fmt",
                    "nv12",
                ]);
            }
            _ => {
                cmd.args([
                    "-c:v",
                    "libx264",
                    "-preset",
                    "veryfast",
                    "-crf",
                    "20",
                    "-pix_fmt",
                    "yuv420p",
                ]);
            }
        }

        if audio_path.is_some() {
            // Pad the audio with silence so the frame count, not the audio length, sets the duration.
            cmd.args(["-af", "apad", "-c:a", "aac", "-b:a", "192k", "-shortest"]);
        }
        cmd.args(["-movflags", "+faststart"]);
        cmd.arg(output_path);

        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("failed to start ffmpeg encoder ({})", selected))?;

        let mut stdin = child.stdin.take().context("missing encoder stdin")?;
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(2);
        let writer = thread::spawn(move || -> Result<()> {
            for frame in rx {
                stdin.write_all(&frame)?;
            }
            stdin.flush()?;
            Ok(())
        });

        Ok(Self {
            codec_name: selected,
            tx: Some(tx),
            writer: Some(writer),
            child,
        })
    }

    pub fn send_frame(&self, rgba: Vec<u8>) -> Result<()> {
        if let Some(ref tx) = self.tx {
            tx.send(rgba).context("encoder pipe closed prematurely")?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        // Close channel so writer closes stdin
        self.tx.take();
        if let Some(w) = self.writer.take() {
            w.join().map_err(|_| anyhow::anyhow!("encoder writer thread panicked"))??;
        }
        let status = self.child.wait()?;
        if !status.success() {
            bail!("ffmpeg encoder ({}) exited with {}", self.codec_name, status);
        }
        Ok(())
    }
}

pub fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn probe_encoder(codec: &str) -> bool {
    if codec == "libx264" {
        return ffmpeg_available();
    }
    let pix = if codec == "h264_qsv" || codec == "h264_mf" {
        "nv12"
    } else {
        "yuv420p"
    };
    Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=256x256:r=30:d=0.04",
            "-pix_fmt",
            pix,
            "-c:v",
            codec,
            "-f",
            "null",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
