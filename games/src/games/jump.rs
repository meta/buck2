/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Side-scrolling endless runner with sprite-based parallax rendering.
//!
//! Beyond a basic runner:
//!
//! - **Multi-layer parallax background**: 4 layers scrolling at different
//!   speeds — sky (1/12), mountains (1/7), trees (1/3), ground (2x) —
//!   creating depth. Trees wrap around seamlessly with 4 staggered instances.
//!
//! - **PNG sprite sheets**: all art loaded from embedded PNGs via the `image`
//!   crate and rendered through the `FrameBuffer` with alpha compositing.
//!   The deer has a 6-frame run animation, birds have 2-frame wing flaps.
//!
//! - **5 obstacle types** with distinct behaviors: small/large boulders
//!   (ground-locked, different speeds), and slow/medium/fast birds (airborne,
//!   bob up/down every 8 ticks, spawn at randomized heights).
//!
//! - **Adaptive difficulty**: obstacle gap shrinks linearly from 65→45 pixels
//!   over the first 60 seconds, and bird probability ramps from 30%→75%
//!   over 2 minutes, smoothly increasing challenge.
//!
//! - **Speed-aware spawn timing**: `compute_next_spawn_delay` calculates
//!   when to spawn the next obstacle so that the gap between obstacles
//!   *as they pass the deer* is correct, accounting for different obstacle
//!   speeds (fast birds close the gap quicker than boulders).
//!
//! - **Inset hitbox collision**: the deer's collision box is 2px smaller
//!   than its sprite on all sides, and obstacles are inset 1px, making
//!   near-misses feel fair.
//!
//! - **Airborne animation freeze**: the deer holds a fixed frame (legs
//!   together) while jumping instead of cycling the run animation.

use std::time::Instant;

use rand::Rng;
use rand::RngExt;
use serde::Deserialize;
use serde::Serialize;
use superconsole::Dimensions;
use superconsole::Lines;
use superconsole::components::Component;
use superconsole::components::DrawMode;

use crate::console::Control;
use crate::framebuffer::FrameBuffer;
use crate::games::jump::sprites::Sprites;

mod sprites;
const GROUND_Y: f32 = 10.0;
const GRAVITY: f32 = -0.3;
const JUMP_VY: f32 = 4.0;

const MIN_GAP_START: f32 = 65.0;
const MIN_GAP_END: f32 = 45.0;
const GAP_RAMP_START: u32 = 300; // 10s at 30fps
const GAP_RAMP_END: u32 = 1800; // 60s at 30fps

fn min_gap(tick: u32) -> f32 {
    if tick <= GAP_RAMP_START {
        MIN_GAP_START
    } else if tick >= GAP_RAMP_END {
        MIN_GAP_END
    } else {
        let t = (tick - GAP_RAMP_START) as f32 / (GAP_RAMP_END - GAP_RAMP_START) as f32;
        MIN_GAP_START + t * (MIN_GAP_END - MIN_GAP_START)
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
enum ObstacleKind {
    SmallBoulder,
    LargeBoulder,
    SlowBird,
    MediumBird,
    FastBird,
}

impl ObstacleKind {
    fn width(self) -> i32 {
        match self {
            ObstacleKind::SmallBoulder => 8,
            ObstacleKind::LargeBoulder => 12,
            ObstacleKind::SlowBird | ObstacleKind::MediumBird | ObstacleKind::FastBird => 12,
        }
    }

    fn height(self) -> i32 {
        match self {
            ObstacleKind::SmallBoulder => 8,
            ObstacleKind::LargeBoulder => 12,
            ObstacleKind::SlowBird | ObstacleKind::MediumBird | ObstacleKind::FastBird => 8,
        }
    }

    fn y(self) -> i32 {
        match self {
            ObstacleKind::SmallBoulder | ObstacleKind::LargeBoulder => 11,
            ObstacleKind::SlowBird | ObstacleKind::FastBird => 14,
            ObstacleKind::MediumBird => 16,
        }
    }

    fn speed(self) -> f32 {
        match self {
            ObstacleKind::SmallBoulder | ObstacleKind::LargeBoulder => 2.0,
            ObstacleKind::SlowBird => 1.7,
            ObstacleKind::MediumBird => 2.3,
            ObstacleKind::FastBird => 3.0,
        }
    }
}

struct Obstacle {
    kind: ObstacleKind,
    x: f32,
    base_y: i32,
}

impl Obstacle {
    fn is_bird(&self) -> bool {
        matches!(
            self.kind,
            ObstacleKind::SlowBird | ObstacleKind::MediumBird | ObstacleKind::FastBird
        )
    }

    fn effective_y(&self, tick: u32) -> i32 {
        if self.is_bird() {
            // Bob up and down by 1px every 8 ticks
            self.base_y + ((tick / 8) % 2) as i32
        } else {
            self.base_y
        }
    }
}

fn draw_obstacle(fb: &mut FrameBuffer, sprites: &Sprites, obs: &Obstacle, tick: u32) {
    let x = obs.x as i32;
    let y = obs.effective_y(tick);
    match obs.kind {
        ObstacleKind::SmallBoulder => fb.render_image(x, y, 0, &sprites.small_boulder),
        ObstacleKind::LargeBoulder => fb.render_image(x, y, 0, &sprites.large_boulder),
        ObstacleKind::SlowBird | ObstacleKind::MediumBird | ObstacleKind::FastBird => {
            let frame = ((tick / 6) % 2) as usize;
            fb.render_image(x, y, 0, &sprites.bird[frame]);
        }
    }
}

fn render_background(fb: &mut FrameBuffer, sprites: &Sprites, tick_count: i32) {
    fb.render_image(0, 12, tick_count / 12, &sprites.background_top);
    fb.render_image(0, 12, tick_count / 7, &sprites.mountains);

    let sw = fb.width + 24;
    let trees_offset = (tick_count / 3) % sw;

    fb.render_image(
        ((sw + 12 - trees_offset) % sw) - 24,
        12,
        0,
        &sprites.trees[0],
    );
    fb.render_image(
        ((sw + 52 - trees_offset) % sw) - 24,
        12,
        0,
        &sprites.trees[1],
    );
    fb.render_image(
        ((sw + 92 - trees_offset) % sw) - 24,
        12,
        0,
        &sprites.trees[2],
    );
    fb.render_image(
        ((sw + 132 - trees_offset) % sw) - 24,
        12,
        0,
        &sprites.trees[3],
    );

    fb.render_image(0, 0, tick_count * 2, &sprites.background_bottom);
}

fn random_obstacle_kind(rng: &mut impl Rng, tick: u32) -> ObstacleKind {
    // Bird probability ramps from 30% to 75% over 2 minutes (3600 ticks).
    let bird_pct = if tick >= 3600 {
        75.0_f32
    } else {
        30.0 + 45.0 * tick as f32 / 3600.0
    };
    let roll = rng.random_range(0.0..100.0_f32);
    if roll < bird_pct {
        // Within birds: equal chance for each type
        match rng.random_range(0..3) {
            0 => ObstacleKind::SlowBird,
            1 => ObstacleKind::MediumBird,
            _ => ObstacleKind::FastBird,
        }
    } else {
        // Within boulders: ~57% small, ~43% large (preserving original 4:3 ratio)
        if rng.random_range(0..7) < 4 {
            ObstacleKind::SmallBoulder
        } else {
            ObstacleKind::LargeBoulder
        }
    }
}

fn check_collision(buck_y: f32, obstacles: &[Obstacle], tick: u32) -> bool {
    // Buck hitbox with 2px insets
    let bx1 = 14;
    let by1 = buck_y as i32 + 2;
    let bx2 = 26;
    let by2 = buck_y as i32 + 14;

    for obs in obstacles {
        let oy = obs.effective_y(tick);
        let ox1 = obs.x as i32 + 1;
        let oy1 = oy + 1;
        let ox2 = obs.x as i32 + obs.kind.width() - 1;
        let oy2 = oy + obs.kind.height() - 1;

        if bx1 < ox2 && bx2 > ox1 && by1 < oy2 && by2 > oy1 {
            return true;
        }
    }
    false
}

const SPAWN_X: f32 = 170.0;
const DEER_X: f32 = 20.0;

/// Compute the tick delay before spawning `next_kind` so that when `next_kind`
/// reaches the deer, `current_kind` (spawned now) is `gap` pixels past the deer.
fn compute_next_spawn_delay(current_kind: ObstacleKind, next_kind: ObstacleKind, gap: f32) -> u32 {
    let dist = SPAWN_X - DEER_X; // 150
    let delay = (dist + gap) / current_kind.speed() - dist / next_kind.speed();
    // Clamp to a reasonable minimum so obstacles never overlap at spawn.
    (delay.max(10.0)) as u32
}

pub struct Game {
    framebuffer: FrameBuffer,
    sprites: Sprites,
    buck_y: f32,
    buck_vy: f32,
    on_ground: bool,
    obstacles: Vec<Obstacle>,
    next_kind: ObstacleKind,
    next_spawn_tick: u32,
    elapsed_secs: f64,
    last_tick: Option<Instant>,
    alive: bool,
}

impl Game {
    pub fn new() -> Self {
        let mut rng = rand::rng();
        let next_kind = random_obstacle_kind(&mut rng, 0);
        Self {
            framebuffer: FrameBuffer::new(),
            sprites: Sprites::load(),
            buck_y: GROUND_Y,
            buck_vy: 0.0,
            on_ground: true,
            obstacles: Vec::new(),
            next_kind,
            next_spawn_tick: 60,
            elapsed_secs: 0.0,
            last_tick: None,
            alive: true,
        }
    }

    fn elapsed_secs(&self) -> u64 {
        self.elapsed_secs as u64
    }
}

#[derive(Serialize, Deserialize)]
struct GameState {
    buck_y: f32,
    buck_vy: f32,
    on_ground: bool,
    obstacles: Vec<ObstacleSave>,
    next_kind: ObstacleKind,
    next_spawn_tick: u32,
    alive: bool,
    #[serde(default)]
    elapsed_secs: f64,
}

#[derive(Serialize, Deserialize)]
struct ObstacleSave {
    kind: ObstacleKind,
    x: f32,
    base_y: i32,
}

impl super::Game for Game {
    fn tick(&mut self, tick_count: u32) -> super::TickResult {
        if !self.alive {
            return super::TickResult {
                alive: false,
                scores: vec![],
            };
        }

        // Update jump physics
        if !self.on_ground {
            self.buck_vy += GRAVITY;
            self.buck_y += self.buck_vy;
            if self.buck_y <= GROUND_Y {
                self.buck_y = GROUND_Y;
                self.buck_vy = 0.0;
                self.on_ground = true;
            }
        }

        // Move obstacles
        for obs in &mut self.obstacles {
            obs.x -= obs.kind.speed();
        }

        // Remove off-screen obstacles
        self.obstacles
            .retain(|obs| obs.x > -(obs.kind.width() as f32));

        // Spawn new obstacles
        if tick_count >= self.next_spawn_tick {
            let kind = self.next_kind;
            let mut rng = rand::rng();
            let base_y = if matches!(
                kind,
                ObstacleKind::SlowBird | ObstacleKind::MediumBird | ObstacleKind::FastBird
            ) {
                kind.y() + rng.random_range(-2..=3)
            } else {
                kind.y()
            };
            self.obstacles.push(Obstacle {
                kind,
                x: SPAWN_X,
                base_y,
            });

            self.next_kind = random_obstacle_kind(&mut rng, tick_count);
            let gap = rng.random_range(min_gap(tick_count)..=min_gap(tick_count) + 20.0);
            let delay = compute_next_spawn_delay(kind, self.next_kind, gap);
            self.next_spawn_tick = tick_count + delay;
        }

        // Check collision
        if check_collision(self.buck_y, &self.obstacles, tick_count) {
            self.alive = false;
        }

        // Accumulate elapsed time
        let now = Instant::now();
        if let Some(prev) = self.last_tick {
            self.elapsed_secs += prev.elapsed().as_secs_f64();
        }
        self.last_tick = Some(now);

        // Render
        let tc = tick_count as i32;
        render_background(&mut self.framebuffer, &self.sprites, tc);

        for obs in &self.obstacles {
            draw_obstacle(&mut self.framebuffer, &self.sprites, obs, tick_count);
        }

        let buck_frame = if self.on_ground {
            (tc / 4) % 6
        } else {
            // Use a fixed frame while airborne (frame 1 = legs together)
            1
        };
        self.framebuffer.render_image(
            12,
            self.buck_y as i32,
            0,
            &self.sprites.buck[buck_frame as usize],
        );

        super::TickResult {
            alive: self.alive,
            scores: vec![super::Score {
                category: "jump".to_owned(),
                value: self.elapsed_secs(),
                lower_is_better: false,
            }],
        }
    }

    fn scores(&self) -> Vec<super::Score> {
        vec![super::Score {
            category: "jump".to_owned(),
            value: self.elapsed_secs(),
            lower_is_better: false,
        }]
    }

    fn input(&mut self, input: Control) -> Option<Control> {
        match &input {
            Control::Up | Control::Char('w') | Control::Char(' ') => {
                if self.on_ground {
                    self.buck_vy = JUMP_VY;
                    self.on_ground = false;
                }
                None
            }
            Control::Down
            | Control::Left
            | Control::Right
            | Control::Char('a')
            | Control::Char('s')
            | Control::Char('d') => None,
            _ => Some(input),
        }
    }

    fn save_state(&self) -> Option<String> {
        let state = GameState {
            buck_y: self.buck_y,
            buck_vy: self.buck_vy,
            on_ground: self.on_ground,
            obstacles: self
                .obstacles
                .iter()
                .map(|o| ObstacleSave {
                    kind: o.kind,
                    x: o.x,
                    base_y: o.base_y,
                })
                .collect(),
            next_kind: self.next_kind,
            next_spawn_tick: self.next_spawn_tick,
            alive: self.alive,
            elapsed_secs: self.elapsed_secs,
        };
        serde_json::to_string(&state).ok()
    }

    fn load_state(&mut self, json: &str) -> bool {
        if let Ok(state) = serde_json::from_str::<GameState>(json) {
            self.buck_y = state.buck_y;
            self.buck_vy = state.buck_vy;
            self.on_ground = state.on_ground;
            self.obstacles = state
                .obstacles
                .into_iter()
                .map(|o| Obstacle {
                    kind: o.kind,
                    x: o.x,
                    base_y: o.base_y,
                })
                .collect();
            self.next_kind = state.next_kind;
            self.next_spawn_tick = state.next_spawn_tick;
            self.alive = state.alive;
            self.elapsed_secs = state.elapsed_secs;
            self.last_tick = None;
            true
        } else {
            false
        }
    }
}

impl Component for Game {
    type Error = anyhow::Error;

    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> anyhow::Result<Lines> {
        let mut lines = self.framebuffer.draw_unchecked(dimensions, mode)?;
        if !matches!(mode, DrawMode::Final) {
            lines.0.insert(
                0,
                vec![format!("Time: {}s", self.elapsed_secs())]
                    .try_into()
                    .unwrap(),
            );
        }
        Ok(lines)
    }
}
