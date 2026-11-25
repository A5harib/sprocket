use anyhow::Result;
use cosmic_text::{FontSystem, Weight};
use std::fs;
use std::path::Path;

pub struct FontStore {
    pub font_system: FontSystem,
    pub display_family: String,
    pub sans_family: String,
    pub mono_family: String,
    pub serif_family: String,
}

impl FontStore {
    pub fn load(assets_dir: &Path) -> Result<Self> {
        let mut db = cosmic_text::fontdb::Database::new();

        // Load project fonts (TTF, OTF, and WOFF2 decompressed via pure-Rust wuff)
        let fonts_dir = assets_dir.join("fonts");
        if fonts_dir.is_dir() {
            for entry in fs::read_dir(&fonts_dir)? {
                let entry = entry?;
                let path = entry.path();
                let ext = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if let Ok(bytes) = fs::read(&path) {
                    match ext.as_str() {
                        "woff2" => {
                            if let Ok(ttf) = wuff::decompress_woff2(&bytes) {
                                db.load_font_data(ttf);
                            }
                        }
                        "woff" => {
                            if let Ok(ttf) = wuff::decompress_woff1(&bytes) {
                                db.load_font_data(ttf);
                            }
                        }
                        "ttf" | "otf" => {
                            db.load_font_data(bytes);
                        }
                        _ => {}
                    }
                }
            }
        }

        // Load clean Windows system fallback fonts (Segoe UI, Arial, Georgia) without scanning 400+ files
        let win_fonts = Path::new(r"C:\Windows\Fonts");
        for fname in [
            "segoeui.ttf",
            "segoeuib.ttf",
            "segoeuii.ttf",
            "segoeuisl.ttf",
            "arial.ttf",
            "arialbd.ttf",
            "georgia.ttf",
            "georgiai.ttf",
            "consola.ttf",
            "consolab.ttf",
        ] {
            let p = win_fonts.join(fname);
            if let Ok(bytes) = fs::read(&p) {
                db.load_font_data(bytes);
            }
        }

        let has_family = |name: &str| {
            db.faces()
                .any(|f| f.families.iter().any(|(fam, _)| fam.eq_ignore_ascii_case(name)))
        };

        let display_family = if has_family("Big Shoulders Display") {
            "Big Shoulders Display"
        } else {
            "Segoe UI"
        }
        .to_string();

        let sans_family = if has_family("Segoe UI") {
            "Segoe UI"
        } else if has_family("IBM Plex Sans") {
            "IBM Plex Sans"
        } else {
            "Arial"
        }
        .to_string();

        let mono_family = if has_family("IBM Plex Mono") {
            "IBM Plex Mono"
        } else {
            "Consolas"
        }
        .to_string();

        let serif_family = if has_family("EB Garamond") {
            "EB Garamond"
        } else if has_family("Georgia") {
            "Georgia"
        } else {
            "Segoe UI"
        }
        .to_string();

        let font_system = FontSystem::new_with_locale_and_db("en-US".to_string(), db);
        Ok(Self {
            font_system,
            display_family,
            sans_family,
            mono_family,
            serif_family,
        })
    }

    pub fn weight(w: u16) -> Weight {
        Weight(w)
    }
}
