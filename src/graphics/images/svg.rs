//! Small, allocation-backed SVG rasterizer for OS artwork.
//!
//! This intentionally implements the compact subset useful for icons rather
//! than the full browser SVG/CSS model: `viewBox`, `rect`, `circle`, `ellipse`,
//! `line`, `polygon`, `polyline`, and straight-line `path` commands
//! (`M/m`, `L/l`, `H/h`, `V/v`, `Z/z`). Shapes support `fill`, `stroke`, and
//! `stroke-width`. Unsupported elements are ignored, so artwork can degrade
//! safely instead of making the desktop fail to paint.

use alloc::vec::Vec;

use super::image::{Image, ImageFormat, PixelFormat};
use crate::graphics::color::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SvgError {
    InvalidUtf8,
    MissingRoot,
    InvalidDimensions,
    InvalidViewBox,
}

#[derive(Debug, Clone, Copy)]
struct Point {
    x: f32,
    y: f32,
}

#[derive(Debug, Clone, Copy)]
struct Paint {
    fill: Option<Color>,
    stroke: Option<Color>,
    stroke_width: f32,
}

impl Default for Paint {
    fn default() -> Self {
        Self {
            fill: Some(Color::BLACK),
            stroke: None,
            stroke_width: 1.0,
        }
    }
}

#[derive(Debug)]
enum Shape {
    Rect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        paint: Paint,
    },
    Ellipse {
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        paint: Paint,
    },
    Poly {
        points: Vec<Point>,
        closed: bool,
        paint: Paint,
    },
}

impl Shape {
    fn sample(&self, point: Point) -> Option<Color> {
        match self {
            Shape::Rect {
                x,
                y,
                width,
                height,
                paint,
            } => {
                let inside = point.x >= *x
                    && point.y >= *y
                    && point.x <= *x + *width
                    && point.y <= *y + *height;
                let half = paint.stroke_width / 2.0;
                let on_stroke = paint.stroke.is_some()
                    && point.x >= *x - half
                    && point.y >= *y - half
                    && point.x <= *x + *width + half
                    && point.y <= *y + *height + half
                    && (point.x <= *x + half
                        || point.x >= *x + *width - half
                        || point.y <= *y + half
                        || point.y >= *y + *height - half);
                if on_stroke {
                    paint.stroke
                } else if inside {
                    paint.fill
                } else {
                    None
                }
            }
            Shape::Ellipse {
                cx,
                cy,
                rx,
                ry,
                paint,
            } => {
                if *rx <= 0.0 || *ry <= 0.0 {
                    return None;
                }
                let dx = (point.x - *cx) / *rx;
                let dy = (point.y - *cy) / *ry;
                let distance = dx * dx + dy * dy;
                if let Some(stroke) = paint.stroke {
                    let inner_rx = (*rx - paint.stroke_width / 2.0).max(0.0);
                    let inner_ry = (*ry - paint.stroke_width / 2.0).max(0.0);
                    let outer_rx = *rx + paint.stroke_width / 2.0;
                    let outer_ry = *ry + paint.stroke_width / 2.0;
                    let outer_dx = (point.x - *cx) / outer_rx;
                    let outer_dy = (point.y - *cy) / outer_ry;
                    let outside_inner = if inner_rx == 0.0 || inner_ry == 0.0 {
                        true
                    } else {
                        let inner_dx = (point.x - *cx) / inner_rx;
                        let inner_dy = (point.y - *cy) / inner_ry;
                        inner_dx * inner_dx + inner_dy * inner_dy >= 1.0
                    };
                    if outer_dx * outer_dx + outer_dy * outer_dy <= 1.0 && outside_inner {
                        return Some(stroke);
                    }
                }
                if distance <= 1.0 {
                    paint.fill
                } else {
                    None
                }
            }
            Shape::Poly {
                points,
                closed,
                paint,
            } => {
                if let Some(stroke) = paint.stroke {
                    let segment_count = if *closed {
                        points.len()
                    } else {
                        points.len().saturating_sub(1)
                    };
                    let threshold = paint.stroke_width * paint.stroke_width / 4.0;
                    for index in 0..segment_count {
                        if distance_to_segment_squared(
                            point,
                            points[index],
                            points[(index + 1) % points.len()],
                        ) <= threshold
                        {
                            return Some(stroke);
                        }
                    }
                }
                if *closed && point_in_polygon(point, points) {
                    paint.fill
                } else {
                    None
                }
            }
        }
    }
}

/// Parsed SVG that retains the original bytes and rasterizes directly at the
/// destination size requested by `GraphicsDevice::draw_image_scaled`.
pub struct SvgImage<'a> {
    #[expect(dead_code, reason = "retained for Image::get_pixel_data")]
    data: &'a [u8],
    width: usize,
    height: usize,
    view_x: f32,
    view_y: f32,
    view_width: f32,
    view_height: f32,
    shapes: Vec<Shape>,
}

impl<'a> SvgImage<'a> {
    pub fn from_bytes(data: &'a [u8]) -> Result<Self, SvgError> {
        let document = core::str::from_utf8(data).map_err(|_| SvgError::InvalidUtf8)?;
        let root_start = document.find("<svg").ok_or(SvgError::MissingRoot)?;
        let root_end = document[root_start..]
            .find('>')
            .map(|end| root_start + end + 1)
            .ok_or(SvgError::MissingRoot)?;
        let root = &document[root_start..root_end];

        let parsed_width = attribute(root, "width").and_then(parse_number);
        let parsed_height = attribute(root, "height").and_then(parse_number);
        let view_box = attribute(root, "viewBox").and_then(parse_view_box);
        let (view_x, view_y, view_width, view_height) = match view_box {
            Some(view) => view,
            None => (
                0.0,
                0.0,
                parsed_width.ok_or(SvgError::InvalidViewBox)?,
                parsed_height.ok_or(SvgError::InvalidViewBox)?,
            ),
        };
        if view_width <= 0.0 || view_height <= 0.0 {
            return Err(SvgError::InvalidViewBox);
        }
        let width = libm::ceilf(parsed_width.unwrap_or(view_width)) as usize;
        let height = libm::ceilf(parsed_height.unwrap_or(view_height)) as usize;
        if width == 0 || height == 0 {
            return Err(SvgError::InvalidDimensions);
        }

        let mut shapes = Vec::new();
        let mut cursor = root_end;
        while let Some(relative) = document[cursor..].find('<') {
            let start = cursor + relative;
            let Some(relative_end) = document[start..].find('>') else {
                break;
            };
            let end = start + relative_end + 1;
            let tag = &document[start..end];
            if tag.starts_with("</svg") {
                break;
            }
            if !tag.starts_with("</") && !tag.starts_with("<!--") {
                if let Some(shape) = parse_shape(tag) {
                    shapes.push(shape);
                }
            }
            cursor = end;
        }

        Ok(Self {
            data,
            width,
            height,
            view_x,
            view_y,
            view_width,
            view_height,
            shapes,
        })
    }

    fn sample_at_size(
        &self,
        x: usize,
        y: usize,
        target_width: usize,
        target_height: usize,
    ) -> Option<Color> {
        if x >= target_width || y >= target_height || target_width == 0 || target_height == 0 {
            return None;
        }
        let point = Point {
            x: self.view_x + (x as f32 + 0.5) * self.view_width / target_width as f32,
            y: self.view_y + (y as f32 + 0.5) * self.view_height / target_height as f32,
        };
        let mut color = None;
        for shape in &self.shapes {
            if let Some(sample) = shape.sample(point) {
                color = Some(sample);
            }
        }
        color
    }
}

impl Image for SvgImage<'_> {
    fn width(&self) -> usize {
        self.width
    }

    fn height(&self) -> usize {
        self.height
    }

    fn format(&self) -> ImageFormat {
        ImageFormat::Svg
    }

    fn pixel_format(&self) -> PixelFormat {
        PixelFormat::Rgba8888
    }

    fn get_pixel(&self, x: usize, y: usize) -> Option<Color> {
        self.sample_at_size(x, y, self.width, self.height)
    }

    fn get_scaled_pixel(
        &self,
        x: usize,
        y: usize,
        target_width: usize,
        target_height: usize,
    ) -> Option<Color> {
        self.sample_at_size(x, y, target_width, target_height)
    }

    fn get_pixel_data(&self) -> &[u8] {
        self.data
    }
}

fn parse_shape(tag: &str) -> Option<Shape> {
    let paint = parse_paint(tag);
    match tag_name(tag)? {
        "rect" => Some(Shape::Rect {
            x: number_attr(tag, "x").unwrap_or(0.0),
            y: number_attr(tag, "y").unwrap_or(0.0),
            width: number_attr(tag, "width")?,
            height: number_attr(tag, "height")?,
            paint,
        }),
        "circle" => {
            let radius = number_attr(tag, "r")?;
            Some(Shape::Ellipse {
                cx: number_attr(tag, "cx").unwrap_or(0.0),
                cy: number_attr(tag, "cy").unwrap_or(0.0),
                rx: radius,
                ry: radius,
                paint,
            })
        }
        "ellipse" => Some(Shape::Ellipse {
            cx: number_attr(tag, "cx").unwrap_or(0.0),
            cy: number_attr(tag, "cy").unwrap_or(0.0),
            rx: number_attr(tag, "rx")?,
            ry: number_attr(tag, "ry")?,
            paint,
        }),
        "line" => Some(Shape::Poly {
            points: alloc::vec![
                Point {
                    x: number_attr(tag, "x1")?,
                    y: number_attr(tag, "y1")?,
                },
                Point {
                    x: number_attr(tag, "x2")?,
                    y: number_attr(tag, "y2")?,
                },
            ],
            closed: false,
            paint: Paint {
                fill: None,
                ..paint
            },
        }),
        "polygon" | "polyline" => Some(Shape::Poly {
            points: parse_points(attribute(tag, "points")?),
            closed: tag_name(tag)? == "polygon",
            paint,
        }),
        "path" => Some(Shape::Poly {
            points: parse_path(attribute(tag, "d")?)?,
            closed: path_is_closed(attribute(tag, "d")?),
            paint,
        }),
        _ => None,
    }
}

fn parse_paint(tag: &str) -> Paint {
    let mut paint = Paint::default();
    if let Some(fill) = attribute(tag, "fill") {
        paint.fill = parse_color(fill);
    }
    if let Some(stroke) = attribute(tag, "stroke") {
        paint.stroke = parse_color(stroke);
    }
    if let Some(width) = number_attr(tag, "stroke-width") {
        paint.stroke_width = width.max(0.0);
    }
    paint
}

fn tag_name(tag: &str) -> Option<&str> {
    let rest = tag.strip_prefix('<')?.trim_start();
    let end = rest
        .find(|ch: char| ch.is_ascii_whitespace() || ch == '/' || ch == '>')
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let bytes = tag.as_bytes();
    let name_bytes = name.as_bytes();
    let mut index = 1;
    while index + name_bytes.len() < bytes.len() {
        if &bytes[index..index + name_bytes.len()] == name_bytes {
            let before_ok = index == 0 || bytes[index - 1].is_ascii_whitespace();
            let mut after = index + name_bytes.len();
            while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                after += 1;
            }
            if before_ok && bytes.get(after) == Some(&b'=') {
                after += 1;
                while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                    after += 1;
                }
                let quote = *bytes.get(after)?;
                if quote != b'\'' && quote != b'"' {
                    return None;
                }
                after += 1;
                let end = bytes[after..].iter().position(|byte| *byte == quote)? + after;
                return Some(&tag[after..end]);
            }
        }
        index += 1;
    }
    None
}

fn number_attr(tag: &str, name: &str) -> Option<f32> {
    attribute(tag, name).and_then(parse_number)
}

fn parse_number(value: &str) -> Option<f32> {
    let end = value
        .find(|ch: char| !(ch.is_ascii_digit() || matches!(ch, '-' | '+' | '.' | 'e' | 'E')))
        .unwrap_or(value.len());
    value[..end].parse().ok()
}

fn parse_view_box(value: &str) -> Option<(f32, f32, f32, f32)> {
    let values = parse_number_list(value);
    (values.len() == 4).then(|| (values[0], values[1], values[2], values[3]))
}

fn parse_points(value: &str) -> Vec<Point> {
    let values = parse_number_list(value);
    let mut points = Vec::new();
    for pair in values.chunks_exact(2) {
        points.push(Point {
            x: pair[0],
            y: pair[1],
        });
    }
    points
}

fn parse_number_list(value: &str) -> Vec<f32> {
    let mut values = Vec::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        while index < bytes.len() && (bytes[index].is_ascii_whitespace() || bytes[index] == b',') {
            index += 1;
        }
        let start = index;
        if index < bytes.len() && matches!(bytes[index], b'+' | b'-') {
            index += 1;
        }
        while index < bytes.len()
            && (bytes[index].is_ascii_digit()
                || matches!(bytes[index], b'.' | b'e' | b'E' | b'+' | b'-'))
        {
            if index > start
                && matches!(bytes[index], b'+' | b'-')
                && !matches!(bytes[index - 1], b'e' | b'E')
            {
                break;
            }
            index += 1;
        }
        if start == index {
            index += 1;
            continue;
        }
        if let Ok(number) = value[start..index].parse() {
            values.push(number);
        }
    }
    values
}

fn parse_path(value: &str) -> Option<Vec<Point>> {
    let mut points = Vec::new();
    let mut numbers = Vec::new();
    let mut command = 'M';
    let mut cursor = Point { x: 0.0, y: 0.0 };
    let mut start = cursor;
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        while index < bytes.len() && (bytes[index].is_ascii_whitespace() || bytes[index] == b',') {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }
        if bytes[index].is_ascii_alphabetic() {
            command = bytes[index] as char;
            index += 1;
            if matches!(command, 'Z' | 'z') {
                cursor = start;
            }
            continue;
        }
        let number_start = index;
        if matches!(bytes[index], b'+' | b'-') {
            index += 1;
        }
        while index < bytes.len()
            && (bytes[index].is_ascii_digit() || matches!(bytes[index], b'.' | b'e' | b'E'))
        {
            index += 1;
            if index < bytes.len()
                && matches!(bytes[index], b'+' | b'-')
                && !matches!(bytes[index - 1], b'e' | b'E')
            {
                break;
            }
        }
        let number = value[number_start..index].parse::<f32>().ok()?;
        numbers.push(number);
        let needed = if matches!(command, 'H' | 'h' | 'V' | 'v') {
            1
        } else {
            2
        };
        if numbers.len() < needed {
            continue;
        }
        let relative = command.is_ascii_lowercase();
        match command {
            'M' | 'm' | 'L' | 'l' => {
                let point = Point {
                    x: numbers[0] + if relative { cursor.x } else { 0.0 },
                    y: numbers[1] + if relative { cursor.y } else { 0.0 },
                };
                cursor = point;
                if matches!(command, 'M' | 'm') && points.is_empty() {
                    start = point;
                    command = if relative { 'l' } else { 'L' };
                }
                points.push(point);
            }
            'H' | 'h' => {
                cursor.x = numbers[0] + if relative { cursor.x } else { 0.0 };
                points.push(cursor);
            }
            'V' | 'v' => {
                cursor.y = numbers[0] + if relative { cursor.y } else { 0.0 };
                points.push(cursor);
            }
            _ => return None,
        }
        numbers.clear();
    }
    (!points.is_empty()).then_some(points)
}

fn path_is_closed(value: &str) -> bool {
    value.bytes().any(|byte| matches!(byte, b'z' | b'Z'))
}

fn parse_color(value: &str) -> Option<Color> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("none") {
        return None;
    }
    if let Some(hex) = value.strip_prefix('#') {
        return match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
                Some(Color::new(r, g, b))
            }
            6 => Some(Color::new(
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
            )),
            _ => None,
        };
    }
    match value {
        "black" => Some(Color::BLACK),
        "white" => Some(Color::WHITE),
        "red" => Some(Color::new(255, 0, 0)),
        "green" => Some(Color::new(0, 128, 0)),
        "blue" => Some(Color::new(0, 0, 255)),
        "yellow" => Some(Color::new(255, 255, 0)),
        "gray" | "grey" => Some(Color::new(128, 128, 128)),
        _ => None,
    }
}

fn point_in_polygon(point: Point, points: &[Point]) -> bool {
    if points.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut previous = points.len() - 1;
    for current in 0..points.len() {
        let a = points[current];
        let b = points[previous];
        if (a.y > point.y) != (b.y > point.y)
            && point.x < (b.x - a.x) * (point.y - a.y) / (b.y - a.y) + a.x
        {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

fn distance_to_segment_squared(point: Point, start: Point, end: Point) -> f32 {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length_squared = dx * dx + dy * dy;
    if length_squared == 0.0 {
        let px = point.x - start.x;
        let py = point.y - start.y;
        return px * px + py * py;
    }
    let t =
        (((point.x - start.x) * dx + (point.y - start.y) * dy) / length_squared).clamp(0.0, 1.0);
    let nearest_x = start.x + t * dx;
    let nearest_y = start.y + t * dy;
    let px = point.x - nearest_x;
    let py = point.y - nearest_y;
    px * px + py * py
}
