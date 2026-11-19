/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use image::RgbaImage;
use superconsole::Dimensions;
use superconsole::Line;
use superconsole::Lines;
use superconsole::Span;
use superconsole::components::Component;
use superconsole::components::DrawMode;
use superconsole::style::Color;
use superconsole::style::Stylize;

/// A fixed-size pixel framebuffer that renders as half-block Unicode characters.
///
/// Each cell maps to a terminal color. The display is rendered using the "▄"
/// (lower half block) character with foreground/background colors to pack two
/// vertical pixels into a single character cell.
#[derive(Debug)]
pub struct FrameBuffer {
    pub display: Box<[[Color; 160]; 80]>,
    pub width: i32,
    pub height: i32,
}

impl FrameBuffer {
    pub fn new() -> Self {
        Self {
            display: vec![[Color::Black; 160]; 80]
                .into_boxed_slice()
                .try_into()
                .unwrap(),
            width: 160,
            height: 80,
        }
    }

    pub fn set(&mut self, x: i32, y: i32, r: u8, g: u8, b: u8) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return;
        }
        self.display[y as usize][x as usize] = Color::Rgb { r, g, b }
    }

    pub fn render_image(&mut self, x: i32, y: i32, rot_x: i32, img: &RgbaImage) {
        let img_height = img.height() as i32;
        let img_width = img.width() as i32;
        let rot_x = rot_x % img_width;
        for (ix, iy, pix) in img.enumerate_pixels() {
            let adj_y = y + img_height - (iy as i32) - 1;
            let adj_x = x + ((ix as i32) + img_width - rot_x) % img_width;
            if pix.0[3] > 0 {
                self.set(adj_x, adj_y, pix.0[0], pix.0[1], pix.0[2]);
            }
        }
    }
}

impl Component for FrameBuffer {
    type Error = anyhow::Error;

    fn draw_unchecked(&self, _dimensions: Dimensions, mode: DrawMode) -> anyhow::Result<Lines> {
        const C: &str = "▄";
        Ok(match mode {
            DrawMode::Final => Lines::new(),
            DrawMode::Normal => {
                let inner_width = self.display[0].len();
                let mut lines: Vec<Line> = Vec::with_capacity(self.display.len() + 2);
                lines.push(
                    vec!["╔".to_owned(), "═".repeat(inner_width), "╗".to_owned()]
                        .try_into()
                        .unwrap(),
                );
                for y in (0..self.display.len()).step_by(2).rev() {
                    let mut line = Line::default();
                    line.push(Span::new_unstyled_lossy("║"));
                    for x in 0..self.display[y].len() {
                        let top = self.display[y + 1][x];
                        let bottom = self.display[y][x];
                        line.push(Span::new_styled_lossy_str(C.with(bottom).on(top)));
                    }
                    line.push(Span::new_unstyled_lossy("║"));
                    lines.push(line);
                }
                lines.push(
                    vec!["╚".to_owned(), "═".repeat(inner_width), "╝".to_owned()]
                        .try_into()
                        .unwrap(),
                );
                Lines(lines)
            }
        })
    }
}
