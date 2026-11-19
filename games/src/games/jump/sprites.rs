/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Cursor;

use image::RgbaImage;

const MOUNTAINS_PNG: &[u8] = include_bytes!("../../../sprites/jump/mountains.png");
const BACKGROUND_PNG: &[u8] = include_bytes!("../../../sprites/jump/background.png");
const BUCK_PNG: &[u8] = include_bytes!("../../../sprites/jump/buck.png");
const TREES_PNG: &[u8] = include_bytes!("../../../sprites/jump/trees.png");
const BOULDERS_PNG: &[u8] = include_bytes!("../../../sprites/jump/boulders.png");
const BIRDS_PNG: &[u8] = include_bytes!("../../../sprites/jump/birds.png");

pub struct Sprites {
    pub background_top: RgbaImage,
    pub background_bottom: RgbaImage,
    pub mountains: RgbaImage,
    pub buck: [RgbaImage; 6],
    pub trees: [RgbaImage; 4],
    pub small_boulder: RgbaImage,
    pub large_boulder: RgbaImage,
    pub bird: [RgbaImage; 2],
}

impl Sprites {
    pub fn load() -> Self {
        let background = image::load(Cursor::new(BACKGROUND_PNG), image::ImageFormat::Png).unwrap();
        let mountains = image::load(Cursor::new(MOUNTAINS_PNG), image::ImageFormat::Png).unwrap();
        let buck = image::load(Cursor::new(BUCK_PNG), image::ImageFormat::Png).unwrap();
        let trees = image::load(Cursor::new(TREES_PNG), image::ImageFormat::Png).unwrap();
        let boulders = image::load(Cursor::new(BOULDERS_PNG), image::ImageFormat::Png).unwrap();
        let birds = image::load(Cursor::new(BIRDS_PNG), image::ImageFormat::Png).unwrap();

        Self {
            background_top: background.crop_imm(0, 0, 160, 68).into_rgba8(),
            background_bottom: background.crop_imm(0, 68, 160, 12).into_rgba8(),
            mountains: mountains.into_rgba8(),
            buck: [
                buck.crop_imm(0, 0, 16, 16).into_rgba8(),
                buck.crop_imm(0, 16, 16, 16).into_rgba8(),
                buck.crop_imm(0, 32, 16, 16).into_rgba8(),
                buck.crop_imm(0, 48, 16, 16).into_rgba8(),
                buck.crop_imm(0, 64, 16, 16).into_rgba8(),
                buck.crop_imm(0, 80, 16, 16).into_rgba8(),
            ],
            trees: [
                trees.crop_imm(0, 0, 24, 32).into_rgba8(),
                trees.crop_imm(0, 32, 24, 32).into_rgba8(),
                trees.crop_imm(0, 64, 24, 32).into_rgba8(),
                trees.crop_imm(0, 96, 24, 32).into_rgba8(),
            ],
            small_boulder: boulders.crop_imm(0, 0, 8, 8).into_rgba8(),
            large_boulder: boulders.crop_imm(0, 8, 12, 12).into_rgba8(),
            bird: [
                birds.crop_imm(0, 0, 12, 8).into_rgba8(),
                birds.crop_imm(0, 8, 12, 8).into_rgba8(),
            ],
        }
    }
}
