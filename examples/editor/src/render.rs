use std::collections::HashMap;

use vello_cpu::color::{AlphaColor, Srgb};
use vello_cpu::kurbo::{Affine, Rect};
use vello_cpu::peniko::{Blob, FontData};
use vello_cpu::{Glyph, Pixmap, RenderContext, Resources};
use winkin::Item;

use crate::editor::Editor;

pub const MARGIN: f32 = 32.0;

pub struct Renderer {
    context: RenderContext,
    resources: Resources,
    fonts: HashMap<(u64, u32), FontData>,
    pixmap: Pixmap,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            context: RenderContext::new(1, 1),
            resources: Resources::new(),
            fonts: HashMap::new(),
            pixmap: Pixmap::new(1, 1),
        }
    }

    pub fn draw(
        &mut self,
        editor: &Editor,
        width: u16,
        height: u16,
        scale: f64,
        scroll: f32,
        caret: bool,
    ) -> &Pixmap {
        let ctx = &mut self.context;
        ctx.reset_and_resize(width, height);
        if self.pixmap.width() != width || self.pixmap.height() != height {
            self.pixmap = Pixmap::new(width, height);
        }
        ctx.set_paint(color(24, 27, 33));
        ctx.fill_rect(&Rect::new(0.0, 0.0, f64::from(width), f64::from(height)));
        ctx.set_transform(
            Affine::scale(scale)
                * Affine::translate((f64::from(MARGIN), f64::from(MARGIN - scroll))),
        );
        ctx.set_paint(color(49, 76, 112));
        for rect in editor.layout.selection_rects(editor.selection.range()) {
            let line = editor.layout.line(rect.line).unwrap().metrics();
            ctx.fill_rect(&Rect::new(
                f64::from(line.left + rect.inline.left),
                f64::from(line.top + rect.block.over),
                f64::from(line.left + rect.inline.right),
                f64::from(line.top + rect.block.under),
            ));
        }
        ctx.set_paint(color(225, 229, 237));
        for line in editor.layout.lines() {
            let metrics = line.metrics();
            if metrics.top + metrics.height() < scroll - MARGIN
                || metrics.top > scroll + f32::from(height) / scale as f32
            {
                continue;
            }
            for item in line.items() {
                let (Item::Text(run) | Item::Generated(run)) = item else {
                    continue;
                };
                let Some(font) = run.font() else { continue };
                let data = self
                    .fonts
                    .entry((font.bytes.id(), font.index))
                    .or_insert_with(|| {
                        FontData::new(
                            Blob::from_raw_parts(font.bytes.arc().clone(), font.bytes.id()),
                            font.index,
                        )
                    });
                let mut glyphs = ctx
                    .glyph_run(&mut self.resources, data)
                    .font_size(font.size)
                    .normalized_coords(font.coords)
                    .hint(true);
                if let Some(skew) = font.skew {
                    glyphs = glyphs
                        .glyph_transform(Affine::skew(f64::from(skew).to_radians().tan(), 0.0));
                }
                glyphs.fill_glyphs(run.glyphs().map(|glyph| Glyph {
                    id: glyph.id,
                    x: glyph.x + metrics.left,
                    y: glyph.y + metrics.top,
                }));
            }
        }
        if let Some(range) = editor.composition_range() {
            ctx.set_paint(color(131, 180, 255));
            for rect in editor.layout.selection_rects(range) {
                let line = editor.layout.line(rect.line).unwrap().metrics();
                let y = line.top + rect.block.under;
                ctx.fill_rect(&Rect::new(
                    f64::from(line.left + rect.inline.left),
                    f64::from(y - 1.0),
                    f64::from(line.left + rect.inline.right),
                    f64::from(y),
                ));
            }
        }
        if caret
            && editor.caret_visible()
            && editor.selection.is_collapsed()
            && let Some(caret) = editor.layout.caret(editor.selection.focus())
        {
            let line = editor.layout.line(caret.line).unwrap().metrics();
            let x = f64::from(line.left + caret.inline.left);
            ctx.set_paint(color(225, 229, 237));
            ctx.fill_rect(&Rect::new(
                x,
                f64::from(line.top + caret.block.over),
                x + 1.0 / scale,
                f64::from(line.top + caret.block.under),
            ));
        }
        ctx.flush();
        ctx.render(&mut self.pixmap, &mut self.resources);
        &self.pixmap
    }
}

fn color(r: u8, g: u8, b: u8) -> AlphaColor<Srgb> {
    AlphaColor::from_rgba8(r, g, b, 255)
}
