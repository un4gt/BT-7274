use ratatui::{buffer::Buffer, layout::Rect, style::Color};

pub const BLACK: Color = Color::Rgb(7, 10, 12);
pub const DIM: Color = Color::Rgb(52, 66, 69);
pub const STEEL: Color = Color::Rgb(119, 139, 136);
pub const OLIVE: Color = Color::Rgb(134, 147, 111);
pub const ARMOR: Color = Color::Rgb(189, 189, 157);
pub const RUST: Color = Color::Rgb(185, 105, 63);
pub const AMBER: Color = Color::Rgb(255, 184, 108);
pub const COLD: Color = Color::Rgb(117, 184, 185);

pub struct Canvas {
    pub buffer: Buffer,
    pub width: i32,
    pub height: i32,
}

impl Canvas {
    pub fn new(width: u16, height: u16) -> Self {
        let mut buffer = Buffer::empty(Rect::new(0, 0, width, height));
        for cell in &mut buffer.content {
            cell.set_bg(BLACK).set_fg(DIM);
        }
        Self {
            buffer,
            width: i32::from(width),
            height: i32::from(height),
        }
    }

    pub fn put(&mut self, x: i32, y: i32, ch: char, color: Color) {
        if x >= 0 && y >= 0 && x < self.width && y < self.height {
            let mut encoded = [0; 4];
            self.buffer[(x as u16, y as u16)]
                .set_symbol(ch.encode_utf8(&mut encoded))
                .set_fg(color);
        }
    }

    pub fn text(&mut self, x: i32, y: i32, text: &str, color: Color) {
        for (dx, ch) in text.chars().enumerate() {
            self.put(x + dx as i32, y, ch, color);
        }
    }

    pub fn centered(&mut self, y: i32, text: &str, color: Color) {
        self.text(
            (self.width - text.chars().count() as i32) / 2,
            y,
            text,
            color,
        );
    }

    pub fn overlay(&mut self, layer: &Canvas) {
        for (target, source) in self.buffer.content.iter_mut().zip(&layer.buffer.content) {
            if source.symbol() != " " || source.fg != DIM || source.bg != BLACK {
                *target = source.clone();
            }
        }
    }

    pub fn tint_background(&mut self, x: i32, y: i32, color: Color, strength: f32) {
        if x >= 0 && y >= 0 && x < self.width && y < self.height {
            let cell = &mut self.buffer[(x as u16, y as u16)];
            cell.set_bg(mix(cell.bg, color, strength));
        }
    }
}

pub fn mix(a: Color, b: Color, t: f32) -> Color {
    if t <= 0.0 {
        return a;
    }
    if t >= 1.0 {
        return b;
    }
    let (ar, ag, ab) = rgb(a);
    let (br, bg, bb) = rgb(b);
    let t = t.clamp(0.0, 1.0);
    let lerp = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t) as u8;
    Color::Rgb(lerp(ar, br), lerp(ag, bg), lerp(ab, bb))
}

pub fn rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (7, 10, 12),
    }
}

/// Stable noise: replay, export and live rendering sample the same particles.
pub fn hash(seed: u32) -> u32 {
    let mut n = seed.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    n = ((n >> ((n >> 28) + 4)) ^ n).wrapping_mul(277_803_737);
    (n >> 22) ^ n
}

pub fn random(seed: u32) -> f32 {
    (hash(seed) & 0xffff) as f32 / 65_535.0
}
