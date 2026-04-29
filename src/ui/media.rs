use super::ScReq;
use anyhow::Result;
use crossterm::{
    cursor::{MoveTo, RestorePosition, SavePosition},
    queue,
};
use image::{DynamicImage, GenericImageView, Rgb, RgbImage};
use ratatui::layout::Rect;
use std::collections::{HashMap, HashSet};
use std::env;
use std::io::{self, Write};
use std::sync::mpsc::Sender;

const CELL_PX: u32 = 10;
const ART_BG: [u8; 3] = [12, 14, 18];

pub struct Media {
    raw: HashMap<String, Vec<u8>>,
    bad: HashSet<String>,
    fly: HashSet<String>,
    enc: HashMap<String, Vec<u8>>,
    logo: Vec<u8>,
    sig: String,
    dirty: bool,
    on: bool,
}

impl Media {
    pub fn new() -> Self {
        Self {
            raw: HashMap::new(),
            bad: HashSet::new(),
            fly: HashSet::new(),
            enc: HashMap::new(),
            logo: include_bytes!("../../assets/sc_logo.png").to_vec(),
            sig: String::new(),
            dirty: true,
            on: sixel_on(),
        }
    }

    pub fn mark(&mut self) {
        self.dirty = true;
    }

    pub fn on(&self) -> bool {
        self.on
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
        if !self.on {
            return Ok(());
        }
        let sig = format!(
            "{}:{:?}:{:?}:{}",
            hide,
            cov.map(|(k, r)| format!("{k}:{}:{}:{}:{}", r.x, r.y, r.width, r.height)),
            logo.map(|r| format!("{}:{}:{}:{}", r.x, r.y, r.width, r.height)),
            sc_on
        );
        if !self.dirty && self.sig == sig {
            return Ok(());
        }
        self.sig = sig;
        self.dirty = false;

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
                }
            } else {
                self.draw_blank(rect)?;
            }
        }

        Ok(())
    }

    fn frame(&mut self, key: &str, rect: Rect) -> Option<Vec<u8>> {
        let ck = format!("cov:{key}:{}x{}", rect.width, rect.height);
        if let Some(buf) = self.enc.get(&ck) {
            return Some(buf.clone());
        }
        let dat = self.raw.get(key)?;
        let img = image::load_from_memory(dat).ok()?;
        let buf = enc_img(img, rect).ok()?;
        self.enc.insert(ck, buf.clone());
        Some(buf)
    }

    fn frame_logo(&mut self, rect: Rect) -> Option<Vec<u8>> {
        let ck = format!("logo:{}x{}", rect.width, rect.height);
        if let Some(buf) = self.enc.get(&ck) {
            return Some(buf.clone());
        }
        let img = image::load_from_memory(&self.logo).ok()?;
        let buf = enc_img(img, rect).ok()?;
        self.enc.insert(ck, buf.clone());
        Some(buf)
    }

    fn draw_blank(&self, rect: Rect) -> Result<()> {
        if rect.width < 1 || rect.height < 1 {
            return Ok(());
        }
        let (w, h) = canvas_dimensions(rect);
        let img = RgbImage::from_pixel(w.max(1), h.max(1), Rgb(ART_BG));
        let six = enc_six(img.as_raw(), w as usize, h as usize, 64);
        self.out(rect, six.as_bytes())
    }

    fn draw_frame(&self, rect: Rect, buf: &[u8]) -> Result<()> {
        self.out(rect, buf)
    }

    fn out(&self, rect: Rect, buf: &[u8]) -> Result<()> {
        let mut out = io::stdout();
        queue!(out, SavePosition, MoveTo(rect.x, rect.y))?;
        out.write_all(b"\x1b7")?;
        out.write_all(buf)?;
        out.write_all(b"\x1b8")?;
        queue!(out, RestorePosition)?;
        out.flush()?;
        Ok(())
    }
}

fn enc_img(img: DynamicImage, rect: Rect) -> Result<Vec<u8>> {
    let rgb = fit_to_canvas(img, rect);
    let (w, h) = rgb.dimensions();
    let six = enc_six(rgb.as_raw(), w as usize, h as usize, 256);
    let mut out = Vec::with_capacity(six.len() + 8);
    out.extend_from_slice(b"\x1bPq");
    out.extend_from_slice(six.as_bytes());
    out.extend_from_slice(b"\x1b\\");
    Ok(out)
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
    (
        (rect.width.max(1) as u32) * CELL_PX,
        (rect.height.max(1) as u32) * CELL_PX,
    )
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

    if env::var_os("WT_SESSION").is_some() {
        return true;
    }

    false
}
