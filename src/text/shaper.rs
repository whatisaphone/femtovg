
use harfbuzz_rs as hb;
use self::hb::hb as hb_sys;
use unicode_bidi::BidiInfo;

use super::{
    Align,
    Baseline,
    Font,
    FontDb,
    FontId,
    TextStyle,
    freetype as ft,
    RenderStyle,
    TextLayout,
    GLYPH_PADDING
};

mod run_segmentation;
use run_segmentation::{
    Segment,
    Segmentable,
    UnicodeScripts
};

// harfbuzz-sys doesn't add this symbol for mac builds.
// And we need it since we're using freetype on OSX.
extern "C" {
    pub fn hb_ft_font_create_referenced(face: ft::ffi::FT_Face) -> *mut hb_sys::hb_font_t;
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Direction {
    Ltr, Rtl
}

#[derive(Copy, Clone, Debug)]
pub struct ShapedGlyph {
    pub x: f32,
    pub y: f32,
    pub c: char,
    pub index: usize,
    pub font_id: FontId,
    pub codepoint: u32,
    pub width: f32,
    pub height: f32,
    pub advance_x: f32,
    pub advance_y: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub bearing_x: f32,
    pub bearing_y: f32
}

#[derive(Clone, Debug)]
pub enum RunResult {
    Success(FontId, Vec<(usize, char, hb::GlyphInfo, hb::GlyphPosition)>),
    Fail(usize, Segment)
}

pub struct Shaper {
}

impl Shaper {
    pub fn new() -> Self {
        Self {
        }
    }

    pub fn layout(&mut self, x: f32, y: f32, fontdb: &mut FontDb, res: &mut TextLayout, style: &TextStyle<'_>) {
        let mut cursor_x = x;
        let mut cursor_y = y;

        let mut padding = GLYPH_PADDING + style.blur.ceil() as u32;

        let line_width = if let RenderStyle::Stroke { width } = style.render_style {
            padding += width as u32;
            width
        } else {
            0
        };

        match style.align {
            Align::Center => cursor_x -= res.width / 2.0,
            Align::Right => cursor_x -= res.width,
            _ => ()
        }

        res.x = cursor_x;

        // TODO: Error handling

        let mut height = 0.0f32;
        let mut y = cursor_y;

        for glyph in &mut res.glyphs {
            let font = fontdb.get_mut(glyph.font_id).unwrap();
            font.set_size(style.size);

            let xpos = cursor_x + glyph.offset_x + glyph.bearing_x - (padding as f32) - (line_width as f32) / 2.0;
            let ypos = cursor_y + glyph.offset_y - glyph.bearing_y - (padding as f32) - (line_width as f32) / 2.0;

            // Baseline alignment
            let size_metrics = font.face.size_metrics().unwrap();
            let ascender = size_metrics.ascender as f32 / 64.0;
            let descender = size_metrics.descender as f32 / 64.0;

            let offset_y = match style.baseline {
                Baseline::Top => ascender,
                Baseline::Middle => (ascender + descender) / 2.0,
                Baseline::Alphabetic => 0.0,
                Baseline::Bottom => descender,
            };

            height = height.max(ascender - descender);
            y = y.min(ypos + offset_y);

            glyph.x = xpos;
            glyph.y = ypos + offset_y;

            cursor_x += glyph.advance_x + style.letter_spacing;
            cursor_y += glyph.advance_y;
        }

        res.y = y;
        res.height = height;
    }

    fn hb_font(font: &mut Font) -> hb::Owned<hb::Font> {
        // harfbuzz_rs doesn't provide a safe way of creating Face or a Font from a freetype face
        // And I didn't want to read the file a second time and keep it in memory just to give
        // it to harfbuzz_rs here. hb::Owned will free the pointer correctly.

        unsafe {
            let raw_font = hb_ft_font_create_referenced(font.face.raw_mut());
            hb::Owned::from_raw(raw_font)
        }
    }

    pub fn shape(&mut self, x: f32, y: f32, fontdb: &mut FontDb, style: &TextStyle, text: &str) -> TextLayout {
        let mut result = TextLayout {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            glyphs: Vec::new()
        };

        // Layout text for the requested font in style
        let mut shaping_results = self.shape_requested_font(fontdb, style, text);

        // for each of the failed runs of text find a fallback font that will render them
        for res in &mut shaping_results {
            if let RunResult::Fail(start_index, segment) = res {
                let font = match fontdb.fallback(&style, &segment.text) {
                    Ok(font) => font,
                    Err(_) => {
                        println!("Could not find font");
                        continue;
                    }
                };

                font.set_size(style.size);

                let font_id = font.id;

                let hb_font = Self::hb_font(font);
                let buffer = segment.hb_buffer();

                let output = hb::shape(&hb_font, buffer, &[]);
                let positions = output.get_glyph_positions();
                let infos = output.get_glyph_infos();

                let mut glyphs = Vec::new();

                for (position, (info, (idx, c))) in positions.iter().zip(infos.iter().zip(segment.text.char_indices())) {
                    glyphs.push((*start_index + idx, c, *info, *position));
                }

                *res = RunResult::Success(font_id, glyphs);
            }

            if let RunResult::Success(font_id, glyph_infos) = res {
                for (index, c, info, position) in glyph_infos {
                    let font = fontdb.get_mut(*font_id).unwrap();
                    font.set_size(style.size);

                    // TODO: Error handling
                    let _ = font.face.load_glyph(info.codepoint, ft::LoadFlag::DEFAULT | ft::LoadFlag::NO_HINTING);
                    let metrics = font.face.glyph().metrics();

                    let advance_x = position.x_advance as f32 / 64.0;

                    result.width += advance_x;

                    result.glyphs.push(ShapedGlyph {
                        x: 0.0,
                        y: 0.0,
                        c: *c,
                        index: *index,
                        font_id: *font_id,
                        codepoint: info.codepoint,
                        width: metrics.width as f32 / 64.0,
                        height: metrics.height as f32 / 64.0,
                        advance_x: advance_x,
                        advance_y: position.y_advance as f32 / 64.0,
                        offset_x: position.x_offset as f32 / 64.0,
                        offset_y: position.y_offset as f32 / 64.0,
                        bearing_x: metrics.horiBearingX as f32 / 64.0,
                        bearing_y: metrics.horiBearingY as f32 / 64.0,
                    });
                }
            }
        }

        self.layout(x, y, fontdb, &mut result, &style);

        result
    }

    fn shape_requested_font(&mut self, fontdb: &mut FontDb, style: &TextStyle, text: &str) -> Vec<RunResult> {
        let mut result = Vec::new();

        // requested font
        let font = match fontdb.find(&style) {
            Ok(font) => font,
            Err(_) => return result,
        };

        font.set_size(style.size);

        let font_id = font.id;

        let hb_font = Self::hb_font(font);

        let mut index = 0;

        // Reorder characters?
        //let bidi_info = BidiInfo::new(text, None);
        //let para = &bidi_info.paragraphs[0];
        //let line = para.range.clone();
        //segment.text = String::from(bidi_info.reorder_line(para, line));

        use unicode_segmentation::UnicodeSegmentation;

        // for (script, direction, text) in text.unicode_scripts() {
        //
        //     dbg!((script, direction, &text));
        //
        //     let split_iter = if direction == Direction::Rtl {
        //         text.split(" ").rev().peekable()
        //     } else {
        //         text.split(" ").peekable()
        //     };
        //
        //     for word in split_iter {
        //         print!("({})", word);
        //     }
        //
        //     println!();
        // }
        //
        // println!("==================================================");

        // segment the text in runs of the same direction and script
        'segments: for segment in text.segments() {
            let buffer = segment.hb_buffer();

            let output = hb::shape(&hb_font, buffer, &[]);
            let positions = output.get_glyph_positions();
            let infos = output.get_glyph_infos();

            let mut items = Vec::new();

            for (position, (info, c)) in positions.iter().zip(infos.iter().zip(segment.text.chars())) {
                items.push((index, c, *info, *position));
                index += c.len_utf8();

                if info.codepoint == 0 {
                    // missing glyphs from font - mark this segment for fallback
                    let start_index = items.iter().nth(0).map(|item| item.0).unwrap_or(0);
                    result.push(RunResult::Fail(start_index, segment.clone()));
                    continue 'segments;
                }
            }

            result.push(RunResult::Success(font_id, items));
        }

        result
    }
}
