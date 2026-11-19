/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use superconsole::Dimensions;
use superconsole::Lines;
use superconsole::components::Component;
use superconsole::components::DrawMode;
use superconsole::style::Color;

use crate::console::Control;
use crate::console::control_to_direction;
use crate::framebuffer::FrameBuffer;

pub fn shuffle(framebuffer: &mut FrameBuffer) {
    for x in 0..160 {
        for y in 0..80 {
            framebuffer.display[y][x] = Color::Rgb {
                r: rand::random(),
                g: rand::random(),
                b: rand::random(),
            }
        }
    }
}

pub struct Game {
    framebuffer: FrameBuffer,
    next_update: u32,
}

impl Game {
    pub fn new() -> Self {
        let mut framebuffer = FrameBuffer::new();
        shuffle(&mut framebuffer);
        Self {
            framebuffer,
            next_update: 10,
        }
    }
}

impl super::Game for Game {
    fn tick(&mut self, tick_count: u32) -> super::TickResult {
        if tick_count == self.next_update {
            shuffle(&mut self.framebuffer);
            self.next_update += 6;
        }
        super::TickResult {
            alive: true,
            scores: vec![],
        }
    }

    fn input(&mut self, input: Control) -> Option<Control> {
        if control_to_direction(&input).is_some() {
            None
        } else {
            Some(input)
        }
    }

    fn save_state(&self) -> Option<String> {
        serde_json::to_string(&self.next_update).ok()
    }

    fn load_state(&mut self, json: &str) -> bool {
        if let Ok(next_update) = serde_json::from_str::<u32>(json) {
            self.next_update = next_update;
            true
        } else {
            false
        }
    }
}

impl Component for Game {
    type Error = anyhow::Error;

    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> anyhow::Result<Lines> {
        self.framebuffer.draw_unchecked(dimensions, mode)
    }
}
