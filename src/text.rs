//! Text for graph scenes: shaped with cosmic-text, drawn glyph by glyph so letter spacing,
//! stroke, shadow, a background box and a tail square all work. Output is premultiplied RGBA.

use crate::fonts::FontStore;
use crate::graph::{Align, Rgba, TextSpec};
use cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping, Style, SwashCache};
use tiny_skia::{Paint, Pixmap, Rect, Transform};

pub struct TextRaster {
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA.
    pub rgba: Vec<u8>,
    /// Where the texture sits in the node's local px: `[x, y, w, h]`.
    pub local: [f32; 4],
}

/// Pick a loaded family for a CSS-ish name: exact match, then a family containing the name
/// ("Plex Mono" finds "IBM Plex Mono"), then generic keywords.
pub fn resolve_family(fonts: &FontStore, name: &str) -> String {
    let lower = name.trim().trim_matches('"').to_ascii_lowercase();
    match lower.as_str() {
        "sans-serif" | "sans" | "system-ui" | "roboto" => return fonts.sans_family.clone(),
        "serif" => return fonts.serif_family.clone(),
        "monospace" | "mono" => return fonts.mono_family.clone(),
        "display" => return fonts.display_family.clone(),
        _ => {}
    }
    let db = fonts.font_system.db();
    let mut contains: Option<String> = None;
    for face in db.faces() {
        for (fam, _) in &face.families {
            let fl = fam.to_ascii_lowercase();
            if fl == lower {
                return fam.clone();
            }
            if contains.is_none() && (fl.contains(&lower) || lower.contains(&fl)) {
                contains = Some(fam.clone());
            }
        }
    }
    contains.unwrap_or_else(|| fonts.sans_family.clone())
}

struct Line {
    width: f32,
    top: f32,
    baseline: f32,
    glyphs: Vec<(cosmic_text::CacheKey, i32, i32, Option<cosmic_text::Color>)>,
}

pub fn rasterize_text(
    fonts: &mut FontStore,
    swash: &mut SwashCache,
    spec: &TextSpec,
    box_w: f32,
    scale: f32,
) -> TextRaster {
    let s = scale.max(0.05);
    let fs = spec.size * s;
    let lh = spec.line_height * spec.size * s;
    let ls = spec.letter_spacing * spec.size * s;
    let family = resolve_family(fonts, &spec.font);

    let mut buffer = Buffer::new(&mut fonts.font_system, Metrics::new(fs.max(1.0), lh.max(1.0)));
    let pad = spec.background.as_ref().map(|b| b.padding).unwrap_or([0.0; 4]);
    let wrap_w = if spec.wrap && box_w > 0.0 {
        Some(((box_w - pad[1] - pad[3]) * s).max(fs))
    } else {
        None
    };
    buffer.set_size(&mut fonts.font_system, wrap_w, None);
    let attrs = Attrs::new()
        .family(Family::Name(&family))
        .weight(FontStore::weight(spec.weight))
        .style(if spec.italic { Style::Italic } else { Style::Normal });
    buffer.set_text(&mut fonts.font_system, &spec.text, attrs, Shaping::Advanced);
    buffer.shape_until_scroll(&mut fonts.font_system, false);

    let mut lines: Vec<Line> = Vec::new();
    for run in buffer.layout_runs() {
        let mut glyphs = Vec::new();
        let mut extra = 0.0f32;
        for g in run.glyphs.iter() {
            let phys = g.physical((extra, 0.0), 1.0);
            glyphs.push((phys.cache_key, phys.x, phys.y, g.color_opt));
            extra += ls;
        }
        lines.push(Line { width: run.line_w + ls * run.glyphs.len() as f32, top: run.line_top, baseline: run.line_y, glyphs });
    }
    if lines.is_empty() {
        lines.push(Line { width: 0.0, top: 0.0, baseline: lh * 0.8, glyphs: Vec::new() });
    }

    let tail = spec.tail_square.as_ref().map(|t| (t.gap * fs, t.size * fs, t.color));
    let tail_w = tail.map(|(g, z, _)| g + z).unwrap_or(0.0);
    let n = lines.len();
    let block_w = lines
        .iter()
        .enumerate()
        .map(|(i, l)| l.width + if i == n - 1 { tail_w } else { 0.0 })
        .fold(0.0f32, f32::max);
    let block_h = lines.last().map(|l| l.top + lh).unwrap_or(lh);
    let (pt, pr, pb, pl) = (pad[0] * s, pad[1] * s, pad[2] * s, pad[3] * s);
    let boxw = block_w + pl + pr;
    let boxh = block_h + pt + pb;

    let mut margin = 2.0f32;
    if let Some(st) = &spec.stroke {
        margin += st.width * s;
    }
    if let Some(sh) = &spec.shadow {
        margin += (sh.blur * 2.0 + sh.dx.abs().max(sh.dy.abs())) * s;
    }
    margin = margin.ceil();

    let tw = (boxw + 2.0 * margin).ceil().max(1.0) as u32;
    let th = (boxh + 2.0 * margin).ceil().max(1.0) as u32;
    let mut pix = Pixmap::new(tw, th).expect("text pixmap");

    // Background box
    if let Some(bg) = &spec.background {
        let mut paint = Paint::default();
        let c = bg.color.to_u8();
        paint.set_color_rgba8(c[0], c[1], c[2], c[3]);
        paint.anti_alias = true;
        if bg.radius > 0.0 {
            if let Some(path) = rounded_rect(margin, margin, boxw, boxh, bg.radius * s) {
                pix.fill_path(&path, &paint, tiny_skia::FillRule::Winding, Transform::identity(), None);
            }
        } else if let Some(r) = Rect::from_xywh(margin, margin, boxw, boxh) {
            pix.fill_rect(r, &paint, Transform::identity(), None);
        }
    }

    let line_x = |l: &Line, i: usize| -> f32 {
        let w = l.width + if i == n - 1 { tail_w } else { 0.0 };
        margin
            + pl
            + match spec.align {
                Align::Left => 0.0,
                Align::Center => (block_w - w) * 0.5,
                Align::Right => block_w - w,
            }
    };
    let oy = margin + pt;

    let draw_all = |pix: &mut Pixmap, dx: f32, dy: f32, color: Rgba, fonts: &mut FontStore, swash: &mut SwashCache, keep_own_color: bool| {
        let base = color.to_u8();
        let base_c = cosmic_text::Color::rgba(base[0], base[1], base[2], base[3]);
        for (i, l) in lines.iter().enumerate() {
            let x0 = (line_x(l, i) + dx).round() as i32;
            let y0 = (oy + l.baseline + dy).round() as i32;
            for (key, gx, gy, own) in &l.glyphs {
                let c = if keep_own_color { own.unwrap_or(base_c) } else { base_c };
                let (pw, ph) = (pix.width() as i32, pix.height() as i32);
                let data = pix.data_mut();
                swash.with_pixels(&mut fonts.font_system, *key, c, |x, y, col| {
                    plot(data, pw, ph, x0 + gx + x, y0 + gy + y, col.r(), col.g(), col.b(), col.a());
                });
            }
        }
    };

    // Shadow under everything else
    if let Some(sh) = &spec.shadow {
        let mut sp = Pixmap::new(tw, th).expect("shadow pixmap");
        draw_all(&mut sp, sh.dx * s, sh.dy * s, sh.color, fonts, swash, false);
        if sh.blur > 0.0 {
            box_blur_premul(sp.data_mut(), tw as usize, th as usize, (sh.blur * s * 0.5).max(1.0));
        }
        pix.draw_pixmap(0, 0, sp.as_ref(), &tiny_skia::PixmapPaint::default(), Transform::identity(), None);
    }

    // Stroke (paint-order: stroke then fill, so only the outer half shows)
    if let Some(st) = &spec.stroke {
        let r = (st.width * s * 0.5).max(0.5);
        let steps = ((r * 6.0).ceil() as usize).clamp(8, 32);
        for k in 0..steps {
            let a = k as f32 / steps as f32 * std::f32::consts::TAU;
            draw_all(&mut pix, a.cos() * r, a.sin() * r, st.color, fonts, swash, false);
        }
    }

    draw_all(&mut pix, 0.0, 0.0, spec.color, fonts, swash, true);

    // Tail square sitting on the last baseline
    if let Some((gap, size, color)) = tail {
        let l = &lines[n - 1];
        let x = line_x(l, n - 1) + l.width + gap;
        let y = oy + l.baseline - size;
        let mut paint = Paint::default();
        let c = color.to_u8();
        paint.set_color_rgba8(c[0], c[1], c[2], c[3]);
        if let Some(r) = Rect::from_xywh(x, y, size, size) {
            pix.fill_rect(r, &paint, Transform::identity(), None);
        }
    }

    // Placement in local px
    let bw = boxw / s;
    let bx = if box_w > 0.0 {
        match spec.align {
            Align::Left => 0.0,
            Align::Center => (box_w - bw) * 0.5,
            Align::Right => box_w - bw,
        }
    } else {
        match spec.align {
            Align::Left => 0.0,
            Align::Center => -bw * 0.5,
            Align::Right => -bw,
        }
    };
    let m = margin / s;
    TextRaster {
        width: tw,
        height: th,
        rgba: pix.take(),
        local: [bx - m, -m, tw as f32 / s, th as f32 / s],
    }
}

#[allow(clippy::too_many_arguments)]
fn plot(data: &mut [u8], w: i32, h: i32, x: i32, y: i32, r: u8, g: u8, b: u8, a: u8) {
    if x < 0 || y < 0 || x >= w || y >= h || a == 0 {
        return;
    }
    let i = ((y * w + x) * 4) as usize;
    let sa = a as u32;
    let inv = 255 - sa;
    let pm = |c: u8| (c as u32 * sa + 127) / 255;
    data[i] = (pm(r) + (data[i] as u32 * inv + 127) / 255).min(255) as u8;
    data[i + 1] = (pm(g) + (data[i + 1] as u32 * inv + 127) / 255).min(255) as u8;
    data[i + 2] = (pm(b) + (data[i + 2] as u32 * inv + 127) / 255).min(255) as u8;
    data[i + 3] = (sa + (data[i + 3] as u32 * inv + 127) / 255).min(255) as u8;
}

pub fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w * 0.5).min(h * 0.5).max(0.0);
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

/// Three box-blur passes approximate a gaussian with standard deviation ~`sigma`.
pub fn box_blur_premul(data: &mut [u8], w: usize, h: usize, sigma: f32) {
    let r = ((sigma * 1.2).round() as usize).max(1);
    let mut tmp = vec![0u8; data.len()];
    for _ in 0..3 {
        blur_pass(data, &mut tmp, w, h, r, true);
        blur_pass(&tmp, data, w, h, r, false);
    }
}

fn blur_pass(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize, horizontal: bool) {
    let (outer, inner) = if horizontal { (h, w) } else { (w, h) };
    let idx = |o: usize, i: usize| if horizontal { (o * w + i) * 4 } else { (i * w + o) * 4 };
    let div = (2 * r + 1) as u32;
    for o in 0..outer {
        let mut acc = [0u32; 4];
        for k in 0..=(2 * r) {
            let i = k.saturating_sub(r).min(inner - 1);
            let p = idx(o, i);
            for c in 0..4 {
                acc[c] += src[p + c] as u32;
            }
        }
        for i in 0..inner {
            let p = idx(o, i);
            for c in 0..4 {
                dst[p + c] = (acc[c] / div) as u8;
            }
            let add = (i + r + 1).min(inner - 1);
            let sub = i.saturating_sub(r);
            let pa = idx(o, add);
            let ps = idx(o, sub);
            for c in 0..4 {
                acc[c] = acc[c] + src[pa + c] as u32 - src[ps + c] as u32;
            }
        }
    }
}
