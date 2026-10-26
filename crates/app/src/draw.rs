//! Drawing helpers: text cut to a width, covers, and the few icons.

use eframe::egui::epaint::{Mesh, RectShape};
use eframe::egui::text::{LayoutJob, TextWrapping};
use eframe::egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, TextureId, pos2, vec2};

use crate::theme;

/// Draws one line of `text` anchored at `pos`, ending in an ellipsis if it
/// is wider than `max_width`. Returns the width actually used.
pub fn text(p: &Painter, pos: Pos2, anchor: Align2, max_width: f32, text: &str, font: &FontId, color: Color32) -> f32 {
    if max_width < 4.0 || text.is_empty() {
        return 0.0;
    }
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font.clone(), color);
    job.wrap = TextWrapping { max_width, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let galley = p.layout_job(job);
    let size = galley.size();
    p.galley(anchor.anchor_size(pos, size).min, galley, color);
    size.x
}

/// Width of `text` on one line, counting trailing spaces.
pub fn width(p: &Painter, text: &str, font: &FontId) -> f32 {
    // A trailing space leaves no ink, so measure with a sentinel and subtract it.
    let measure = |s: String| p.layout_no_wrap(s, font.clone(), Color32::BLACK).size().x;
    measure(format!("{text}|")) - measure("|".to_owned())
}

/// A cover image with rounded corners and a hairline edge, or a neutral
/// placeholder when there is no artwork (yet).
pub fn cover(p: &Painter, rect: Rect, texture: Option<TextureId>, radius: f32) {
    match texture {
        Some(texture) => {
            let whole = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            p.add(RectShape::filled(rect, radius, Color32::WHITE).with_texture(texture, whole));
        }
        None => {
            p.rect_filled(rect, radius, theme::NO_COVER);
            note(p, rect.center(), rect.width() * 0.16, theme::FAINT);
        }
    }
    p.rect_stroke(rect, radius, Stroke::new(1.0, theme::COVER_EDGE), StrokeKind::Inside);
}

/// A musical note about `2 * size` tall, centred on `center`.
pub fn note(p: &Painter, center: Pos2, size: f32, color: Color32) {
    let head = pos2(center.x - size * 0.35, center.y + size * 0.7);
    let top = pos2(head.x + size * 0.42, center.y - size);
    let stroke = Stroke::new((size * 0.16).max(1.0), color);
    p.circle_filled(head, size * 0.42, color);
    p.line_segment([pos2(top.x, head.y), top], stroke);
    p.line_segment([top, pos2(top.x + size * 0.6, top.y + size * 0.45)], stroke);
}

/// How far below the top of a line of `font` the middle of its capitals sits: where an icon beside the line centres.
/// Measured from the ink of a capital, so it holds whatever the font's ascent and line height.
pub fn cap_middle(p: &Painter, font: &FontId) -> f32 {
    let galley = p.layout_no_wrap("H".to_owned(), font.clone(), Color32::BLACK);
    let row = galley.rows.first();
    row.and_then(|row| row.row.glyphs.first().map(|g| row.pos.y + g.pos.y + g.uv_rect.offset.y + g.uv_rect.size.y / 2.0))
        .unwrap_or(font.size * 0.6)
}

/// The verified badge: a small disc of `color` with a tick cut out of it, `size` across.
pub fn verified(p: &Painter, center: Pos2, size: f32, color: Color32) {
    p.circle_filled(center, size / 2.0, color);
    let at = |x: f32, y: f32| pos2(center.x + x * size, center.y + y * size);
    let tick = vec![at(-0.24, 0.02), at(-0.07, 0.19), at(0.25, -0.15)];
    p.add(Shape::line(tick, Stroke::new((size * 0.14).max(1.2), theme::BG)));
}

/// A triangle pointing right (or left when `size` is negative), centred on `center`.
pub fn triangle(p: &Painter, center: Pos2, size: f32, color: Color32) {
    let points = vec![
        pos2(center.x - size * 0.45, center.y - size.abs() * 0.55),
        pos2(center.x + size * 0.55, center.y),
        pos2(center.x - size * 0.45, center.y + size.abs() * 0.55),
    ];
    p.add(Shape::convex_polygon(points, color, Stroke::NONE));
}

pub fn pause_bars(p: &Painter, center: Pos2, size: f32, color: Color32) {
    for side in [-1.0, 1.0] {
        let bar = Rect::from_center_size(pos2(center.x + side * size * 0.28, center.y), vec2(size * 0.3, size * 1.1));
        p.rect_filled(bar, 1.0, color);
    }
}

/// Previous-track (`direction` -1) or next-track (+1) button glyph.
pub fn skip(p: &Painter, center: Pos2, size: f32, direction: f32, color: Color32) {
    triangle(p, pos2(center.x - direction * size * 0.25, center.y), size * direction, color);
    let bar = Rect::from_center_size(pos2(center.x + direction * size * 0.55, center.y), vec2(size * 0.2, size * 1.1));
    p.rect_filled(bar, 0.5, color);
}

/// A loudspeaker facing right, about 18 across: with `waves` arcs of sound before it (0 to 2), or a cross when `None`,
/// as when the sound is off.
pub fn speaker(p: &Painter, center: Pos2, waves: Option<u8>, color: Color32) {
    let at = |x: f32, y: f32| pos2(center.x + x, center.y + y);
    p.rect_filled(Rect::from_min_max(at(-9.0, -2.5), at(-5.0, 2.5)), 0.5, color);
    p.add(Shape::convex_polygon(vec![at(-5.5, -2.5), at(-1.5, -6.0), at(-1.5, 6.0), at(-5.5, 2.5)], color, Stroke::NONE));
    let stroke = Stroke::new(1.4, color);
    match waves {
        Some(waves) => {
            for radius in [4.0, 7.5].into_iter().take(waves as usize) {
                let arc = (-4..=4).map(|i| {
                    let angle = i as f32 * 0.19;
                    at(-1.5 + radius * angle.cos(), radius * angle.sin())
                });
                p.add(Shape::line(arc.collect(), stroke));
            }
        }
        None => {
            p.line_segment([at(2.0, -3.0), at(8.0, 3.0)], stroke);
            p.line_segment([at(2.0, 3.0), at(8.0, -3.0)], stroke);
        }
    }
}

/// Two arrows crossing on their way right, about 20 across: the shuffle button.
pub fn shuffle(p: &Painter, center: Pos2, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    for side in [-1.0f32, 1.0] {
        // Level, then over to the other side along a smooth step, then level again into the head.
        let path = (0..=16)
            .map(|i| {
                let x = -9.0 + i as f32;
                let t = ((x + 4.5) / 8.0).clamp(0.0, 1.0);
                let y = side * 4.5 * (1.0 - 2.0 * t * t * (3.0 - 2.0 * t));
                pos2(center.x + x, center.y + y)
            })
            .collect();
        p.add(Shape::line(path, stroke));
        let tip = pos2(center.x + 10.0, center.y - side * 4.5);
        let head = vec![pos2(tip.x - 4.0, tip.y - 3.5), tip, pos2(tip.x - 4.0, tip.y + 3.5)];
        p.add(Shape::convex_polygon(head, color, Stroke::NONE));
    }
}

pub fn magnifier(p: &Painter, center: Pos2, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let lens = pos2(center.x - 1.5, center.y - 1.5);
    p.circle_stroke(lens, 4.5, stroke);
    p.line_segment([pos2(lens.x + 3.4, lens.y + 3.4), pos2(lens.x + 7.0, lens.y + 7.0)], stroke);
}

/// A heart `size` across, filled or in outline.
pub fn heart(p: &Painter, center: Pos2, size: f32, color: Color32, filled: bool) {
    // The usual curve, x = 16 sin³t, y = 13 cos t − 5 cos 2t − 2 cos 3t − cos 4t: 32 across, its middle at y = −2.5.
    const STEPS: usize = 48;
    let scale = size / 32.0;
    let points: Vec<Pos2> = (0..=STEPS)
        .map(|i| {
            let t = i as f32 / STEPS as f32 * std::f32::consts::TAU;
            let (x, y) = (16.0 * t.sin().powi(3), 13.0 * t.cos() - 5.0 * (2.0 * t).cos() - 2.0 * (3.0 * t).cos() - (4.0 * t).cos());
            pos2(center.x + x * scale, center.y - (y + 2.5) * scale)
        })
        .collect();
    if filled {
        // A fan from the middle, which sees the whole edge; the outline drawn over it smooths the edge.
        let mut mesh = Mesh::default();
        mesh.colored_vertex(center, color);
        for &point in &points[..STEPS] {
            mesh.colored_vertex(point, color);
        }
        for i in 0..STEPS as u32 {
            mesh.add_triangle(0, 1 + i, 1 + (i + 1) % STEPS as u32);
        }
        p.add(Shape::mesh(mesh));
    }
    // Each side on its own, from the notch at the top to the tip: a line joined around those sharp points would end
    // them in long spikes.
    let stroke = Stroke::new(if filled { 0.8 } else { 1.3 }, color);
    p.add(Shape::line(points[..=STEPS / 2].to_vec(), stroke));
    p.add(Shape::line(points[STEPS / 2..].to_vec(), stroke));
}
