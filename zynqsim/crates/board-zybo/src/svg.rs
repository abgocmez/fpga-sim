//! A small SVG subset for the 2D board art (`svg = "board.svg"` in a board file).
//!
//! The 2D view is drawn by the software rasteriser in [`crate::render`]; this module reads
//! the artwork behind the controls and draws it through the same code path, so the GUI, the
//! MCP screenshot and the golden-image tests all show the same pixels. It is **not** a
//! general SVG renderer.
//!
//! Supported: `svg` (`viewBox`, or `width`/`height`), `g`, `rect` (with `rx`), `circle`,
//! `ellipse`, `line`, `polyline`, `polygon` and `path` with the commands `M L H V C Q Z`
//! (absolute and relative). Attributes: `fill`, `stroke`, `stroke-width`, `opacity`,
//! `fill-opacity`, `stroke-opacity` and a `style` attribute carrying the same properties,
//! inherited through groups. Colours: `#rgb`, `#rrggbb`, `rgb(r,g,b)`, `none` and a few
//! names. Text, images, gradients, filters, clipping, `use` and the like are skipped.
//!
//! Anything that would draw the *wrong* picture is an error instead of being skipped:
//! a `transform` attribute, a `url(...)` paint, a percentage length, a path command outside
//! the list above. The view box is stretched over the board's layout rectangle.

use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform};

/// One drawable shape with its resolved paint.
#[derive(Clone, Debug)]
struct Shape {
    path: Path,
    fill: Option<[u8; 4]>,
    stroke: Option<([u8; 4], f32)>,
}

/// A parsed drawing, ready to rasterise.
#[derive(Clone, Debug)]
pub(crate) struct Art {
    view_box: [f32; 4],
    shapes: Vec<Shape>,
}

impl Art {
    /// Number of shapes that will be drawn.
    #[cfg(test)]
    pub(crate) fn shape_count(&self) -> usize {
        self.shapes.len()
    }

    /// Draws the art so that its view box covers the `lw` x `lh` layout rectangle;
    /// `tf` maps layout units to pixels.
    pub(crate) fn draw(&self, pm: &mut Pixmap, tf: Transform, lw: f32, lh: f32) {
        let [vx, vy, vw, vh] = self.view_box;
        let (sx, sy) = (lw / vw, lh / vh);
        let local = tf.pre_concat(Transform::from_row(sx, 0.0, 0.0, sy, -vx * sx, -vy * sy));
        for s in &self.shapes {
            if let Some(c) = s.fill {
                pm.fill_path(&s.path, &paint(c), FillRule::Winding, local, None);
            }
            if let Some((c, w)) = s.stroke {
                let stroke = Stroke {
                    width: w,
                    ..Stroke::default()
                };
                pm.stroke_path(&s.path, &paint(c), &stroke, local, None);
            }
        }
    }
}

fn paint(c: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(c[0], c[1], c[2], c[3]));
    p.anti_alias = true;
    p
}

// ----------------------------------------------------------------- XML

#[derive(Debug, PartialEq)]
enum Kind {
    Open,
    Close,
    SelfClose,
}

#[derive(Debug)]
struct Tag {
    name: String,
    attrs: Vec<(String, String)>,
    kind: Kind,
}

impl Tag {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

fn line_of(text: &str, at: usize) -> usize {
    text[..at.min(text.len())].matches('\n').count() + 1
}

/// Splits the document into tags. Character data is ignored (there is no text
/// rendering); comments, processing instructions, DOCTYPE and CDATA are skipped.
fn tags(text: &str) -> Result<Vec<Tag>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(off) = text[i..].find('<') {
        let start = i + off;
        let rest = &text[start..];
        let skip_to = |end: &str| -> Result<usize, String> {
            rest.find(end)
                .map(|p| start + p + end.len())
                .ok_or_else(|| format!("line {}: `{end}` is missing", line_of(text, start)))
        };
        if rest.starts_with("<!--") {
            i = skip_to("-->")?;
        } else if rest.starts_with("<![CDATA[") {
            i = skip_to("]]>")?;
        } else if rest.starts_with("<?") {
            i = skip_to("?>")?;
        } else if rest.starts_with("<!") {
            i = skip_to(">")?;
        } else {
            // A tag: ends at the first `>` outside quotes.
            let mut quote = None;
            let mut end = None;
            for (p, ch) in rest.char_indices().skip(1) {
                match (quote, ch) {
                    (Some(q), c) if c == q => quote = None,
                    (None, '"' | '\'') => quote = Some(ch),
                    (None, '>') => {
                        end = Some(p);
                        break;
                    }
                    _ => {}
                }
            }
            let end =
                end.ok_or_else(|| format!("line {}: unterminated tag", line_of(text, start)))?;
            let inner = &rest[1..end];
            out.push(parse_tag(inner).map_err(|e| format!("line {}: {e}", line_of(text, start)))?);
            i = start + end + 1;
        }
    }
    Ok(out)
}

fn parse_tag(inner: &str) -> Result<Tag, String> {
    let (inner, kind) = if let Some(s) = inner.strip_prefix('/') {
        (s, Kind::Close)
    } else if let Some(s) = inner.strip_suffix('/') {
        (s, Kind::SelfClose)
    } else {
        (inner, Kind::Open)
    };
    let inner = inner.trim();
    let name_end = inner
        .find(|c: char| c.is_whitespace())
        .unwrap_or(inner.len());
    let full = &inner[..name_end];
    if full.is_empty() {
        return Err("empty tag name".to_string());
    }
    // Drop a namespace prefix: `svg:rect` is `rect`.
    let name = full.rsplit(':').next().unwrap_or(full).to_string();
    let mut attrs = Vec::new();
    let mut rest = inner[name_end..].trim_start();
    while !rest.is_empty() {
        let eq = rest
            .find('=')
            .ok_or_else(|| format!("malformed attribute near `{rest}` in <{name}>"))?;
        let key = rest[..eq].trim();
        let after = rest[eq + 1..].trim_start();
        let q = after
            .chars()
            .next()
            .filter(|c| *c == '"' || *c == '\'')
            .ok_or_else(|| format!("attribute `{key}` of <{name}> is not quoted"))?;
        let body = &after[1..];
        let close = body
            .find(q)
            .ok_or_else(|| format!("attribute `{key}` of <{name}> is not closed"))?;
        attrs.push((key.to_string(), body[..close].to_string()));
        rest = body[close + 1..].trim_start();
    }
    Ok(Tag { name, attrs, kind })
}

// -------------------------------------------------------------- values

fn length(s: &str, what: &str) -> Result<f32, String> {
    let t = s.trim();
    if t.ends_with('%') {
        return Err(format!("{what}=\"{s}\": percentages are not supported"));
    }
    let t = t.strip_suffix("px").unwrap_or(t);
    t.parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("{what}=\"{s}\" is not a number"))
}

fn named_colour(n: &str) -> Option<[u8; 3]> {
    Some(match n {
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        "red" => [255, 0, 0],
        "green" => [0, 128, 0],
        "lime" => [0, 255, 0],
        "blue" => [0, 0, 255],
        "yellow" => [255, 255, 0],
        "gray" | "grey" => [128, 128, 128],
        "silver" => [192, 192, 192],
        "orange" => [255, 165, 0],
        "purple" => [128, 0, 128],
        "navy" => [0, 0, 128],
        "teal" => [0, 128, 128],
        "maroon" => [128, 0, 0],
        "gold" => [255, 215, 0],
        _ => return None,
    })
}

/// `Ok(None)` is `none`.
fn colour(s: &str, what: &str) -> Result<Option<[u8; 3]>, String> {
    let t = s.trim();
    let lower = t.to_ascii_lowercase();
    if lower == "none" || lower == "transparent" {
        return Ok(None);
    }
    if lower.starts_with("url(") {
        return Err(format!(
            "{what}=\"{s}\": gradients and patterns (url(...)) are not supported"
        ));
    }
    if let Some(h) = lower.strip_prefix('#') {
        let hex = |x: &str| u8::from_str_radix(x, 16).ok();
        let v = match h.len() {
            3 => {
                let d: Vec<u8> = h
                    .chars()
                    .filter_map(|c| c.to_digit(16))
                    .map(|d| (d * 17) as u8)
                    .collect();
                (d.len() == 3).then(|| [d[0], d[1], d[2]])
            }
            6 => match (hex(&h[0..2]), hex(&h[2..4]), hex(&h[4..6])) {
                (Some(r), Some(g), Some(b)) => Some([r, g, b]),
                _ => None,
            },
            _ => None,
        };
        return v
            .map(Some)
            .ok_or_else(|| format!("{what}=\"{s}\" is not a #rgb or #rrggbb colour"));
    }
    if let Some(args) = lower.strip_prefix("rgb(").and_then(|a| a.strip_suffix(')')) {
        let parts: Vec<&str> = args.split(',').map(str::trim).collect();
        if parts.len() == 3 {
            let c: Vec<Option<u8>> = parts.iter().map(|p| p.parse::<u8>().ok()).collect();
            if let [Some(r), Some(g), Some(b)] = c[..] {
                return Ok(Some([r, g, b]));
            }
        }
        return Err(format!("{what}=\"{s}\": expected rgb(0-255, 0-255, 0-255)"));
    }
    named_colour(&lower)
        .map(Some)
        .ok_or_else(|| format!("{what}=\"{s}\" is not a colour this renderer knows"))
}

// --------------------------------------------------------------- style

#[derive(Clone, Debug)]
struct Style {
    fill: Option<[u8; 3]>,
    stroke: Option<[u8; 3]>,
    stroke_width: f32,
    opacity: f32,
    fill_opacity: f32,
    stroke_opacity: f32,
}

impl Default for Style {
    fn default() -> Style {
        Style {
            fill: Some([0, 0, 0]),
            stroke: None,
            stroke_width: 1.0,
            opacity: 1.0,
            fill_opacity: 1.0,
            stroke_opacity: 1.0,
        }
    }
}

impl Style {
    fn set(&mut self, prop: &str, value: &str) -> Result<(), String> {
        let unit = |v: &str| -> Result<f32, String> {
            v.trim()
                .parse::<f32>()
                .ok()
                .filter(|x| x.is_finite())
                .map(|x| x.clamp(0.0, 1.0))
                .ok_or_else(|| format!("{prop}=\"{v}\" is not a number"))
        };
        match prop {
            "fill" => self.fill = colour(value, "fill")?,
            "stroke" => self.stroke = colour(value, "stroke")?,
            "stroke-width" => self.stroke_width = length(value, "stroke-width")?.max(0.0),
            "opacity" => self.opacity = unit(value)?,
            "fill-opacity" => self.fill_opacity = unit(value)?,
            "stroke-opacity" => self.stroke_opacity = unit(value)?,
            _ => {}
        }
        Ok(())
    }

    /// Applies the presentation attributes and the `style` attribute of a tag.
    fn apply(&mut self, tag: &Tag) -> Result<(), String> {
        for (k, v) in &tag.attrs {
            self.set(k, v)?;
        }
        if let Some(style) = tag.attr("style") {
            for decl in style.split(';') {
                if let Some((k, v)) = decl.split_once(':') {
                    self.set(k.trim(), v.trim())?;
                }
            }
        }
        Ok(())
    }

    fn rgba(c: [u8; 3], a: f32) -> [u8; 4] {
        [c[0], c[1], c[2], (a.clamp(0.0, 1.0) * 255.0).round() as u8]
    }
}

// --------------------------------------------------------------- paths

struct Scanner<'a> {
    s: &'a [u8],
    i: usize,
}

impl Scanner<'_> {
    fn skip_sep(&mut self) {
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',')
        {
            self.i += 1;
        }
    }

    fn peek_number(&mut self) -> bool {
        self.skip_sep();
        self.i < self.s.len()
            && (self.s[self.i].is_ascii_digit() || matches!(self.s[self.i], b'-' | b'+' | b'.'))
    }

    fn number(&mut self) -> Result<f32, String> {
        self.skip_sep();
        let start = self.i;
        if self.i < self.s.len() && matches!(self.s[self.i], b'-' | b'+') {
            self.i += 1;
        }
        let mut seen_dot = false;
        while self.i < self.s.len() {
            let c = self.s[self.i];
            if c.is_ascii_digit() {
                self.i += 1;
            } else if c == b'.' && !seen_dot {
                seen_dot = true;
                self.i += 1;
            } else if (c == b'e' || c == b'E')
                && self.i > start
                && self
                    .s
                    .get(self.i + 1)
                    .is_some_and(|n| n.is_ascii_digit() || matches!(n, b'-' | b'+'))
            {
                self.i += 2;
            } else {
                break;
            }
        }
        std::str::from_utf8(&self.s[start..self.i])
            .ok()
            .and_then(|t| t.parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("path data: expected a number at offset {start}"))
    }
}

fn path_data(d: &str) -> Result<Option<Path>, String> {
    let mut sc = Scanner {
        s: d.as_bytes(),
        i: 0,
    };
    let mut pb = PathBuilder::new();
    let (mut cx, mut cy) = (0.0f32, 0.0f32);
    let (mut sx, mut sy) = (0.0f32, 0.0f32);
    let mut cmd = 0u8;
    let mut started = false;
    loop {
        sc.skip_sep();
        if sc.i >= sc.s.len() {
            break;
        }
        let c = sc.s[sc.i];
        if c.is_ascii_alphabetic() {
            cmd = c;
            sc.i += 1;
            if !matches!(
                cmd.to_ascii_uppercase(),
                b'M' | b'L' | b'H' | b'V' | b'C' | b'Q' | b'Z'
            ) {
                return Err(format!(
                    "path command `{}` is not supported (use M L H V C Q Z)",
                    cmd as char
                ));
            }
        } else if cmd == 0 {
            return Err("path data must start with a command".to_string());
        }
        let rel = cmd.is_ascii_lowercase();
        let (ox, oy) = if rel { (cx, cy) } else { (0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            b'Z' => {
                pb.close();
                cx = sx;
                cy = sy;
                cmd = 0;
                // `Z` takes no numbers; a following number needs a new command.
                if sc.peek_number() {
                    return Err("path data: numbers after `Z`".to_string());
                }
            }
            b'M' => {
                let (x, y) = (sc.number()? + ox, sc.number()? + oy);
                pb.move_to(x, y);
                (cx, cy, sx, sy) = (x, y, x, y);
                started = true;
                // Further pairs after a move are implicit line-tos.
                cmd = if rel { b'l' } else { b'L' };
            }
            _ if !started => return Err("path data must start with M".to_string()),
            b'L' => {
                let (x, y) = (sc.number()? + ox, sc.number()? + oy);
                pb.line_to(x, y);
                (cx, cy) = (x, y);
            }
            b'H' => {
                let x = sc.number()? + ox;
                pb.line_to(x, cy);
                cx = x;
            }
            b'V' => {
                let y = sc.number()? + oy;
                pb.line_to(cx, y);
                cy = y;
            }
            b'C' => {
                let (x1, y1) = (sc.number()? + ox, sc.number()? + oy);
                let (x2, y2) = (sc.number()? + ox, sc.number()? + oy);
                let (x, y) = (sc.number()? + ox, sc.number()? + oy);
                pb.cubic_to(x1, y1, x2, y2, x, y);
                (cx, cy) = (x, y);
            }
            b'Q' => {
                let (x1, y1) = (sc.number()? + ox, sc.number()? + oy);
                let (x, y) = (sc.number()? + ox, sc.number()? + oy);
                pb.quad_to(x1, y1, x, y);
                (cx, cy) = (x, y);
            }
            _ => unreachable!("commands are filtered above"),
        }
    }
    Ok(pb.finish())
}

fn points(s: &str, close: bool) -> Result<Option<Path>, String> {
    let mut sc = Scanner {
        s: s.as_bytes(),
        i: 0,
    };
    let mut pb = PathBuilder::new();
    let mut n = 0;
    while sc.peek_number() {
        let (x, y) = (sc.number()?, sc.number()?);
        if n == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
        n += 1;
    }
    sc.skip_sep();
    if sc.i < sc.s.len() {
        return Err(format!("points=\"{s}\" has trailing characters"));
    }
    if n < 2 {
        return Err(format!("points=\"{s}\" needs at least two points"));
    }
    if close {
        pb.close();
    }
    Ok(pb.finish())
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    if r <= 0.0 {
        return Rect::from_xywh(x, y, w, h).map(PathBuilder::from_rect);
    }
    let k = r.min(w / 2.0).min(h / 2.0);
    let (l, t, rt, b) = (x, y, x + w, y + h);
    let mut pb = PathBuilder::new();
    pb.move_to(l + k, t);
    pb.line_to(rt - k, t);
    pb.quad_to(rt, t, rt, t + k);
    pb.line_to(rt, b - k);
    pb.quad_to(rt, b, rt - k, b);
    pb.line_to(l + k, b);
    pb.quad_to(l, b, l, b - k);
    pb.line_to(l, t + k);
    pb.quad_to(l, t, l + k, t);
    pb.close();
    pb.finish()
}

// ---------------------------------------------------------------- parse

/// Elements whose children are never drawn directly.
const HIDDEN_CONTAINERS: &[&str] = &[
    "defs",
    "clipPath",
    "mask",
    "pattern",
    "marker",
    "symbol",
    "linearGradient",
    "radialGradient",
    "filter",
    "style",
    "title",
    "desc",
    "metadata",
    "text",
    "foreignObject",
];

/// Parses an SVG document in the supported subset.
pub(crate) fn parse(text: &str) -> Result<Art, String> {
    let all = tags(text)?;
    let mut stack: Vec<Style> = vec![Style::default()];
    let mut hidden = 0usize;
    let mut open_names: Vec<String> = Vec::new();
    let mut view_box: Option<[f32; 4]> = None;
    let mut shapes = Vec::new();
    let mut saw_svg = false;

    for tag in &all {
        if tag.kind == Kind::Close {
            let name = open_names
                .pop()
                .ok_or_else(|| format!("</{}> closes nothing", tag.name))?;
            if name != tag.name {
                return Err(format!("</{}> closes <{name}>", tag.name));
            }
            stack.pop();
            if HIDDEN_CONTAINERS.contains(&name.as_str()) {
                hidden -= 1;
            }
            continue;
        }

        if let Some(t) = tag.attr("transform") {
            if !t.trim().is_empty() {
                return Err(format!(
                    "<{}> has transform=\"{t}\": transforms are not supported \
                     (bake them into the coordinates)",
                    tag.name
                ));
            }
        }
        let mut style = stack.last().cloned().unwrap_or_default();
        style
            .apply(tag)
            .map_err(|e| format!("<{}>: {e}", tag.name))?;

        if tag.name == "svg" && !saw_svg {
            saw_svg = true;
            view_box = Some(if let Some(vb) = tag.attr("viewBox") {
                let mut sc = Scanner {
                    s: vb.as_bytes(),
                    i: 0,
                };
                let v = [sc.number()?, sc.number()?, sc.number()?, sc.number()?];
                if v[2] <= 0.0 || v[3] <= 0.0 {
                    return Err(format!("viewBox=\"{vb}\" has a non-positive size"));
                }
                v
            } else {
                match (tag.attr("width"), tag.attr("height")) {
                    (Some(w), Some(h)) => {
                        let (w, h) = (length(w, "width")?, length(h, "height")?);
                        if w <= 0.0 || h <= 0.0 {
                            return Err("width and height must be positive".to_string());
                        }
                        [0.0, 0.0, w, h]
                    }
                    _ => {
                        return Err(
                            "the <svg> element needs a viewBox, or width and height".to_string()
                        );
                    }
                }
            });
        }

        let drawable = hidden == 0 && saw_svg;
        if drawable {
            if let Some(path) = shape_path(tag)? {
                let fill_ok = tag.name != "line";
                let fill = if fill_ok {
                    style
                        .fill
                        .map(|c| Style::rgba(c, style.opacity * style.fill_opacity))
                } else {
                    None
                };
                let stroke = style.stroke.map(|c| {
                    (
                        Style::rgba(c, style.opacity * style.stroke_opacity),
                        style.stroke_width,
                    )
                });
                if fill.is_some() || stroke.is_some() {
                    shapes.push(Shape { path, fill, stroke });
                }
            }
        }

        if tag.kind == Kind::Open {
            if HIDDEN_CONTAINERS.contains(&tag.name.as_str()) {
                hidden += 1;
            }
            open_names.push(tag.name.clone());
            stack.push(style);
        }
    }
    if let Some(n) = open_names.last() {
        return Err(format!("<{n}> is never closed"));
    }
    let view_box = view_box.ok_or_else(|| "no <svg> element".to_string())?;
    Ok(Art { view_box, shapes })
}

/// The outline of a shape element, or `None` for any other element.
fn shape_path(tag: &Tag) -> Result<Option<Path>, String> {
    let num = |name: &str, default: f32| -> Result<f32, String> {
        match tag.attr(name) {
            Some(v) => length(v, name),
            None => Ok(default),
        }
    };
    let bad = |what: &str| format!("<{}> {what}", tag.name);
    let path = match tag.name.as_str() {
        "rect" => {
            let (x, y) = (num("x", 0.0)?, num("y", 0.0)?);
            let (w, h) = (num("width", 0.0)?, num("height", 0.0)?);
            if w <= 0.0 || h <= 0.0 {
                return Err(bad("needs a positive width and height"));
            }
            let rx = num("rx", num("ry", 0.0)?)?;
            rounded_rect(x, y, w, h, rx)
        }
        "circle" => {
            let r = num("r", 0.0)?;
            if r <= 0.0 {
                return Err(bad("needs a positive r"));
            }
            PathBuilder::from_circle(num("cx", 0.0)?, num("cy", 0.0)?, r)
        }
        "ellipse" => {
            let (rx, ry) = (num("rx", 0.0)?, num("ry", 0.0)?);
            if rx <= 0.0 || ry <= 0.0 {
                return Err(bad("needs a positive rx and ry"));
            }
            let (cx, cy) = (num("cx", 0.0)?, num("cy", 0.0)?);
            Rect::from_xywh(cx - rx, cy - ry, 2.0 * rx, 2.0 * ry).and_then(PathBuilder::from_oval)
        }
        "line" => {
            let mut pb = PathBuilder::new();
            pb.move_to(num("x1", 0.0)?, num("y1", 0.0)?);
            pb.line_to(num("x2", 0.0)?, num("y2", 0.0)?);
            pb.finish()
        }
        "polyline" | "polygon" => {
            let pts = tag.attr("points").ok_or_else(|| bad("needs points"))?;
            points(pts, tag.name == "polygon")?
        }
        "path" => {
            let d = tag.attr("d").ok_or_else(|| bad("needs d"))?;
            path_data(d)?
        }
        _ => return Ok(None),
    };
    Ok(Some(path.ok_or_else(|| bad("has a degenerate outline"))?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r##"<?xml version="1.0"?>
<!-- a comment with <tags> -->
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 64">
  <title>ignored</title>
  <defs><rect width="5" height="5" fill="red"/></defs>
  <g fill="#336699" stroke="none">
    <rect x="2" y="2" width="30" height="20" rx="3"/>
    <circle cx="50" cy="30" r="8" fill="rgb(255,0,0)" opacity="0.5"/>
    <path d="M 60 10 L 90 10 l 0 20 H 60 Z" stroke="white" stroke-width="2"/>
    <polygon points="10,40 30,40 20,60"/>
    <line x1="0" y1="62" x2="100" y2="62" style="stroke:#fff; stroke-width:2"/>
  </g>
  <text x="1" y="1">skipped</text>
</svg>"##;

    #[test]
    fn a_document_in_the_subset_parses() {
        let art = parse(GOOD).unwrap();
        assert_eq!(art.view_box, [0.0, 0.0, 100.0, 64.0]);
        // rect, circle, path, polygon, line (the <defs> rect and <text> are skipped).
        assert_eq!(art.shape_count(), 5);
    }

    #[test]
    fn drawing_changes_pixels_where_the_art_is() {
        let art = parse(GOOD).unwrap();
        let mut pm = Pixmap::new(100, 64).unwrap();
        pm.fill(Color::from_rgba8(0, 0, 0, 255));
        art.draw(&mut pm, Transform::identity(), 100.0, 64.0);
        let px = |x: usize, y: usize| {
            let o = (y * 100 + x) * 4;
            [pm.data()[o], pm.data()[o + 1], pm.data()[o + 2]]
        };
        assert_eq!(px(15, 12), [0x33, 0x66, 0x99], "the rounded rect");
        assert_eq!(px(95, 50), [0, 0, 0], "nothing there");
        assert!(px(50, 30)[0] > 100, "the half transparent red circle");
    }

    #[test]
    fn the_view_box_is_stretched_over_the_layout() {
        let art =
            parse(r##"<svg viewBox="0 0 10 10"><rect width="10" height="10" fill="#fff"/></svg>"##)
                .unwrap();
        let mut pm = Pixmap::new(40, 20).unwrap();
        pm.fill(Color::from_rgba8(0, 0, 0, 255));
        art.draw(&mut pm, Transform::identity(), 40.0, 20.0);
        assert_eq!(&pm.data()[..3], &[255, 255, 255]);
        assert_eq!(&pm.data()[(39 + 19 * 40) * 4..][..3], &[255, 255, 255]);
    }

    fn err(text: &str) -> String {
        parse(text).unwrap_err()
    }

    #[test]
    fn what_would_draw_the_wrong_picture_is_an_error() {
        let wrap = |inner: &str| format!(r#"<svg viewBox="0 0 10 10">{inner}</svg>"#);
        assert!(
            err(&wrap(
                r#"<g transform="rotate(5)"><rect width="1" height="1"/></g>"#
            ))
            .contains("transforms are not supported")
        );
        assert!(err(&wrap(r#"<rect width="1" height="1" fill="url(#g)"/>"#)).contains("url(...)"));
        assert!(err(&wrap(r#"<rect width="50%" height="1"/>"#)).contains("percentages"));
        assert!(
            err(&wrap(r#"<path d="M0 0 A 5 5 0 0 1 10 10"/>"#))
                .contains("command `A` is not supported")
        );
        assert!(
            err(&wrap(r#"<rect width="1" height="1" fill="chartreuse"/>"#))
                .contains("not a colour")
        );
        assert!(err(&wrap(r#"<rect height="1"/>"#)).contains("positive width"));
    }

    #[test]
    fn structural_errors_are_reported_with_a_line() {
        assert!(
            err("<svg viewBox=\"0 0 1 1\">\n<rect width=\"1\" height=\"1\"").contains("line 2")
        );
        assert!(err(r#"<svg viewBox="0 0 1 1"><g></svg>"#).contains("closes"));
        assert!(err(r#"<svg viewBox="0 0 1 1"><g>"#).contains("never closed"));
        assert!(err(r#"<svg><rect width="1" height="1"/></svg>"#).contains("viewBox"));
        assert!(err(r#"<svg viewBox="0 0 0 5"/>"#).contains("non-positive"));
        assert!(err("not svg at all").contains("no <svg>"));
        assert!(
            err(r#"<svg viewBox="0 0 1 1"><rect width=1 height="1"/></svg>"#)
                .contains("not quoted")
        );
    }

    #[test]
    fn size_attributes_replace_a_missing_view_box() {
        let a =
            parse(r#"<svg width="200px" height="100"><rect width="1" height="1"/></svg>"#).unwrap();
        assert_eq!(a.view_box, [0.0, 0.0, 200.0, 100.0]);
    }
}
