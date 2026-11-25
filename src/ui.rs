use crate::fonts::FontStore;
use crate::scene::{ChromeStyle, Headline, OverlayStyle, ReelTile};
use cosmic_text::{Align, Attrs, Buffer, Color as TextColor, Family, Metrics, Shaping, Style, SwashCache};
use std::path::Path;
use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Paint, PathBuilder, Pixmap,
    Point, Rect, SpreadMode, Stroke, Transform,
};

pub struct UiRasterizer {
    swash: SwashCache,
}

impl UiRasterizer {
    pub fn new() -> Self {
        Self {
            swash: SwashCache::new(),
        }
    }

    /// Rasterize one reel's static UI overlay (gradients, chrome, icons, text, AITA card, meme overlay)
    /// into straight RGBA bytes of size `(width, height)`.
    pub fn render_reel_overlay(
        &mut self,
        fonts: &mut FontStore,
        tile: &ReelTile,
        chrome: ChromeStyle,
        width: u32,
        height: u32,
        assets_dir: &Path,
    ) -> Vec<u8> {
        let mut pixmap = Pixmap::new(width, height).expect("valid pixmap size");
        let w = width as f32;
        let h = height as f32;

        // 1. AITA Reddit story top card when crop == "bottom"
        if tile.crop == "bottom" {
            let half_h = h * 0.5;
            fill_rect(&mut pixmap, 0.0, 0.0, w, half_h, [26, 26, 27, 255]);
            let pad_top = if chrome == ChromeStyle::Ig { 300.0 } else { 260.0 };
            let sans = fonts.sans_family.clone();
            self.draw_text(
                fonts,
                &mut pixmap,
                "r/AmItheJerk",
                70.0,
                pad_top,
                w - 140.0,
                30.0,
                36.0,
                &sans,
                700,
                Style::Normal,
                [255, 139, 96, 255],
                Align::Left,
                false,
                false,
            );
            self.draw_text(
                fonts,
                &mut pixmap,
                "AITA for telling my sister-in-law she can't bring her kids to my wedding?",
                70.0,
                pad_top + 56.0,
                w - 140.0,
                58.0,
                68.0,
                &sans,
                700,
                Style::Normal,
                [255, 255, 255, 255],
                Align::Left,
                false,
                false,
            );
            self.draw_text(
                fonts,
                &mut pixmap,
                "14.2k upvotes · 3,108 comments",
                70.0,
                pad_top + 285.0,
                w - 140.0,
                28.0,
                34.0,
                &sans,
                400,
                Style::Normal,
                [154, 153, 143, 255],
                Align::Left,
                false,
                false,
            );
        }

        // 2. Meme overlay text
        if !tile.overlay_text.is_empty() {
            let sans = fonts.sans_family.clone();
            let serif = fonts.serif_family.clone();
            match tile.overlay_style {
                OverlayStyle::Normal => {
                    self.draw_text(
                        fonts,
                        &mut pixmap,
                        &tile.overlay_text,
                        70.0,
                        520.0,
                        w - 140.0,
                        74.0,
                        82.0,
                        &sans,
                        900,
                        Style::Normal,
                        [255, 255, 255, 255],
                        Align::Center,
                        true,
                        true,
                    );
                }
                OverlayStyle::Quiet => {
                    self.draw_text(
                        fonts,
                        &mut pixmap,
                        &tile.overlay_text,
                        70.0,
                        860.0,
                        w - 140.0,
                        64.0,
                        74.0,
                        &serif,
                        400,
                        Style::Italic,
                        [255, 255, 255, 255],
                        Align::Center,
                        true,
                        false,
                    );
                }
                OverlayStyle::Pov => {
                    self.draw_text(
                        fonts,
                        &mut pixmap,
                        &tile.overlay_text,
                        70.0,
                        300.0,
                        w - 140.0,
                        66.0,
                        74.0,
                        &sans,
                        800,
                        Style::Normal,
                        [255, 255, 255, 255],
                        Align::Left,
                        true,
                        false,
                    );
                }
            }
        }

        // 3. Per-reel chrome
        match chrome {
            ChromeStyle::Ig => self.draw_ig_reel_chrome(fonts, &mut pixmap, tile, w, h, assets_dir),
            ChromeStyle::Simple => self.draw_simple_reel_chrome(fonts, &mut pixmap, tile, w, h),
        }

        pixmap_to_straight_rgba(&pixmap)
    }

    fn draw_ig_reel_chrome(
        &mut self,
        fonts: &mut FontStore,
        pixmap: &mut Pixmap,
        tile: &ReelTile,
        w: f32,
        h: f32,
        assets_dir: &Path,
    ) {
        // Top gradient: 330px tall, 0.38 -> 0.0
        fill_vgradient(
            pixmap,
            0.0,
            0.0,
            w,
            330.0,
            &[
                (0.0, Color::from_rgba(0.0, 0.0, 0.0, 0.38).unwrap()),
                (1.0, Color::from_rgba(0.0, 0.0, 0.0, 0.0).unwrap()),
            ],
        );

        // Bottom gradient: 820px tall, 0.0 -> 0.28 (at 55%) -> 0.58 (at 100%)
        fill_vgradient(
            pixmap,
            0.0,
            h - 820.0,
            w,
            820.0,
            &[
                (0.0, Color::from_rgba(0.0, 0.0, 0.0, 0.0).unwrap()),
                (0.55, Color::from_rgba(0.0, 0.0, 0.0, 0.28).unwrap()),
                (1.0, Color::from_rgba(0.0, 0.0, 0.0, 0.58).unwrap()),
            ],
        );

        let poster_img = image::open(assets_dir.join(&tile.poster))
            .ok()
            .map(|i| i.to_rgba8());
        let sans = fonts.sans_family.clone();

        // --- Right rail (right: 22px, bottom: 46px, width: 126px -> center x = 995) ---
        let rail_cx = w - 22.0 - 63.0;
        let mut cursor_y = h - 46.0;

        // Disc: 80x80 rounded square (r=16) with 4px white border
        cursor_y -= 80.0;
        let disc_x = rail_cx - 40.0;
        let disc_y = cursor_y;
        if let Some(ref pimg) = poster_img {
            blit_rounded_image(pixmap, pimg, disc_x + 3.0, disc_y + 3.0, 74.0, 74.0, 13.0);
        } else {
            fill_rounded_rect(pixmap, disc_x, disc_y, 80.0, 80.0, 16.0, [40, 40, 40, 255]);
        }
        stroke_rounded_rect(pixmap, disc_x + 2.0, disc_y + 2.0, 76.0, 76.0, 15.0, 4.0, [255, 255, 255, 255]);

        // More 3 dots (padding 14px, dot 11px, gap 9px)
        cursor_y -= 30.0 + 39.0;
        let dots_cy = cursor_y + 19.5;
        for dx in [-20.0f32, 0.0, 20.0] {
            fill_circle(pixmap, rail_cx + dx, dots_cy, 5.5, [255, 255, 255, 255]);
        }

        // 4 rail buttons stacked upwards: share, repost, comment, heart
        let items = [
            ("share", tile.shares.as_str()),
            ("repost", tile.reposts.as_str()),
            ("comment", tile.comments.as_str()),
            ("heart", tile.likes.as_str()),
        ];
        for (icon_name, label) in items {
            let label_h = 36.0;
            let icon_sz = 78.0;
            cursor_y -= 30.0 + icon_sz + 6.0 + label_h;
            draw_ig_icon(pixmap, icon_name, rail_cx - icon_sz * 0.5, cursor_y, icon_sz, 1.9, [255, 255, 255, 255]);
            self.draw_text(
                fonts,
                pixmap,
                label,
                rail_cx - 65.0,
                cursor_y + icon_sz + 4.0,
                130.0,
                31.0,
                36.0,
                &sans,
                500,
                Style::Normal,
                [255, 255, 255, 255],
                Align::Center,
                true,
                false,
            );
        }

        // --- Bottom-left meta (left: 40px, bottom: 48px) ---
        let meta_x = 40.0;
        let audio_y = h - 48.0 - 38.0;
        draw_ig_icon(pixmap, "note", meta_x, audio_y + 2.0, 34.0, 2.2, [255, 255, 255, 255]);
        self.draw_text(
            fonts,
            pixmap,
            &tile.audio_line,
            meta_x + 46.0,
            audio_y,
            700.0,
            32.0,
            38.0,
            &sans,
            400,
            Style::Normal,
            [255, 255, 255, 240],
            Align::Left,
            true,
            false,
        );

        // Caption row (20px above audio)
        let cap_y = audio_y - 20.0 - 44.0;
        let clean_cap = strip_non_bmp(&tile.caption);
        if clean_cap.is_empty() {
            self.draw_text(
                fonts,
                pixmap,
                "... more",
                meta_x,
                cap_y,
                820.0,
                35.0,
                42.0,
                &sans,
                400,
                Style::Normal,
                [255, 255, 255, 184],
                Align::Left,
                true,
                false,
            );
        } else {
            let trimmed = truncate_words(&clean_cap, 36);
            let cap_w = self.measure_text_width(fonts, &trimmed, 35.0, &sans, 400);
            self.draw_text(
                fonts,
                pixmap,
                &trimmed,
                meta_x,
                cap_y,
                670.0,
                35.0,
                42.0,
                &sans,
                400,
                Style::Normal,
                [255, 255, 255, 245],
                Align::Left,
                true,
                false,
            );
            self.draw_text(
                fonts,
                pixmap,
                "... more",
                (meta_x + cap_w + 10.0).min(meta_x + 675.0),
                cap_y,
                160.0,
                35.0,
                42.0,
                &sans,
                400,
                Style::Normal,
                [255, 255, 255, 184],
                Align::Left,
                true,
                false,
            );
        }

        // Who row: 84x84 circular avatar + handle + Follow pill (22px above caption)
        let who_y = cap_y - 22.0 - 84.0;
        if let Some(ref pimg) = poster_img {
            blit_rounded_image(pixmap, pimg, meta_x, who_y, 84.0, 84.0, 42.0);
        } else {
            fill_circle(pixmap, meta_x + 42.0, who_y + 42.0, 42.0, [200, 80, 60, 255]);
        }

        let handle_x = meta_x + 84.0 + 24.0;
        let handle_w = self.measure_text_width(fonts, &tile.handle, 37.0, &sans, 700);
        self.draw_text(
            fonts,
            pixmap,
            &tile.handle,
            handle_x,
            who_y + 20.0,
            560.0,
            37.0,
            44.0,
            &sans,
            700,
            Style::Normal,
            [255, 255, 255, 255],
            Align::Left,
            true,
            false,
        );

        // Follow pill
        let fol_x = (handle_x + handle_w + 24.0).min(710.0);
        let fol_y = who_y + 16.0;
        let fol_w = 148.0;
        let fol_h = 54.0;
        stroke_rounded_rect(pixmap, fol_x, fol_y, fol_w, fol_h, 18.0, 2.5, [255, 255, 255, 242]);
        self.draw_text(
            fonts,
            pixmap,
            "Follow",
            fol_x,
            fol_y + 8.0,
            fol_w,
            33.0,
            38.0,
            &sans,
            600,
            Style::Normal,
            [255, 255, 255, 255],
            Align::Center,
            true,
            false,
        );
    }

    fn draw_simple_reel_chrome(
        &mut self,
        fonts: &mut FontStore,
        pixmap: &mut Pixmap,
        tile: &ReelTile,
        w: f32,
        h: f32,
    ) {
        // Bottom gradient: 760px tall, 0.0 -> 0.62
        fill_vgradient(
            pixmap,
            0.0,
            h - 760.0,
            w,
            760.0,
            &[
                (0.0, Color::from_rgba(0.0, 0.0, 0.0, 0.0).unwrap()),
                (1.0, Color::from_rgba(0.0, 0.0, 0.0, 0.62).unwrap()),
            ],
        );

        let sans = fonts.sans_family.clone();

        // Right rail (right: 30px, bottom: 150px, width: 120px)
        let rail_cx = w - 30.0 - 60.0;
        let mut cy = h - 150.0;

        // Three dots
        cy -= 20.0;
        for dx in [-18.0f32, 0.0, 18.0] {
            fill_circle(pixmap, rail_cx + dx, cy, 4.5, [255, 255, 255, 255]);
        }

        // Send
        cy -= 46.0 + 76.0;
        draw_ig_icon(pixmap, "share", rail_cx - 38.0, cy, 76.0, 2.0, [255, 255, 255, 255]);

        // Comment + count
        cy -= 46.0 + 76.0 + 8.0 + 34.0;
        draw_ig_icon(pixmap, "comment", rail_cx - 38.0, cy, 76.0, 2.0, [255, 255, 255, 255]);
        self.draw_text(
            fonts,
            pixmap,
            &tile.comments,
            rail_cx - 60.0,
            cy + 82.0,
            120.0,
            28.0,
            34.0,
            &sans,
            700,
            Style::Normal,
            [255, 255, 255, 255],
            Align::Center,
            true,
            false,
        );

        // Heart + likes
        cy -= 46.0 + 76.0 + 8.0 + 34.0;
        draw_ig_icon(pixmap, "heart", rail_cx - 38.0, cy, 76.0, 2.0, [255, 255, 255, 255]);
        self.draw_text(
            fonts,
            pixmap,
            &tile.likes,
            rail_cx - 60.0,
            cy + 82.0,
            120.0,
            28.0,
            34.0,
            &sans,
            700,
            Style::Normal,
            [255, 255, 255, 255],
            Align::Center,
            true,
            false,
        );

        // Bottom-left meta (left: 44px, bottom: 120px)
        let meta_x = 44.0;
        let aud_y = h - 120.0 - 34.0;
        self.draw_text(
            fonts,
            pixmap,
            &tile.audio_line,
            meta_x,
            aud_y,
            800.0,
            28.0,
            34.0,
            &sans,
            400,
            Style::Normal,
            [255, 255, 255, 218],
            Align::Left,
            false,
            false,
        );

        let cap_y = aud_y - 16.0 - 40.0;
        let cap_text = if tile.caption.is_empty() {
            "... more".to_string()
        } else {
            format!("{} ... more", tile.caption)
        };
        self.draw_text(
            fonts,
            pixmap,
            &cap_text,
            meta_x,
            cap_y,
            820.0,
            33.0,
            40.0,
            &sans,
            400,
            Style::Normal,
            [255, 255, 255, 242],
            Align::Left,
            false,
            false,
        );

        let who_y = cap_y - 22.0 - 76.0;
        let av_col = [tile.avatar_color[0], tile.avatar_color[1], tile.avatar_color[2], 255];
        fill_circle(pixmap, meta_x + 38.0, who_y + 38.0, 38.0, [255, 255, 255, 255]);
        fill_circle(pixmap, meta_x + 38.0, who_y + 38.0, 35.0, av_col);
        let initial = tile
            .handle
            .chars()
            .next()
            .unwrap_or('A')
            .to_ascii_uppercase()
            .to_string();
        self.draw_text(
            fonts,
            pixmap,
            &initial,
            meta_x,
            who_y + 17.0,
            76.0,
            34.0,
            40.0,
            &sans,
            900,
            Style::Normal,
            [17, 17, 16, 255],
            Align::Center,
            false,
            false,
        );

        let hx = meta_x + 76.0 + 22.0;
        let hw = self.measure_text_width(fonts, &tile.handle, 36.0, &sans, 700);
        self.draw_text(
            fonts,
            pixmap,
            &tile.handle,
            hx,
            who_y + 17.0,
            550.0,
            36.0,
            42.0,
            &sans,
            700,
            Style::Normal,
            [255, 255, 255, 255],
            Align::Left,
            false,
            false,
        );

        let fx = (hx + hw + 22.0).min(710.0);
        stroke_rounded_rect(pixmap, fx, who_y + 14.0, 134.0, 48.0, 14.0, 2.0, [255, 255, 255, 230]);
        self.draw_text(
            fonts,
            pixmap,
            "Follow",
            fx,
            who_y + 20.0,
            134.0,
            30.0,
            34.0,
            &sans,
            700,
            Style::Normal,
            [255, 255, 255, 255],
            Align::Center,
            false,
            false,
        );

        // Progress track at bottom
        fill_rect(pixmap, 0.0, h - 7.0, w, 7.0, [255, 255, 255, 72]);
    }

    /// Rasterize the fixed Android Instagram Reels status bar, Reels header, and bottom nav bar.
    pub fn render_ig_fixed(
        &mut self,
        fonts: &mut FontStore,
        width: u32,
        height: u32,
        nav_top: f32,
        profile_poster: &Path,
    ) -> Vec<u8> {
        let mut pixmap = Pixmap::new(width, height).expect("valid pixmap size");
        let w = width as f32;
        let h = height as f32;
        let sans = fonts.sans_family.clone();

        // --- Status bar (y=0..70) ---
        self.draw_text(
            fonts,
            &mut pixmap,
            "1:47",
            52.0,
            18.0,
            160.0,
            33.0,
            38.0,
            &sans,
            500,
            Style::Normal,
            [255, 255, 255, 255],
            Align::Left,
            true,
            false,
        );

        // Right side: signal + wifi + battery + "12%"
        let right_x = w - 46.0;
        let pct_w = 66.0;
        self.draw_text(
            fonts,
            &mut pixmap,
            "12%",
            right_x - pct_w,
            20.0,
            pct_w + 10.0,
            30.0,
            34.0,
            &sans,
            500,
            Style::Normal,
            [255, 255, 255, 255],
            Align::Right,
            true,
            false,
        );

        // Battery icon
        let batt_x = right_x - pct_w - 16.0 - 40.0;
        let batt_y = 24.0;
        stroke_rounded_rect(&mut pixmap, batt_x, batt_y + 4.0, 32.0, 18.0, 4.0, 2.2, [255, 255, 255, 255]);
        fill_rounded_rect(&mut pixmap, batt_x + 3.0, batt_y + 7.0, 5.0, 12.0, 1.5, [240, 56, 31, 255]);
        fill_rounded_rect(&mut pixmap, batt_x + 34.0, batt_y + 9.0, 3.5, 8.0, 1.5, [255, 255, 255, 255]);

        // Wifi icon (filled triangle sector approximation)
        let wifi_x = batt_x - 18.0 - 34.0;
        let wifi_y = 22.0;
        draw_wifi_icon(&mut pixmap, wifi_x, wifi_y, 32.0);

        // Signal bars (4 ascending bars)
        let sig_x = wifi_x - 18.0 - 30.0;
        let sig_bottom = 48.0;
        for (idx, bh) in [6.0f32, 12.0, 18.0, 24.0].iter().enumerate() {
            fill_rect(&mut pixmap, sig_x + idx as f32 * 7.5, sig_bottom - bh, 5.0, *bh, [255, 255, 255, 255]);
        }

        // --- Reels header (y=70..220) ---
        self.draw_text(
            fonts,
            &mut pixmap,
            "Reels",
            44.0,
            106.0,
            240.0,
            58.0,
            66.0,
            &sans,
            700,
            Style::Normal,
            [255, 255, 255, 255],
            Align::Left,
            true,
            false,
        );
        let reels_w = self.measure_text_width(fonts, "Reels", 58.0, &sans, 700);
        draw_ig_icon(&mut pixmap, "chev", 44.0 + reels_w + 8.0, 118.0, 52.0, 2.6, [255, 255, 255, 255]);
        draw_ig_icon(&mut pixmap, "camera", w - 44.0 - 72.0, 108.0, 72.0, 2.0, [255, 255, 255, 255]);

        // --- Bottom nav bar (y=nav_top..h) ---
        fill_rect(&mut pixmap, 0.0, nav_top, w, h - nav_top, [0, 0, 0, 255]);
        let row_cy = nav_top + 70.0;
        let pad = 20.0;
        let slot_w = (w - pad * 2.0) / 5.0;
        let icon_sz = 70.0;

        // 0: Home
        let cx0 = pad + slot_w * 0.5;
        draw_ig_icon(&mut pixmap, "home", cx0 - icon_sz * 0.5, row_cy - icon_sz * 0.5, icon_sz, 2.0, [255, 255, 255, 255]);

        // 1: Active Reels (filled white rounded rect with dark clapper lines + play triangle)
        let cx1 = pad + slot_w * 1.5;
        draw_reels_active_icon(&mut pixmap, cx1 - icon_sz * 0.5, row_cy - icon_sz * 0.5, icon_sz);

        // 2: DM
        let cx2 = pad + slot_w * 2.5;
        draw_ig_icon(&mut pixmap, "share", cx2 - icon_sz * 0.5, row_cy - icon_sz * 0.5, icon_sz, 2.0, [255, 255, 255, 255]);

        // 3: Search
        let cx3 = pad + slot_w * 3.5;
        draw_ig_icon(&mut pixmap, "search", cx3 - icon_sz * 0.5, row_cy - icon_sz * 0.5, icon_sz, 2.0, [255, 255, 255, 255]);

        // 4: Profile avatar
        let cx4 = pad + slot_w * 4.5;
        if let Ok(img) = image::open(profile_poster) {
            let rgba = img.to_rgba8();
            blit_rounded_image(
                &mut pixmap,
                &rgba,
                cx4 - 35.0,
                row_cy - 35.0,
                70.0,
                70.0,
                35.0,
            );
        } else {
            fill_circle(&mut pixmap, cx4, row_cy, 35.0, [110, 110, 110, 255]);
        }

        // Gesture pill: left=470, top=nav_top+160, w=140, h=10, r=5
        fill_rounded_rect(&mut pixmap, 470.0, nav_top + 160.0, 140.0, 10.0, 5.0, [255, 255, 255, 140]);

        pixmap_to_straight_rgba(&pixmap)
    }

    /// Rasterize the phone bezel + status bar overlay for v1-phone.
    pub fn render_phone_bezel(
        &mut self,
        fonts: &mut FontStore,
        width: u32,
        height: u32,
        vp_x: f32,
        vp_y: f32,
        vp_w: f32,
        vp_h: f32,
        bezel_px: f32,
        outer_radius: f32,
        inner_radius: f32,
    ) -> (Vec<u8>, Vec<u8>) {
        // Back layer: drop shadow + solid bezel behind the screen
        let mut back = Pixmap::new(width, height).expect("valid pixmap size");
        let bx = vp_x - bezel_px;
        let by = vp_y - bezel_px;
        let bw = vp_w + bezel_px * 2.0;
        let bh = vp_h + bezel_px * 2.0;

        // Soft layered drop shadow
        for step in (1..=10).rev() {
            let expand = step as f32 * 5.0;
            let alpha = (26.0 / (step as f32).sqrt()) as u8;
            fill_rounded_rect(
                &mut back,
                bx - expand * 0.4,
                by + 24.0 - expand * 0.2,
                bw + expand * 0.8,
                bh + expand * 0.8,
                outer_radius + expand * 0.3,
                [17, 17, 16, alpha],
            );
        }
        fill_rounded_rect(&mut back, bx, by, bw, bh, outer_radius, [11, 11, 10, 255]);

        // Front layer: status bar drawn on top of the phone screen
        let mut front = Pixmap::new(width, height).expect("valid pixmap size");
        let _ = inner_radius;
        let s = vp_w / 1080.0;
        let sans = fonts.sans_family.clone();
        self.draw_text(
            fonts,
            &mut front,
            "1:47",
            vp_x + 70.0 * s,
            vp_y + 28.0 * s,
            160.0 * s,
            38.0 * s,
            44.0 * s,
            &sans,
            700,
            Style::Normal,
            [255, 255, 255, 255],
            Align::Left,
            true,
            false,
        );
        // Dynamic island pill
        fill_rounded_rect(
            &mut front,
            vp_x + (1080.0 - 150.0) * 0.5 * s,
            vp_y + 34.0 * s,
            150.0 * s,
            44.0 * s,
            22.0 * s,
            [0, 0, 0, 255],
        );
        self.draw_text(
            fonts,
            &mut front,
            "●●● 12%",
            vp_x + vp_w - 220.0 * s,
            vp_y + 32.0 * s,
            150.0 * s,
            30.0 * s,
            36.0 * s,
            &sans,
            700,
            Style::Normal,
            [255, 255, 255, 255],
            Align::Right,
            true,
            false,
        );

        (pixmap_to_straight_rgba(&back), pixmap_to_straight_rgba(&front))
    }

    /// Rasterize the `"Name three."` display headline into a centered texture of size `(width, band_h)`.
    pub fn render_headline(
        &mut self,
        fonts: &mut FontStore,
        headline: &Headline,
        canvas_w: u32,
    ) -> (u32, u32, Vec<u8>) {
        let band_h: u32 = 340;
        let mut pixmap = Pixmap::new(canvas_w, band_h).expect("valid headline pixmap");
        let fam = fonts.display_family.clone();
        let fs = headline.font_size;
        let text_w = self.measure_text_width(fonts, &headline.text, fs, &fam, 800);
        let dot_sz = fs * 0.17;
        let dot_gap = fs * 0.06;
        let total_w = text_w + dot_gap + dot_sz;
        let start_x = (canvas_w as f32 - total_w) * 0.5;
        let top_y = 30.0;

        self.draw_text(
            fonts,
            &mut pixmap,
            &headline.text,
            start_x,
            top_y,
            text_w + 40.0,
            fs,
            fs * 0.95,
            &fam,
            800,
            Style::Normal,
            headline.color,
            Align::Left,
            false,
            false,
        );

        // Red square period sitting on the display text baseline
        let dot_x = start_x + text_w + dot_gap;
        let dot_y = top_y + fs * 0.78 - dot_sz;
        fill_rect(&mut pixmap, dot_x, dot_y, dot_sz, dot_sz, headline.dot_color);

        (canvas_w, band_h, pixmap_to_straight_rgba(&pixmap))
    }

    /// Rasterize a small margin duration label (e.g. "0.50s") for v3-strip.
    pub fn render_margin_label(&mut self, fonts: &mut FontStore, text: &str) -> (u32, u32, Vec<u8>) {
        let w = 180u32;
        let h = 56u32;
        let mut pixmap = Pixmap::new(w, h).expect("valid label pixmap");
        let mono = fonts.mono_family.clone();
        self.draw_text(
            fonts,
            &mut pixmap,
            text,
            0.0,
            6.0,
            w as f32,
            34.0,
            40.0,
            &mono,
            600,
            Style::Normal,
            [17, 17, 16, 255],
            Align::Left,
            false,
            false,
        );
        (w, h, pixmap_to_straight_rgba(&pixmap))
    }

    fn measure_text_width(
        &self,
        fonts: &mut FontStore,
        text: &str,
        font_size: f32,
        family: &str,
        weight: u16,
    ) -> f32 {
        let mut buffer = Buffer::new(&mut fonts.font_system, Metrics::new(font_size, font_size * 1.15));
        buffer.set_size(&mut fonts.font_system, Some(2000.0), Some(font_size * 2.0));
        let attrs = Attrs::new()
            .family(Family::Name(family))
            .weight(FontStore::weight(weight));
        buffer.set_text(&mut fonts.font_system, text, attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut fonts.font_system, false);
        let mut max_w = 0.0f32;
        for run in buffer.layout_runs() {
            max_w = max_w.max(run.line_w);
        }
        max_w
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_text(
        &mut self,
        fonts: &mut FontStore,
        pixmap: &mut Pixmap,
        text: &str,
        x: f32,
        y: f32,
        max_width: f32,
        font_size: f32,
        line_height: f32,
        family: &str,
        weight: u16,
        style: Style,
        rgba: [u8; 4],
        align: Align,
        shadow: bool,
        outline: bool,
    ) {
        let mut buffer = Buffer::new(&mut fonts.font_system, Metrics::new(font_size, line_height));
        buffer.set_size(&mut fonts.font_system, Some(max_width), Some(1000.0));
        let attrs = Attrs::new()
            .family(Family::Name(family))
            .weight(FontStore::weight(weight))
            .style(style);
        buffer.set_text(&mut fonts.font_system, text, attrs, Shaping::Advanced);
        for line in buffer.lines.iter_mut() {
            line.set_align(Some(align));
        }
        buffer.shape_until_scroll(&mut fonts.font_system, false);

        if outline {
            for (ox, oy) in [(-3.0, 0.0), (3.0, 0.0), (0.0, -3.0), (0.0, 3.0), (-2.0, -2.0), (2.0, 2.0), (-2.0, 2.0), (2.0, -2.0)] {
                self.blit_buffer(fonts, &buffer, pixmap, x + ox, y + oy, [0, 0, 0, 255]);
            }
        } else if shadow {
            self.blit_buffer(fonts, &buffer, pixmap, x, y + 2.0, [0, 0, 0, 110]);
        }

        self.blit_buffer(fonts, &buffer, pixmap, x, y, rgba);
    }

    fn blit_buffer(
        &mut self,
        fonts: &mut FontStore,
        buffer: &Buffer,
        pixmap: &mut Pixmap,
        origin_x: f32,
        origin_y: f32,
        rgba: [u8; 4],
    ) {
        let pw = pixmap.width() as i32;
        let ph = pixmap.height() as i32;
        let data = pixmap.data_mut();
        let col = TextColor::rgba(rgba[0], rgba[1], rgba[2], rgba[3]);

        buffer.draw(&mut fonts.font_system, &mut self.swash, col, |gx, gy, _w, _h, c| {
            let px = gx + origin_x.round() as i32;
            let py = gy + origin_y.round() as i32;
            if px < 0 || py < 0 || px >= pw || py >= ph {
                return;
            }
            let sa = c.a() as u32;
            if sa == 0 {
                return;
            }
            let idx = ((py * pw + px) * 4) as usize;
            // Source-over onto premultiplied pixmap
            let sr = (c.r() as u32 * sa + 127) / 255;
            let sg = (c.g() as u32 * sa + 127) / 255;
            let sb = (c.b() as u32 * sa + 127) / 255;
            let inv = 255 - sa;

            data[idx] = (sr + (data[idx] as u32 * inv + 127) / 255).min(255) as u8;
            data[idx + 1] = (sg + (data[idx + 1] as u32 * inv + 127) / 255).min(255) as u8;
            data[idx + 2] = (sb + (data[idx + 2] as u32 * inv + 127) / 255).min(255) as u8;
            data[idx + 3] = (sa + (data[idx + 3] as u32 * inv + 127) / 255).min(255) as u8;
        });
    }
}

// ---------------------------------------------------------------------------
// Vector shapes, icons, gradients & rounded image blitting
// ---------------------------------------------------------------------------

fn fill_rect(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, rgba: [u8; 4]) {
    let Some(rect) = Rect::from_xywh(x, y, w, h) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
}

fn fill_vgradient(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, stops: &[(f32, Color)]) {
    let Some(rect) = Rect::from_xywh(x, y, w, h) else {
        return;
    };
    let gstops: Vec<GradientStop> = stops
        .iter()
        .map(|(p, c)| GradientStop::new(*p, *c))
        .collect();
    let Some(shader) = LinearGradient::new(
        Point::from_xy(x, y),
        Point::from_xy(x, y + h),
        gstops,
        SpreadMode::Pad,
        Transform::identity(),
    ) else {
        return;
    };
    let mut paint = Paint::default();
    paint.shader = shader;
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
}

fn rounded_rect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w * 0.5).min(h * 0.5).max(0.0);
    let k = r * 0.55228475;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

fn fill_rounded_rect(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, rgba: [u8; 4]) {
    let Some(path) = rounded_rect_path(x, y, w, h, r) else {
        return;
    };
    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
}

fn stroke_rounded_rect(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    stroke_w: f32,
    rgba: [u8; 4],
) {
    let Some(path) = rounded_rect_path(x, y, w, h, r) else {
        return;
    };
    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
    let stroke = Stroke {
        width: stroke_w,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

fn fill_circle(pixmap: &mut Pixmap, cx: f32, cy: f32, r: f32, rgba: [u8; 4]) {
    let Some(path) = PathBuilder::from_circle(cx, cy, r) else {
        return;
    };
    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
}

/// Draw crisp vector icons matching `ig_ui.py` (in a 24x24 viewBox scaled to `size`).
fn draw_ig_icon(
    pixmap: &mut Pixmap,
    name: &str,
    x: f32,
    y: f32,
    size: f32,
    stroke_24: f32,
    rgba: [u8; 4],
) {
    let s = size / 24.0;
    let tf = Transform::from_scale(s, s).post_translate(x, y);
    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
    let stroke = Stroke {
        width: stroke_24,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };

    let mut pb = PathBuilder::new();
    match name {
        "heart" => {
            pb.move_to(12.0, 20.3);
            pb.cubic_to(7.5, 17.5, 3.8, 13.8, 3.8, 9.4);
            pb.cubic_to(3.8, 6.8, 5.8, 4.9, 8.3, 4.9);
            pb.cubic_to(9.9, 4.9, 11.2, 5.7, 12.0, 7.0);
            pb.cubic_to(12.8, 5.7, 14.1, 4.9, 15.7, 4.9);
            pb.cubic_to(18.2, 4.9, 20.2, 6.8, 20.2, 9.4);
            pb.cubic_to(20.2, 13.8, 16.5, 17.5, 12.0, 20.3);
            pb.close();
        }
        "comment" => {
            // Circular speech bubble with bottom-right tail
            let r = 8.8f32;
            let cx = 12.0f32;
            let cy = 12.0f32;
            let steps = 40;
            for i in 0..=steps {
                // Start at ~50 deg and sweep to ~35 deg around the top/left
                let a = (50.0 + (345.0 * i as f32 / steps as f32)).to_radians();
                let px = cx + r * a.cos();
                let py = cy + r * a.sin();
                if i == 0 {
                    pb.move_to(px, py);
                } else {
                    pb.line_to(px, py);
                }
            }
            pb.line_to(21.4, 21.4);
            pb.close();
        }
        "repost" => {
            pb.move_to(4.5, 11.5);
            pb.line_to(4.5, 9.8);
            pb.quad_to(4.5, 6.5, 7.8, 6.5);
            pb.line_to(19.2, 6.5);
            pb.move_to(16.3, 3.6);
            pb.line_to(19.2, 6.5);
            pb.line_to(16.3, 9.4);

            pb.move_to(19.5, 12.5);
            pb.line_to(19.5, 14.2);
            pb.quad_to(19.5, 17.5, 16.2, 17.5);
            pb.line_to(4.8, 17.5);
            pb.move_to(7.7, 20.4);
            pb.line_to(4.8, 17.5);
            pb.line_to(7.7, 14.6);
        }
        "share" => {
            pb.move_to(21.6, 2.6);
            pb.line_to(2.7, 8.9);
            pb.line_to(10.3, 12.5);
            pb.line_to(13.9, 20.1);
            pb.close();
            pb.move_to(10.3, 12.5);
            pb.line_to(15.7, 7.1);
        }
        "note" => {
            pb.move_to(9.0, 18.2);
            pb.line_to(9.0, 5.6);
            pb.line_to(19.5, 3.4);
            pb.line_to(19.5, 15.8);
            if let Some(p) = pb.finish() {
                pixmap.stroke_path(&p, &paint, &stroke, tf, None);
            }
            if let Some(c1) = PathBuilder::from_circle(6.6, 18.2, 2.5) {
                pixmap.stroke_path(&c1, &paint, &stroke, tf, None);
            }
            if let Some(c2) = PathBuilder::from_circle(17.0, 15.8, 2.5) {
                pixmap.stroke_path(&c2, &paint, &stroke, tf, None);
            }
            return;
        }
        "camera" => {
            if let Some(rr) = rounded_rect_path(2.8, 6.3, 18.4, 13.6, 3.4) {
                pixmap.stroke_path(&rr, &paint, &stroke, tf, None);
            }
            if let Some(c) = PathBuilder::from_circle(12.0, 13.1, 3.8) {
                pixmap.stroke_path(&c, &paint, &stroke, tf, None);
            }
            pb.move_to(8.6, 6.3);
            pb.line_to(10.0, 3.9);
            pb.line_to(14.0, 3.9);
            pb.line_to(15.4, 6.3);
        }
        "chev" => {
            pb.move_to(6.5, 9.5);
            pb.line_to(12.0, 15.0);
            pb.line_to(17.5, 9.5);
        }
        "home" => {
            pb.move_to(3.5, 10.6);
            pb.line_to(12.0, 3.5);
            pb.line_to(20.5, 10.6);
            pb.line_to(20.5, 20.6);
            pb.line_to(14.7, 20.6);
            pb.line_to(14.7, 14.4);
            pb.line_to(9.3, 14.4);
            pb.line_to(9.3, 20.6);
            pb.line_to(3.5, 20.6);
            pb.close();
        }
        "search" => {
            if let Some(c) = PathBuilder::from_circle(10.6, 10.6, 7.1) {
                pixmap.stroke_path(&c, &paint, &stroke, tf, None);
            }
            pb.move_to(16.0, 16.0);
            pb.line_to(21.0, 21.0);
        }
        _ => return,
    }

    if let Some(path) = pb.finish() {
        pixmap.stroke_path(&path, &paint, &stroke, tf, None);
    }
}

fn draw_reels_active_icon(pixmap: &mut Pixmap, x: f32, y: f32, size: f32) {
    let s = size / 24.0;
    let tf = Transform::from_scale(s, s).post_translate(x, y);

    if let Some(rr) = rounded_rect_path(2.6, 2.6, 18.8, 18.8, 5.0) {
        let mut paint = Paint::default();
        paint.anti_alias = true;
        paint.set_color_rgba8(255, 255, 255, 255);
        pixmap.fill_path(&rr, &paint, FillRule::Winding, tf, None);
    }

    let mut dark = Paint::default();
    dark.anti_alias = true;
    dark.set_color_rgba8(0, 0, 0, 255);
    let stroke = Stroke {
        width: 1.6,
        line_cap: LineCap::Round,
        ..Stroke::default()
    };
    let mut pb = PathBuilder::new();
    pb.move_to(2.8, 8.2);
    pb.line_to(21.2, 8.2);
    pb.move_to(8.6, 2.8);
    pb.line_to(11.4, 8.2);
    pb.move_to(14.4, 2.8);
    pb.line_to(17.2, 8.2);
    if let Some(p) = pb.finish() {
        pixmap.stroke_path(&p, &dark, &stroke, tf, None);
    }

    let mut tri = PathBuilder::new();
    tri.move_to(10.0, 11.4);
    tri.line_to(15.0, 14.4);
    tri.line_to(10.0, 17.4);
    tri.close();
    if let Some(p) = tri.finish() {
        pixmap.fill_path(&p, &dark, FillRule::Winding, tf, None);
    }
}

fn draw_wifi_icon(pixmap: &mut Pixmap, x: f32, y: f32, size: f32) {
    let s = size / 24.0;
    let tf = Transform::from_scale(s, s).post_translate(x, y);
    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.set_color_rgba8(255, 255, 255, 255);
    let mut pb = PathBuilder::new();
    pb.move_to(12.0, 20.5);
    pb.line_to(1.5, 9.2);
    pb.quad_to(12.0, 2.5, 22.5, 9.2);
    pb.close();
    if let Some(p) = pb.finish() {
        pixmap.fill_path(&p, &paint, FillRule::Winding, tf, None);
    }
}

/// Blit an RGBA image into a rounded-rect destination using center-crop cover fit.
fn blit_rounded_image(
    pixmap: &mut Pixmap,
    src: &image::RgbaImage,
    dx: f32,
    dy: f32,
    dw: f32,
    dh: f32,
    radius: f32,
) {
    let sw = src.width() as f32;
    let sh = src.height() as f32;
    let src_aspect = sw / sh;
    let dst_aspect = dw / dh;

    let (crop_x, crop_y, crop_w, crop_h) = if src_aspect > dst_aspect {
        let cw = sh * dst_aspect;
        ((sw - cw) * 0.5, 0.0, cw, sh)
    } else {
        let ch = sw / dst_aspect;
        (0.0, (sh - ch) * 0.5, sw, ch)
    };

    let pw = pixmap.width() as i32;
    let ph = pixmap.height() as i32;
    let data = pixmap.data_mut();

    let x0 = dx.floor().max(0.0) as i32;
    let y0 = dy.floor().max(0.0) as i32;
    let x1 = (dx + dw).ceil().min(pw as f32) as i32;
    let y1 = (dy + dh).ceil().min(ph as f32) as i32;

    let half_w = dw * 0.5;
    let half_h = dh * 0.5;
    let r = radius.min(half_w).min(half_h);

    for py in y0..y1 {
        let ly = py as f32 + 0.5 - dy;
        let qy = (ly - half_h).abs() - (half_h - r);
        for px in x0..x1 {
            let lx = px as f32 + 0.5 - dx;
            let qx = (lx - half_w).abs() - (half_w - r);
            let dist = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;
            let alpha = (0.5 - dist).clamp(0.0, 1.0);
            if alpha <= 0.001 {
                continue;
            }
            let sx = (crop_x + (lx / dw) * crop_w).clamp(0.0, sw - 1.0) as u32;
            let sy = (crop_y + (ly / dh) * crop_h).clamp(0.0, sh - 1.0) as u32;
            let sp = src.get_pixel(sx, sy).0;
            let sa = (alpha * 255.0).round() as u32;
            let sr = (sp[0] as u32 * sa + 127) / 255;
            let sg = (sp[1] as u32 * sa + 127) / 255;
            let sb = (sp[2] as u32 * sa + 127) / 255;
            let inv = 255 - sa;
            let idx = ((py * pw + px) * 4) as usize;
            data[idx] = (sr + (data[idx] as u32 * inv + 127) / 255).min(255) as u8;
            data[idx + 1] = (sg + (data[idx + 1] as u32 * inv + 127) / 255).min(255) as u8;
            data[idx + 2] = (sb + (data[idx + 2] as u32 * inv + 127) / 255).min(255) as u8;
            data[idx + 3] = (sa + (data[idx + 3] as u32 * inv + 127) / 255).min(255) as u8;
        }
    }
}

fn pixmap_to_straight_rgba(pixmap: &Pixmap) -> Vec<u8> {
    let src = pixmap.data();
    let mut out = vec![0u8; src.len()];
    for (dst_px, src_px) in out.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
        let a = src_px[3];
        if a == 0 {
            dst_px.copy_from_slice(&[0, 0, 0, 0]);
        } else if a == 255 {
            dst_px.copy_from_slice(src_px);
        } else {
            let a32 = a as u32;
            let half = a32 / 2;
            dst_px[0] = ((src_px[0] as u32 * 255 + half) / a32).min(255) as u8;
            dst_px[1] = ((src_px[1] as u32 * 255 + half) / a32).min(255) as u8;
            dst_px[2] = ((src_px[2] as u32 * 255 + half) / a32).min(255) as u8;
            dst_px[3] = a;
        }
    }
    out
}

fn strip_non_bmp(s: &str) -> String {
    // Keep standard text & BMP punctuation cleanly (avoid missing-glyph boxes when no color emoji font is loaded)
    s.chars()
        .filter(|&c| (c as u32) <= 0xFFFF && c != '\u{200D}' && c != '\u{2615}')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn truncate_words(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        return s.to_string();
    }
    let slice: String = chars[..max_chars].iter().collect();
    if let Some(pos) = slice.rfind(' ') {
        if pos > max_chars / 2 {
            return slice[..pos].trim_end().to_string();
        }
    }
    slice.trim_end().to_string()
}
