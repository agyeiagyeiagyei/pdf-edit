//! Positioned text extraction via hayro's interpreter: each glyph is captured
//! with its device-space position, then aggregated into selectable spans.
//! Coordinates are top-left origin, y down, in PDF points (zoom 1.0 space).

use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::hayro_syntax::Pdf;
use hayro::hayro_interpret::hayro_syntax::page::Page;
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterSettings, Paint,
    PathDrawMode, SoftMask, interpret_page,
};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret;
use kurbo::{Affine, BezPath, Rect};

fn glyph_advance(glyph: &Glyph) -> Option<f32> {
    match glyph {
        Glyph::Outline(g) => g.advance_width(),
        Glyph::Type3(_) => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextSpan {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub text: String,
}

struct GlyphRec {
    x: f64,
    y: f64,
    size: f64,
    advance: f64,
    text: String,
}

struct TextDevice {
    glyphs: Vec<GlyphRec>,
}

impl<'a> Device<'a> for TextDevice {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}

    fn draw_glyph(
        &mut self,
        glyph: &Glyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        _: &Paint<'a>,
        _: &GlyphDrawMode,
    ) {
        let Some(bf) = glyph.as_unicode() else { return };
        let text = match bf {
            BfString::Char(c) => c.to_string(),
            BfString::String(s) => s,
        };
        if text.trim().is_empty() {
            return;
        }
        let full = transform * glyph_transform;
        let origin = full * kurbo::Point::new(0.0, 0.0);
        // glyph_transform maps em units (1000/em) through the text matrix,
        // so |full * (0,1000) - origin| is the font size in device points.
        let up = full * kurbo::Point::new(0.0, 1000.0);
        let size = ((up.x - origin.x).powi(2) + (up.y - origin.y).powi(2)).sqrt();
        let advance = glyph_advance(glyph)
            .map(|w| {
                let right = full * kurbo::Point::new(w as f64, 0.0);
                ((right.x - origin.x).powi(2) + (right.y - origin.y).powi(2)).sqrt()
            })
            .unwrap_or(size * 0.5);
        if size < 0.5 || size > 1000.0 {
            return;
        }
        self.glyphs.push(GlyphRec {
            x: origin.x,
            y: origin.y,
            size,
            advance,
            text,
        });
    }
}

fn extract_glyphs(pdf_bytes: &[u8], page_index: usize) -> Result<Vec<GlyphRec>, String> {
    let pdf = Pdf::new(pdf_bytes.to_vec()).map_err(|e| format!("parse: {e:?}"))?;
    let page = pdf
        .pages()
        .get(page_index)
        .ok_or_else(|| format!("no page {page_index}"))?;
    extract_glyphs_from_page(page)
}

fn extract_glyphs_from_page(page: &Page) -> Result<Vec<GlyphRec>, String> {
    let cache = hayro_interpret::InterpreterCache::new();
    let mut settings = InterpreterSettings::default();
    settings.render_annotations = false;
    let initial_transform = page.initial_transform(true).to_kurbo();
    let (w, h) = page.render_dimensions();
    let mut ctx = Context::new(
        initial_transform,
        Rect::new(0.0, 0.0, w as f64, h as f64),
        &cache,
        page.xref(),
        settings,
    );
    let mut device = TextDevice { glyphs: Vec::new() };
    interpret_page(page, &mut ctx, &mut device);
    Ok(device.glyphs)
}

fn aggregate(mut glyphs: Vec<GlyphRec>) -> Vec<TextSpan> {
    // group into lines: similar baseline y (within 40% of font size)
    glyphs.sort_by(|a, b| {
        a.y.partial_cmp(&b.y)
            .unwrap()
            .then(a.x.partial_cmp(&b.x).unwrap())
    });
    let mut lines: Vec<Vec<GlyphRec>> = Vec::new();
    for g in glyphs {
        let fits = match lines.last_mut() {
            Some(line) => {
                let ly = line[0].y;
                let ls = line[0].size;
                (g.y - ly).abs() < 0.4 * ls.max(g.size)
            }
            None => false,
        };
        if fits {
            lines.last_mut().unwrap().push(g);
        } else {
            lines.push(vec![g]);
        }
    }

    let mut spans = Vec::new();
    for mut line in lines {
        line.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap());
        let mut cur: Option<TextSpan> = None;
        let mut cur_end = 0.0f64;
        for g in &line {
            let gap = g.x - cur_end;
            let break_span = match &cur {
                None => true,
                Some(_) => gap > 0.45 * g.size,
            };
            if break_span {
                if let Some(s) = cur.take() {
                    spans.push(s);
                }
                cur = Some(TextSpan {
                    x: g.x as f32,
                    y: (g.y - 0.85 * g.size) as f32,
                    w: g.advance as f32,
                    h: (g.size * 1.15) as f32,
                    text: g.text.clone(),
                });
            } else if let Some(s) = cur.as_mut() {
                if gap > 0.22 * g.size && !s.text.ends_with(' ') {
                    s.text.push(' ');
                }
                s.text.push_str(&g.text);
                s.w = (g.x + g.advance - s.x as f64) as f32;
            }
            cur_end = g.x + g.advance;
        }
        if let Some(s) = cur.take() {
            spans.push(s);
        }
    }
    spans
}

/// Positioned text spans for one page (0-based), top-left origin in points.
pub fn text_spans(pdf_bytes: &[u8], page_index: usize) -> Result<Vec<TextSpan>, String> {
    Ok(aggregate(extract_glyphs(pdf_bytes, page_index)?))
}

/// A line-level search hit: the spans (with their rects) that the match covers.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub page: usize,
    pub text: String,
    pub spans: Vec<TextSpan>,
}

/// Case-insensitive substring search across aggregated line text.
/// Matches spanning multiple spans on one line are returned with all covered spans.
pub fn search(pdf_bytes: &[u8], query: &str) -> Result<Vec<SearchHit>, String> {
    let pdf = Pdf::new(pdf_bytes.to_vec()).map_err(|e| format!("parse: {e:?}"))?;
    let n = pdf.pages().len();
    let q = query.to_lowercase();
    let mut hits = Vec::new();
    if q.is_empty() {
        return Ok(hits);
    }
    for page_index in 0..n {
        let spans = aggregate(extract_glyphs(pdf_bytes, page_index)?);
        // rebuild lines from spans (spans on one line share y within tolerance)
        let mut lines: Vec<Vec<&TextSpan>> = Vec::new();
        for s in &spans {
            let fits = lines.last_mut().and_then(|line: &mut Vec<&TextSpan>| {
                let ly = line[0].y;
                if (s.y - ly).abs() < 0.5 * line[0].h {
                    line.push(s);
                    Some(())
                } else {
                    None
                }
            });
            if fits.is_none() {
                lines.push(vec![s]);
            }
        }
        for line in lines {
            let line_text: String = line.iter().map(|s| s.text.as_str()).collect();
            let lower = line_text.to_lowercase();
            let mut start = 0;
            while let Some(pos) = lower[start..].find(&q) {
                let abs = start + pos;
                let end = abs + q.len();
                // map char range back to covering spans
                let mut covered: Vec<TextSpan> = Vec::new();
                let mut offset = 0;
                for s in &line {
                    let s_start = offset;
                    let s_end = offset + s.text.chars().count();
                    offset = s_end;
                    if s_end > abs && s_start < end {
                        covered.push((*s).clone());
                    }
                }
                if !covered.is_empty() {
                    hits.push(SearchHit {
                        page: page_index,
                        text: line_text[abs..end].to_string(),
                        spans: covered,
                    });
                }
                start = abs + q.len().max(1);
            }
        }
    }
    Ok(hits)
}
