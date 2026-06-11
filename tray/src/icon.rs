// SPDX-License-Identifier: MIT
//! Render the tray icon: a rounded square, colour-coded by usage severity,
//! with the worst provider's percentage drawn in the middle. Output is the
//! ARGB32 (network byte order) buffer ksni expects.

use std::sync::Arc;

use ksni::Icon;

/// What the icon should show in the centre.
#[derive(Clone, Copy)]
pub enum IconState {
    /// A percentage 0..=100 and whether it is stale (last-known) data.
    Percent { value: f32, stale: bool },
    /// No usable data (loading, or every provider unconfigured/errored).
    NoData,
}

struct Rgb(u8, u8, u8);

fn severity_color(pct: f32) -> Rgb {
    if pct >= 80.0 {
        Rgb(0xE7, 0x4C, 0x3C) // red
    } else if pct >= 50.0 {
        Rgb(0xF1, 0xC4, 0x0F) // amber
    } else {
        Rgb(0x2E, 0xCC, 0x71) // green
    }
}

fn dim(Rgb(r, g, b): Rgb, factor: f32) -> Rgb {
    Rgb(
        (r as f32 * factor) as u8,
        (g as f32 * factor) as u8,
        (b as f32 * factor) as u8,
    )
}

/// Pick black or white text for legibility on the given background.
fn text_color(Rgb(r, g, b): &Rgb) -> Rgb {
    let lum = 0.299 * *r as f32 + 0.587 * *g as f32 + 0.114 * *b as f32;
    if lum > 150.0 {
        Rgb(0x20, 0x20, 0x20)
    } else {
        Rgb(0xFF, 0xFF, 0xFF)
    }
}

/// Render one icon at the given square size.
pub fn render(size: i32, state: IconState, font: Option<&Arc<fontdue::Font>>) -> Icon {
    let s = size.max(8) as usize;
    // Straight (non-premultiplied) ARGB stored as [A, R, G, B] per pixel.
    let mut buf = vec![0u8; s * s * 4];

    let (bg, label) = match state {
        IconState::Percent { value, stale } => {
            let mut c = severity_color(value);
            if stale {
                c = dim(c, 0.62);
            }
            (c, format!("{:.0}", value.clamp(0.0, 100.0)))
        }
        IconState::NoData => (Rgb(0x70, 0x78, 0x80), "–".to_string()),
    };

    draw_rounded_rect(&mut buf, s, &bg);

    if let Some(font) = font {
        draw_centered_text(&mut buf, s, &label, &text_color(&bg), font);
    }

    Icon {
        width: size,
        height: size,
        data: buf,
    }
}

/// Fill a rounded square (with 1px anti-aliased edge) in `color`.
fn draw_rounded_rect(buf: &mut [u8], s: usize, color: &Rgb) {
    let size = s as f32;
    let margin = size * 0.06;
    let radius = size * 0.24;
    let left = margin;
    let right = size - margin;
    let top = margin;
    let bottom = size - margin;
    let in_l = left + radius;
    let in_r = right - radius;
    let in_t = top + radius;
    let in_b = bottom - radius;

    for y in 0..s {
        for x in 0..s {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let qx = px.clamp(in_l, in_r);
            let qy = py.clamp(in_t, in_b);
            let dist = ((px - qx).powi(2) + (py - qy).powi(2)).sqrt();
            // coverage: full inside, ramp down across the last pixel of the edge.
            let cov = (radius - dist + 0.5).clamp(0.0, 1.0);
            if cov <= 0.0 {
                continue;
            }
            let i = (y * s + x) * 4;
            buf[i] = (cov * 255.0) as u8; // A
            buf[i + 1] = color.0; // R
            buf[i + 2] = color.1; // G
            buf[i + 3] = color.2; // B
        }
    }
}

/// Composite `text` centred over the already-filled background.
fn draw_centered_text(buf: &mut [u8], s: usize, text: &str, color: &Rgb, font: &fontdue::Font) {
    let size = s as f32;

    // Choose a pixel size: aim for ~66% of the icon height, then shrink if the
    // string is too wide (e.g. "100").
    let mut px = size * 0.80;
    let max_w = size * 0.90;
    let total_advance = |px: f32| -> f32 {
        text.chars()
            .map(|c| font.metrics(c, px).advance_width)
            .sum::<f32>()
    };
    let w = total_advance(px);
    if w > max_w {
        px *= max_w / w;
    }

    // Lay out glyphs. Use the first glyph's metrics to set the baseline so the
    // digits are vertically centred.
    let glyphs: Vec<(fontdue::Metrics, Vec<u8>)> =
        text.chars().map(|c| font.rasterize(c, px)).collect();
    if glyphs.is_empty() {
        return;
    }
    let block_w: f32 = glyphs.iter().map(|(m, _)| m.advance_width).sum();
    let (gh, gymin) = glyphs
        .iter()
        .map(|(m, _)| (m.height as f32, m.ymin as f32))
        .fold((0.0_f32, 0.0_f32), |(h, ym), (gh, gy)| {
            if gh > h {
                (gh, gy)
            } else {
                (h, ym)
            }
        });
    let baseline = (size + gh) / 2.0 + gymin;
    let mut pen_x = (size - block_w) / 2.0;

    for (m, bitmap) in &glyphs {
        let gx = pen_x + m.xmin as f32;
        let gy = baseline - m.ymin as f32 - m.height as f32;
        for row in 0..m.height {
            for col in 0..m.width {
                let cov = bitmap[row * m.width + col];
                if cov == 0 {
                    continue;
                }
                let tx = (gx + col as f32).round() as isize;
                let ty = (gy + row as f32).round() as isize;
                if tx < 0 || ty < 0 || tx as usize >= s || ty as usize >= s {
                    continue;
                }
                let i = (ty as usize * s + tx as usize) * 4;
                let a = cov as f32 / 255.0;
                // Blend text colour over the existing (straight) pixel.
                buf[i + 1] = blend(buf[i + 1], color.0, a);
                buf[i + 2] = blend(buf[i + 2], color.1, a);
                buf[i + 3] = blend(buf[i + 3], color.2, a);
                buf[i] = buf[i].max(cov); // keep the pixel at least as opaque as the text
            }
        }
        pen_x += m.advance_width;
    }
}

fn blend(bg: u8, fg: u8, a: f32) -> u8 {
    (bg as f32 * (1.0 - a) + fg as f32 * a).round().clamp(0.0, 255.0) as u8
}
