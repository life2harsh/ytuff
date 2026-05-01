use super::ScReq;
use anyhow::Result;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use crossterm::{
    cursor::{RestorePosition, SavePosition},
    queue,
};
use image::{DynamicImage, GenericImageView, ImageOutputFormat, Rgb, RgbImage};
use ratatui::layout::Rect;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{self, Cursor, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::{SystemTime, UNIX_EPOCH};

const ART_BG: [u8; 3] = [12, 14, 18];
const GRAPHIC_ART_MAX_EDGE: u32 = 640;
const KITTY_COVER_IMAGE_ID: u32 = 2001;
const KITTY_LOGO_IMAGE_ID: u32 = 2002;
const KITTY_PLACEMENT_ID: u32 = 1;
const KITTY_CHUNK_SIZE: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArtRenderer {
    Off,
    Kitty,
    Sixel,
    Wimg,
    Blocks,
}

impl ArtRenderer {
    fn from_env() -> Self {
        if let Ok(value) = env::var("RUSTPLAYER_ART") {
            let value = value.trim().to_ascii_lowercase();
            return match value.as_str() {
                "off" | "0" | "false" => Self::Off,
                "kitty" => Self::Kitty,
                "sixel" => Self::Sixel,
                "1" | "true" | "yes" | "on" | "wimg" => Self::Wimg,
                "blocks" | "block" => Self::Blocks,
                _ => Self::auto_detect(),
            };
        }

        Self::auto_detect()
    }

    fn auto_detect() -> Self {
        if kitty_on() {
            Self::Kitty
        } else if sixel_on() && wimg_on() {
            Self::Wimg
        } else {
            Self::Blocks
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Kitty => "kitty",
            Self::Sixel => "sixel",
            Self::Wimg => "wimg",
            Self::Blocks => "blocks",
        }
    }
}

pub struct Media {
    raw: HashMap<String, Vec<u8>>,
    bad: HashSet<String>,
    fly: HashSet<String>,
    enc: HashMap<String, Vec<u8>>,
    logo: Vec<u8>,
    sig: String,
    dirty: bool,
    renderer: ArtRenderer,
    renderer_note: Option<String>,
    last_cov: Option<Rect>,
    last_logo: Option<Rect>,
}

impl Media {
    pub fn new() -> Self {
        let requested = ArtRenderer::from_env();
        let kitty_ready = kitty_on();
        let sixel_ready = sixel_on();
        let wimg_ready = wimg_on();
        let (renderer, renderer_note) = match requested {
            ArtRenderer::Off => (ArtRenderer::Off, None),
            ArtRenderer::Kitty if kitty_ready => (ArtRenderer::Kitty, Some("protocol".to_string())),
            ArtRenderer::Kitty => (ArtRenderer::Blocks, Some("kitty unavailable".to_string())),
            ArtRenderer::Sixel if sixel_ready => (ArtRenderer::Sixel, None),
            ArtRenderer::Sixel => (ArtRenderer::Blocks, Some("sixel unavailable".to_string())),
            ArtRenderer::Wimg if wimg_ready && sixel_ready => {
                (ArtRenderer::Wimg, Some("renderer".to_string()))
            }
            ArtRenderer::Wimg if !wimg_ready => {
                (ArtRenderer::Blocks, Some("wimg unavailable".to_string()))
            }
            ArtRenderer::Wimg => (ArtRenderer::Blocks, Some("sixel unavailable".to_string())),
            ArtRenderer::Blocks => (ArtRenderer::Blocks, None),
        };
        Self {
            raw: HashMap::new(),
            bad: HashSet::new(),
            fly: HashSet::new(),
            enc: HashMap::new(),
            logo: include_bytes!("../../assets/sc_logo.png").to_vec(),
            sig: String::new(),
            dirty: true,
            renderer,
            renderer_note,
            last_cov: None,
            last_logo: None,
        }
    }

    pub fn mark(&mut self) {
        self.dirty = true;
    }

    pub fn on(&self) -> bool {
        self.renderer != ArtRenderer::Off
    }

    pub fn renderer_label(&self) -> String {
        if let Some(note) = self.renderer_note.as_ref() {
            return format!("{} ({})", self.renderer.label(), note);
        }
        self.renderer.label().to_string()
    }

    pub fn want(&mut self, key: &str, url: &str, tx: &Sender<ScReq>) {
        if self.raw.contains_key(key) || self.bad.contains(key) || self.fly.contains(key) {
            return;
        }
        self.fly.insert(key.to_string());
        let _ = tx.send(ScReq::Art(key.to_string(), url.to_string()));
    }

    pub fn put(&mut self, key: String, dat: Result<Vec<u8>, String>) {
        self.fly.remove(&key);
        match dat {
            Ok(dat) => {
                let dat = match self.renderer {
                    ArtRenderer::Kitty | ArtRenderer::Sixel | ArtRenderer::Wimg => {
                        shrink_art_bytes(dat)
                    }
                    ArtRenderer::Off | ArtRenderer::Blocks => dat,
                };
                self.enc
                    .retain(|cache_key, _| !cache_key.contains(&format!(":cov:{key}:")));
                self.raw.insert(key, dat);
            }
            Err(_) => {
                self.bad.insert(key);
            }
        }
        self.dirty = true;
    }

    pub fn art_bytes(&self, key: &str) -> Option<Vec<u8>> {
        self.raw.get(key).cloned()
    }

    pub fn draw(
        &mut self,
        cov: Option<(&str, Rect)>,
        logo: Option<Rect>,
        sc_on: bool,
        hide: bool,
    ) -> Result<()> {
        if self.renderer == ArtRenderer::Off {
            return Ok(());
        }
        let logo = if sc_on { logo } else { None };
        let sig = format!(
            "{}:{:?}:{:?}:{}:{}",
            hide,
            cov.map(|(k, r)| format!("{k}:{}:{}:{}:{}", r.x, r.y, r.width, r.height)),
            logo.map(|r| format!("{}:{}:{}:{}", r.x, r.y, r.width, r.height)),
            sc_on,
            self.renderer.label(),
        );
        if !self.dirty && self.sig == sig {
            return Ok(());
        }
        self.sig = sig;
        self.dirty = false;

        if self.renderer == ArtRenderer::Kitty {
            self.clear_kitty()?;

            if let Some((key, rect)) = cov {
                if !hide {
                    if let Some(frame) = self.frame(key, rect) {
                        self.draw_frame(rect, &frame)?;
                    }
                }
            }

            if let Some(rect) = logo {
                if sc_on && !hide {
                    if let Some(frame) = self.frame_logo(rect) {
                        self.draw_frame(rect, &frame)?;
                    }
                }
            }

            self.last_cov = cov.map(|(_, rect)| rect);
            self.last_logo = logo;
            return Ok(());
        }

        let next_cov = cov.map(|(_, rect)| rect);
        let next_logo = logo;
        if let Some(rect) = self.last_cov.filter(|prev| Some(*prev) != next_cov) {
            self.draw_blank(rect)?;
        }
        if let Some(rect) = self.last_logo.filter(|prev| Some(*prev) != next_logo) {
            self.draw_blank(rect)?;
        }

        if let Some((key, rect)) = cov {
            if hide {
                self.draw_blank(rect)?;
            } else if let Some(frame) = self.frame(key, rect) {
                self.draw_frame(rect, &frame)?;
            } else {
                self.draw_blank(rect)?;
            }
        }

        if let Some(rect) = logo {
            if sc_on && !hide {
                if let Some(frame) = self.frame_logo(rect) {
                    self.draw_frame(rect, &frame)?;
                } else {
                    self.draw_blank(rect)?;
                }
            } else {
                self.draw_blank(rect)?;
            }
        }

        self.last_cov = next_cov;
        self.last_logo = next_logo;

        Ok(())
    }

    fn frame(&mut self, key: &str, rect: Rect) -> Option<Vec<u8>> {
        match self.renderer {
            ArtRenderer::Off => None,
            ArtRenderer::Kitty => self.frame_kitty(key, rect),
            ArtRenderer::Sixel => self.frame_sixel(key, rect),
            ArtRenderer::Blocks => self.frame_blocks(key, rect),
            ArtRenderer::Wimg => self.frame_wimg(key, rect),
        }
    }

    fn frame_logo(&mut self, rect: Rect) -> Option<Vec<u8>> {
        match self.renderer {
            ArtRenderer::Off => None,
            ArtRenderer::Kitty => self.frame_logo_kitty(rect),
            ArtRenderer::Sixel => self.frame_logo_sixel(rect),
            ArtRenderer::Wimg => self.frame_logo_wimg(rect),
            ArtRenderer::Blocks => self.frame_logo_blocks(rect),
        }
    }

    fn draw_blank(&self, rect: Rect) -> Result<()> {
        match self.renderer {
            ArtRenderer::Off => Ok(()),
            ArtRenderer::Kitty => Ok(()),
            ArtRenderer::Sixel => self.draw_blank_sixel(rect),
            ArtRenderer::Wimg => self.draw_blank_sixel(rect),
            ArtRenderer::Blocks => self.draw_blank_blocks(rect),
        }
    }

    fn frame_kitty(&mut self, key: &str, rect: Rect) -> Option<Vec<u8>> {
        let ck = format!("kitty:cov:{key}:{}x{}", rect.width, rect.height);
        if let Some(buf) = self.enc.get(&ck) {
            return Some(buf.clone());
        }
        let dat = self.raw.get(key)?;
        let img = image::load_from_memory(dat).ok()?;
        let buf = enc_kitty(img, rect, KITTY_COVER_IMAGE_ID, KITTY_PLACEMENT_ID).ok()?;
        self.enc.insert(ck, buf.clone());
        Some(buf)
    }

    fn frame_sixel(&mut self, key: &str, rect: Rect) -> Option<Vec<u8>> {
        let ck = format!("sixel:cov:{key}:{}x{}", rect.width, rect.height);
        if let Some(buf) = self.enc.get(&ck) {
            return Some(buf.clone());
        }
        let dat = self.raw.get(key)?;
        let img = image::load_from_memory(dat).ok()?;
        let buf = enc_img(img, rect).ok()?;
        self.enc.insert(ck, buf.clone());
        Some(buf)
    }

    fn frame_logo_sixel(&mut self, rect: Rect) -> Option<Vec<u8>> {
        let ck = format!("sixel:logo:{}x{}", rect.width, rect.height);
        if let Some(buf) = self.enc.get(&ck) {
            return Some(buf.clone());
        }
        let img = image::load_from_memory(&self.logo).ok()?;
        let buf = enc_img(img, rect).ok()?;
        self.enc.insert(ck, buf.clone());
        Some(buf)
    }

    fn frame_blocks(&mut self, key: &str, rect: Rect) -> Option<Vec<u8>> {
        let ck = format!("blocks:cov:{key}:{}x{}", rect.width, rect.height);
        if let Some(buf) = self.enc.get(&ck) {
            return Some(buf.clone());
        }
        let dat = self.raw.get(key)?;
        let img = image::load_from_memory(dat).ok()?;
        let buf = enc_blocks(img, rect);
        self.enc.insert(ck, buf.clone());
        Some(buf)
    }

    fn frame_wimg(&mut self, key: &str, rect: Rect) -> Option<Vec<u8>> {
        let ck = format!("wimg:cov:{key}:{}x{}", rect.width, rect.height);
        if let Some(buf) = self.enc.get(&ck) {
            return Some(buf.clone());
        }
        let dat = self.raw.get(key)?;
        let img = image::load_from_memory(dat).ok()?;
        let buf = enc_wimg(img, rect).ok()?;
        self.enc.insert(ck, buf.clone());
        Some(buf)
    }

    fn frame_logo_kitty(&mut self, rect: Rect) -> Option<Vec<u8>> {
        let ck = format!("kitty:logo:{}x{}", rect.width, rect.height);
        if let Some(buf) = self.enc.get(&ck) {
            return Some(buf.clone());
        }
        let img = image::load_from_memory(&self.logo).ok()?;
        let buf = enc_kitty(img, rect, KITTY_LOGO_IMAGE_ID, KITTY_PLACEMENT_ID).ok()?;
        self.enc.insert(ck, buf.clone());
        Some(buf)
    }

    fn frame_logo_blocks(&mut self, rect: Rect) -> Option<Vec<u8>> {
        let ck = format!("blocks:logo:{}x{}", rect.width, rect.height);
        if let Some(buf) = self.enc.get(&ck) {
            return Some(buf.clone());
        }
        let img = image::load_from_memory(&self.logo).ok()?;
        let buf = enc_blocks(img, rect);
        self.enc.insert(ck, buf.clone());
        Some(buf)
    }

    fn frame_logo_wimg(&mut self, rect: Rect) -> Option<Vec<u8>> {
        let ck = format!("wimg:logo:{}x{}", rect.width, rect.height);
        if let Some(buf) = self.enc.get(&ck) {
            return Some(buf.clone());
        }
        let img = image::load_from_memory(&self.logo).ok()?;
        let buf = enc_wimg(img, rect).ok()?;
        self.enc.insert(ck, buf.clone());
        Some(buf)
    }

    fn draw_blank_sixel(&self, rect: Rect) -> Result<()> {
        if rect.width < 1 || rect.height < 1 {
            return Ok(());
        }
        let (w, h) = canvas_dimensions(rect);
        let img = RgbImage::from_pixel(w.max(1), h.max(1), Rgb(ART_BG));
        let six = wrap_sixel(enc_six(img.as_raw(), w as usize, h as usize, 64));
        self.out_graphic(rect, &six)
    }

    fn draw_blank_blocks(&self, rect: Rect) -> Result<()> {
        if rect.width < 1 || rect.height < 1 {
            return Ok(());
        }
        let buf = enc_blank_blocks(rect);
        self.out_blocks(rect, &buf)
    }

    fn draw_frame(&self, rect: Rect, buf: &[u8]) -> Result<()> {
        match self.renderer {
            ArtRenderer::Off => Ok(()),
            ArtRenderer::Blocks => self.out_blocks(rect, buf),
            ArtRenderer::Kitty | ArtRenderer::Sixel | ArtRenderer::Wimg => {
                self.out_graphic(rect, buf)
            }
        }
    }

    fn clear_kitty(&self) -> Result<()> {
        let mut out = io::stdout();
        queue!(out, SavePosition)?;
        out.write_all(&kitty_delete(KITTY_COVER_IMAGE_ID, KITTY_PLACEMENT_ID))?;
        out.write_all(&kitty_delete(KITTY_LOGO_IMAGE_ID, KITTY_PLACEMENT_ID))?;
        queue!(out, RestorePosition)?;
        out.flush()?;
        Ok(())
    }

    fn out_blocks(&self, rect: Rect, buf: &[u8]) -> Result<()> {
        let mut out = io::stdout();
        queue!(out, SavePosition)?;
        let base_x = rect.x + 1;
        let base_y = rect.y + 1;

        for (row, line) in buf.split(|byte| *byte == b'\n').enumerate() {
            if row >= rect.height as usize {
                break;
            }
            out.write_all(&format!("\x1b[{};{}H", base_y + row as u16, base_x).into_bytes())?;
            out.write_all(line)?;
        }

        queue!(out, RestorePosition)?;
        out.flush()?;
        Ok(())
    }

    fn out_graphic(&self, rect: Rect, buf: &[u8]) -> Result<()> {
        if rect.width < 1 || rect.height < 1 {
            return Ok(());
        }
        let mut out = io::stdout();
        queue!(out, SavePosition)?;
        out.write_all(&format!("\x1b[{};{}H", rect.y + 1, rect.x + 1).into_bytes())?;
        out.write_all(buf)?;
        queue!(out, RestorePosition)?;
        out.flush()?;
        Ok(())
    }
}

fn enc_img(img: DynamicImage, rect: Rect) -> Result<Vec<u8>> {
    let rgb = fit_to_canvas(img, rect);
    let (w, h) = rgb.dimensions();
    Ok(wrap_sixel(enc_six(
        rgb.as_raw(),
        w as usize,
        h as usize,
        256,
    )))
}

fn enc_blocks(img: DynamicImage, rect: Rect) -> Vec<u8> {
    let rgb = fit_to_cells(img, rect);
    let (w, h) = rgb.dimensions();
    if w == 0 || h == 0 {
        return Vec::new();
    }

    let mut out = String::new();
    let mut y = 0;
    while y < h {
        let mut x = 0;
        while x < w {
            let pixels = [
                rgb.get_pixel(x, y).0,
                pixel_or_bg(&rgb, x + 1, y),
                pixel_or_bg(&rgb, x, y + 1),
                pixel_or_bg(&rgb, x + 1, y + 1),
            ];
            let (glyph, fg, bg) = quadrant_cell(&pixels);
            if glyph == ' ' {
                out.push_str(&format!("\x1b[48;2;{};{};{}m ", bg[0], bg[1], bg[2]));
            } else {
                out.push_str(&format!(
                    "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m{}",
                    fg[0], fg[1], fg[2], bg[0], bg[1], bg[2], glyph
                ));
            }
            x += 2;
        }
        out.push_str("\x1b[0m");
        y += 2;
        if y < h {
            out.push('\n');
        }
    }

    out.into_bytes()
}

fn enc_wimg(img: DynamicImage, rect: Rect) -> Result<Vec<u8>> {
    let rgb = fit_to_canvas(img, rect);
    let path = temp_wimg_path();
    image::DynamicImage::ImageRgb8(rgb).save(&path)?;

    let output = Command::new("wimg")
        .arg(&path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    let _ = fs::remove_file(&path);

    if !output.status.success() {
        let msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if msg.is_empty() {
            anyhow::bail!("wimg exited with status {}", output.status);
        }
        anyhow::bail!("wimg failed: {}", msg);
    }

    if output.stdout.is_empty() {
        anyhow::bail!("wimg returned no image bytes");
    }

    Ok(normalize_graphic(output.stdout))
}

fn enc_kitty(img: DynamicImage, rect: Rect, image_id: u32, placement_id: u32) -> Result<Vec<u8>> {
    let rgb = fit_to_canvas(img, rect);
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(rgb)
        .write_to(&mut Cursor::new(&mut png), ImageOutputFormat::Png)?;
    Ok(wrap_kitty_png(png, rect, image_id, placement_id))
}

fn enc_blank_blocks(rect: Rect) -> Vec<u8> {
    let w = rect.width.max(1) as usize;
    let h = rect.height.max(1) as usize;
    let mut out = String::new();
    let fill = format!(
        "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m▀",
        ART_BG[0], ART_BG[1], ART_BG[2], ART_BG[0], ART_BG[1], ART_BG[2]
    );
    for y in 0..h {
        for _ in 0..w {
            out.push_str(&fill);
        }
        out.push_str("\x1b[0m");
        if y + 1 < h {
            out.push('\n');
        }
    }
    out.into_bytes()
}

fn shrink_art_bytes(dat: Vec<u8>) -> Vec<u8> {
    let Ok(img) = image::load_from_memory(&dat) else {
        return dat;
    };
    let (w, h) = img.dimensions();
    if w.max(h) <= GRAPHIC_ART_MAX_EDGE {
        return dat;
    }

    let resized = img.resize(
        GRAPHIC_ART_MAX_EDGE,
        GRAPHIC_ART_MAX_EDGE,
        image::imageops::FilterType::Triangle,
    );
    let mut out = Vec::new();
    if resized
        .write_to(&mut Cursor::new(&mut out), ImageOutputFormat::Png)
        .is_ok()
        && !out.is_empty()
    {
        out
    } else {
        dat
    }
}

fn fit_to_canvas(img: DynamicImage, rect: Rect) -> RgbImage {
    let (canvas_w, canvas_h) = canvas_dimensions(rect);
    let mut canvas = RgbImage::from_pixel(canvas_w.max(1), canvas_h.max(1), Rgb(ART_BG));
    let fitted = fit(img, canvas_w, canvas_h).to_rgb8();
    let (w, h) = fitted.dimensions();
    let x = canvas_w.saturating_sub(w) / 2;
    let y = canvas_h.saturating_sub(h) / 2;
    image::imageops::overlay(&mut canvas, &fitted, x.into(), y.into());
    canvas
}

fn fit_to_cells(img: DynamicImage, rect: Rect) -> RgbImage {
    let (canvas_w, canvas_h) = cell_dimensions(rect);
    let mut canvas = RgbImage::from_pixel(canvas_w.max(1), canvas_h.max(1), Rgb(ART_BG));
    let fitted = fit(img, canvas_w, canvas_h).to_rgb8();
    let (w, h) = fitted.dimensions();
    let x = canvas_w.saturating_sub(w) / 2;
    let y = canvas_h.saturating_sub(h) / 2;
    image::imageops::overlay(&mut canvas, &fitted, x.into(), y.into());
    canvas
}

fn fit(img: DynamicImage, max_w: u32, max_h: u32) -> DynamicImage {
    let (w, h) = img.dimensions();
    let asp = w as f32 / h as f32;
    let max_w = max_w as f32;
    let max_h = max_h as f32;
    let (mut nw, mut nh) = if max_w / max_h > asp {
        let nh = max_h.min(h as f32);
        let nw = nh * asp;
        (nw, nh)
    } else {
        let nw = max_w.min(w as f32);
        let nh = nw / asp;
        (nw, nh)
    };
    if nw > w as f32 || nh > h as f32 {
        nw = w as f32;
        nh = h as f32;
    }
    img.resize(
        nw.max(1.0).round() as u32,
        nh.max(1.0).round() as u32,
        image::imageops::FilterType::Lanczos3,
    )
}

fn canvas_dimensions(rect: Rect) -> (u32, u32) {
    let (cell_w, cell_h) = art_cell_pixels();
    (
        (rect.width.max(1) as u32) * cell_w,
        (rect.height.max(1) as u32) * cell_h,
    )
}

fn cell_dimensions(rect: Rect) -> (u32, u32) {
    // Render each terminal cell as a 2x2 color quadrant to keep the fallback
    // path smooth and broadly compatible across terminal fonts.
    (rect.width.max(1) as u32 * 2, rect.height.max(1) as u32 * 2)
}

fn pixel_or_bg(rgb: &RgbImage, x: u32, y: u32) -> [u8; 3] {
    if x < rgb.width() && y < rgb.height() {
        rgb.get_pixel(x, y).0
    } else {
        ART_BG
    }
}

fn quadrant_cell(pixels: &[[u8; 3]; 4]) -> (char, [u8; 3], [u8; 3]) {
    let mut best_mask = 0u8;
    let mut best_fg = avg_rgb4(pixels, 0b1111);
    let mut best_bg = best_fg;
    let mut best_err = u32::MAX;

    for mask in 0u8..=0b1111 {
        let fg = avg_rgb4(pixels, mask);
        let bg = avg_rgb4(pixels, !mask & 0b1111);
        let mut err = 0u32;

        for (idx, pixel) in pixels.iter().enumerate() {
            let target = if mask & (1 << idx) != 0 { fg } else { bg };
            err = err.saturating_add(rgb_dist2(*pixel, target));
        }

        if err < best_err {
            best_err = err;
            best_mask = mask;
            best_fg = fg;
            best_bg = bg;
        }
    }

    (quadrant_glyph(best_mask), best_fg, best_bg)
}

fn avg_rgb4(pixels: &[[u8; 3]; 4], mask: u8) -> [u8; 3] {
    let mut sum = [0u32; 3];
    let mut count = 0u32;

    for (idx, pixel) in pixels.iter().enumerate() {
        if mask & (1 << idx) == 0 {
            continue;
        }
        sum[0] += pixel[0] as u32;
        sum[1] += pixel[1] as u32;
        sum[2] += pixel[2] as u32;
        count += 1;
    }

    if count == 0 {
        return ART_BG;
    }

    [
        (sum[0] / count) as u8,
        (sum[1] / count) as u8,
        (sum[2] / count) as u8,
    ]
}

fn rgb_dist2(a: [u8; 3], b: [u8; 3]) -> u32 {
    let dr = a[0] as i32 - b[0] as i32;
    let dg = a[1] as i32 - b[1] as i32;
    let db = a[2] as i32 - b[2] as i32;
    (dr * dr + dg * dg + db * db) as u32
}

fn quadrant_glyph(mask: u8) -> char {
    match mask & 0b1111 {
        0b0000 => ' ',
        0b0001 => '▘',
        0b0010 => '▝',
        0b0011 => '▀',
        0b0100 => '▖',
        0b0101 => '▌',
        0b0110 => '▞',
        0b0111 => '▛',
        0b1000 => '▗',
        0b1001 => '▚',
        0b1010 => '▐',
        0b1011 => '▜',
        0b1100 => '▄',
        0b1101 => '▙',
        0b1110 => '▟',
        _ => '█',
    }
}

fn enc_six(rgb: &[u8], w: usize, h: usize, max_pal: usize) -> String {
    let mut pal: Vec<(u8, u8, u8)> = Vec::new();
    let mut map = HashMap::<(u8, u8, u8), u8>::new();
    for px in rgb.chunks(3) {
        if px.len() < 3 {
            continue;
        }
        let c = (px[0], px[1], px[2]);
        if map.contains_key(&c) {
            continue;
        }
        if pal.len() >= max_pal {
            break;
        }
        let idx = pal.len() as u8;
        pal.push(c);
        map.insert(c, idx);
    }
    let mut idxs = Vec::with_capacity(w * h);
    for px in rgb.chunks(3) {
        if px.len() < 3 {
            continue;
        }
        let c = (px[0], px[1], px[2]);
        let idx = *map.get(&c).unwrap_or(&near(&pal, c));
        idxs.push(idx);
    }
    let mut out = String::new();
    out.push('"');
    out.push_str("0;0;");
    out.push_str(&w.to_string());
    out.push(';');
    out.push_str(&h.to_string());
    for (i, (r, g, b)) in pal.iter().enumerate() {
        let pr = (*r as f32 / 255.0 * 100.0).round() as u8;
        let pg = (*g as f32 / 255.0 * 100.0).round() as u8;
        let pb = (*b as f32 / 255.0 * 100.0).round() as u8;
        out.push_str(&format!("#{};2;{};{};{}", i, pr, pg, pb));
    }
    for ci in 0..pal.len() {
        out.push_str(&format!("#{}", ci));
        let mut y = 0;
        while y < h {
            let mut x = 0;
            let mut seen = false;
            while x < w {
                let mut bits = 0u8;
                for bit in 0..6 {
                    let yy = y + bit;
                    if yy >= h {
                        break;
                    }
                    let idx = yy * w + x;
                    if idxs[idx] as usize == ci {
                        bits |= 1 << bit;
                    }
                }
                if bits != 0 {
                    out.push((63 + bits) as char);
                    seen = true;
                } else if seen {
                    out.push('?');
                }
                x += 1;
            }
            out.push('$');
            y += 6;
        }
        out.push('-');
    }
    out.push('-');
    out
}

fn near(pal: &[(u8, u8, u8)], c: (u8, u8, u8)) -> u8 {
    if pal.is_empty() {
        return 0;
    }
    let (tr, tg, tb) = c;
    let mut best = 0usize;
    let mut dist = u32::MAX;
    for (i, (r, g, b)) in pal.iter().enumerate() {
        let dr = *r as i32 - tr as i32;
        let dg = *g as i32 - tg as i32;
        let db = *b as i32 - tb as i32;
        let d = (dr * dr + dg * dg + db * db) as u32;
        if d < dist {
            dist = d;
            best = i;
        }
    }
    best as u8
}

fn sixel_on() -> bool {
    if let Ok(v) = env::var("RUSTPLAYER_SIXEL") {
        let v = v.trim().to_ascii_lowercase();
        if matches!(v.as_str(), "1" | "true" | "yes" | "on") {
            return true;
        }
        if matches!(v.as_str(), "0" | "false" | "no" | "off") {
            return false;
        }
    }

    if let Ok(term) = env::var("TERM") {
        let term = term.to_ascii_lowercase();
        if term.contains("sixel") || term.contains("mlterm") {
            return true;
        }
    }

    if let Ok(prog) = env::var("TERM_PROGRAM") {
        if prog.eq_ignore_ascii_case("wezterm") {
            return true;
        }
    }

    false
}

fn kitty_on() -> bool {
    if let Ok(v) = env::var("RUSTPLAYER_KITTY") {
        let v = v.trim().to_ascii_lowercase();
        if matches!(v.as_str(), "1" | "true" | "yes" | "on") {
            return true;
        }
        if matches!(v.as_str(), "0" | "false" | "no" | "off") {
            return false;
        }
    }

    if env::var_os("KITTY_WINDOW_ID").is_some() || env::var_os("KITTY_PID").is_some() {
        return true;
    }

    if env::var_os("WEZTERM_EXECUTABLE").is_some() || env::var_os("GHOSTTY_RESOURCES_DIR").is_some()
    {
        return true;
    }

    if let Ok(term) = env::var("TERM") {
        let term = term.to_ascii_lowercase();
        if term.contains("kitty") {
            return true;
        }
    }

    if let Ok(prog) = env::var("TERM_PROGRAM") {
        if prog.eq_ignore_ascii_case("wezterm") || prog.eq_ignore_ascii_case("ghostty") {
            return true;
        }
    }

    false
}

fn art_cell_pixels() -> (u32, u32) {
    let (default_w, default_h) = if cfg!(windows) { (8, 16) } else { (10, 20) };
    let w = env_u32("RUSTPLAYER_ART_CELL_W").unwrap_or(default_w).max(1);
    let h = env_u32("RUSTPLAYER_ART_CELL_H").unwrap_or(default_h).max(1);
    (w, h)
}

fn env_u32(name: &str) -> Option<u32> {
    env::var(name).ok()?.trim().parse().ok()
}

fn wrap_sixel(six: String) -> Vec<u8> {
    let mut out = Vec::with_capacity(six.len() + 8);
    out.extend_from_slice(b"\x1bPq");
    out.extend_from_slice(six.as_bytes());
    out.extend_from_slice(b"\x1b\\");
    out
}

fn wrap_kitty_png(png: Vec<u8>, rect: Rect, image_id: u32, placement_id: u32) -> Vec<u8> {
    let encoded = BASE64.encode(png);
    let total_chunks = encoded.len().div_ceil(KITTY_CHUNK_SIZE);
    let mut out = Vec::with_capacity(encoded.len() + total_chunks.saturating_mul(64));

    for (index, chunk) in encoded.as_bytes().chunks(KITTY_CHUNK_SIZE).enumerate() {
        let more = if index + 1 < total_chunks { 1 } else { 0 };
        if index == 0 {
            out.extend_from_slice(
                format!(
                    "\x1b_Ga=T,q=2,f=100,i={image_id},p={placement_id},c={},r={},m={more};",
                    rect.width.max(1),
                    rect.height.max(1)
                )
                .as_bytes(),
            );
        } else {
            out.extend_from_slice(format!("\x1b_Gq=2,m={more};").as_bytes());
        }
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }

    out
}

fn kitty_delete(image_id: u32, placement_id: u32) -> Vec<u8> {
    format!("\x1b_Ga=d,d=I,q=2,i={image_id},p={placement_id}\x1b\\").into_bytes()
}

fn normalize_graphic(buf: Vec<u8>) -> Vec<u8> {
    if buf.starts_with(b"\x1bP") {
        return buf;
    }
    if buf.starts_with(b"\"") || buf.starts_with(b"#") {
        let mut out = Vec::with_capacity(buf.len() + 8);
        out.extend_from_slice(b"\x1bPq");
        out.extend_from_slice(&buf);
        out.extend_from_slice(b"\x1b\\");
        return out;
    }
    buf
}

fn wimg_on() -> bool {
    Command::new("wimg")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

fn temp_wimg_path() -> std::path::PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let mut path = std::env::temp_dir();
    path.push(format!(
        "rustplayer-wimg-{}-{stamp}.png",
        std::process::id()
    ));
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kitty_wrap_includes_metadata_and_final_chunk() {
        let payload = vec![42u8; 8000];
        let rect = Rect::new(0, 0, 12, 6);
        let wrapped = String::from_utf8(wrap_kitty_png(payload, rect, 77, 9)).unwrap();

        assert!(wrapped.contains("\x1b_Ga=T,q=2,f=100,i=77,p=9,c=12,r=6,m=1;"));
        assert!(wrapped.contains("\x1b_Gq=2,m=0;"));
        assert!(wrapped.ends_with("\x1b\\"));
    }

    #[test]
    fn kitty_delete_targets_specific_image_and_placement() {
        let delete = String::from_utf8(kitty_delete(77, 9)).unwrap();
        assert_eq!(delete, "\x1b_Ga=d,d=I,q=2,i=77,p=9\x1b\\");
    }
}
