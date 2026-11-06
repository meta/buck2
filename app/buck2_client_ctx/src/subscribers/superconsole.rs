/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use buck2_data::CommandExecutionDetails;
use buck2_error::BuckErrorContext;
use buck2_event_observer::action_sub_error_display::ActionSubErrorDisplay;
use buck2_event_observer::display;
use buck2_event_observer::display::TargetDisplayOptions;
use buck2_event_observer::display::display_file_watcher_end;
use buck2_event_observer::event_observer::DebugEventObserverExtra;
use buck2_event_observer::re_state::ReState;
use buck2_event_observer::session_info::SessionInfo;
use buck2_event_observer::two_snapshots::TwoSnapshots;
use buck2_event_observer::unpack_event::VisitorError;
use buck2_event_observer::unpack_event::unpack_event;
use buck2_event_observer::verbosity::Verbosity;
use buck2_event_observer::what_ran::WhatRanCommandConsoleFormat;
use buck2_event_observer::what_ran::WhatRanOutputCommand;
use buck2_event_observer::what_ran::WhatRanOutputWriter;
use buck2_event_observer::what_ran::command_to_string;
use buck2_event_observer::what_ran::worker_command_as_fallback_to_string;
use buck2_events::BuckEvent;
use buck2_health_check::report::DisplayReport;
use buck2_wrapper_common::invocation_id::TraceId;
use strum::IntoEnumIterator;
use superconsole::Component;
use superconsole::Dimensions;
use superconsole::DrawMode;
use superconsole::Line;
use superconsole::Lines;
use superconsole::Span;
pub(crate) use superconsole::SuperConsole;
use superconsole::components::DrawVertical;
use superconsole::style::Attribute;
use superconsole::style::Color;
use superconsole::style::ContentStyle;
use superconsole::style::StyledContent;
use superconsole::style::Stylize;
use tokio::sync::mpsc::Receiver;

use crate::console_interaction_stream::ConsoleInteraction;
use crate::console_interaction_stream::ConsoleKey;
use crate::console_interaction_stream::SuperConsoleToggle;
use crate::subscribers::console_output_limit::EmitResult;
use crate::subscribers::emit_event::emit_event_if_relevant;
use crate::subscribers::simpleconsole::SimpleConsole;
use crate::subscribers::subscriber::EventSubscriber;
use crate::subscribers::superconsole::commands::CommandsComponent;
use crate::subscribers::superconsole::debug_events::DebugEventsComponent;
use crate::subscribers::superconsole::debugger::StarlarkDebuggerComponent;
use crate::subscribers::superconsole::dice::DiceComponent;
use crate::subscribers::superconsole::header::TasksHeader;
use crate::subscribers::superconsole::io::IoHeader;
use crate::subscribers::superconsole::re::ReHeader;
use crate::subscribers::superconsole::session_info::SessionInfoComponent;
use crate::subscribers::superconsole::system_warning::SystemWarningComponent;
use crate::subscribers::superconsole::test::TestHeader;
use crate::subscribers::superconsole::timed_list::Cutoffs;
use crate::subscribers::superconsole::timed_list::TimedList;
use crate::subscribers::superconsole::timekeeper::Timekeeper;
use crate::ticker::Tick;

mod commands;
mod common;
pub(crate) mod debug_events;
mod debugger;
pub(crate) mod dice;
mod header;
pub(crate) mod io;
mod message_renderer;
mod re;
pub mod session_info;
pub(crate) mod system_warning;
pub mod timekeeper;

pub mod test;
pub mod timed_list;

const SUPERCONSOLE_WIDTH: usize = 300;

/// Games list matching the standalone menu binary.
const GAMES: &[(&str, &str)] = &[
    ("blocks", "color matching"),
    ("snake", "classic snake"),
    ("jump", "side scroller"),
    ("2048", "tile slider"),
    ("minesweeper", "mine finder"),
    ("breakout", "brick breaker"),
    ("sokoban", "box pusher"),
    ("life", "cellular automaton"),
    ("static", "random colors"),
];

fn games_save_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_owned());
    PathBuf::from(home).join(".buck2_games")
}

fn games_save_path_for(game_name: &str) -> PathBuf {
    games_save_dir().join(format!("{}.json", game_name))
}

fn games_has_save(game_name: &str) -> bool {
    games_save_path_for(game_name).exists()
}

fn build_high_scores_lines() -> superconsole::Lines {
    use games::games::HighScores;
    use games::games::menu::bordered_line;
    use games::games::menu::hline;
    use superconsole::style::Stylize;

    let hs_path = games_save_dir().join("high_scores.json");
    let high_scores = HighScores::load(&hs_path);

    let mut lines: Vec<superconsole::Line> = vec![
        superconsole::Line::default(),
        hline("\u{2554}", "\u{2557}"),
        bordered_line(vec![superconsole::Span::new_styled_lossy(
            " HIGH SCORES".to_owned().bold(),
        )]),
        hline("\u{2560}", "\u{2563}"),
    ];

    let mut any_scores = false;
    for &(game_name, _) in GAMES {
        if let Some(entries) = high_scores.get(game_name) {
            if entries.is_empty() {
                continue;
            }
            any_scores = true;
            lines.push(bordered_line(vec![]));
            lines.push(bordered_line(vec![superconsole::Span::new_styled_lossy(
                format!("  {game_name}").bold(),
            )]));
            for (category, entry) in entries {
                let label = format!("    {:<30}{:>7}", category, entry.value);
                lines.push(bordered_line(vec![superconsole::Span::new_unstyled_lossy(
                    &label,
                )]));
            }
        }
    }

    if !any_scores {
        lines.push(bordered_line(vec![]));
        lines.push(bordered_line(vec![superconsole::Span::new_unstyled_lossy(
            " No high scores yet.",
        )]));
    }

    lines.push(bordered_line(vec![]));
    lines.push(hline("\u{255a}", "\u{255d}"));
    lines.push(superconsole::Line::default());
    lines.push(vec!["  Esc to go back"].try_into().unwrap());

    superconsole::Lines(lines)
}

fn make_game(idx: usize) -> (Box<dyn games::games::Game>, &'static str) {
    match idx {
        0 => (Box::new(games::games::blocks::Game::new()), "blocks"),
        1 => (Box::new(games::games::snake::Game::new()), "snake"),
        2 => (Box::new(games::games::jump::Game::new()), "jump"),
        3 => (Box::new(games::games::twenty48::Game::new()), "2048"),
        4 => (
            Box::new(games::games::minesweeper::Game::new()),
            "minesweeper",
        ),
        5 => (Box::new(games::games::breakout::Game::new()), "breakout"),
        6 => (Box::new(games::games::sokoban::Game::new()), "sokoban"),
        7 => (Box::new(games::games::life::Game::new()), "life"),
        8 => (Box::new(games::games::r#static::Game::new()), "static"),
        _ => unreachable!(),
    }
}

/// State machine for parsing multi-byte escape sequences from raw chars.
///
/// Because the buck2 integration receives characters one at a time via
/// `SuperConsoleToggle::key()`, we can't do async timeouts like `control_reader`.
/// Instead we track how many ticks have elapsed since we entered `SawEscape` and
/// let the tick loop call `flush()` to emit a bare Escape after a short delay.
enum EscapeState {
    Normal,
    /// Saw 0x1b, waiting for follow-up. `ticks_waiting` counts how many render
    /// ticks have passed so we can time out and emit a bare Escape.
    SawEscape {
        ticks_waiting: u32,
    },
    SawBracket,
    /// CSI with parameter bytes collected so far (e.g. "1;2").
    CsiParam(Vec<char>),
    /// Saw 0xe0 prefix (legacy cmd.exe arrow key encoding).
    SawE0,
}

impl EscapeState {
    /// Feed a raw character and return `Control` values if any are complete.
    /// Returns (first, second) — second is Some only when a bare Escape is
    /// followed by a non-sequence char (both need to be delivered).
    fn feed(
        &mut self,
        c: char,
    ) -> (
        Option<games::console::Control>,
        Option<games::console::Control>,
    ) {
        use games::console::Control;

        let state = std::mem::replace(self, EscapeState::Normal);
        match state {
            EscapeState::Normal => {
                if c as u32 == 0x1b {
                    *self = EscapeState::SawEscape { ticks_waiting: 0 };
                    (None, None)
                } else if c as u32 == 0xe0 || c as u32 == 0x00 {
                    // Legacy cmd.exe sends 0xe0 or 0x00 prefix for arrow/function keys.
                    *self = EscapeState::SawE0;
                    (None, None)
                } else {
                    (Some(Control::Char(c)), None)
                }
            }
            EscapeState::SawEscape { .. } => {
                if c as u32 == 0x5b {
                    *self = EscapeState::SawBracket;
                    (None, None)
                } else if c as u32 == 0x1b {
                    // Another escape: emit the pending one, start a new one.
                    *self = EscapeState::SawEscape { ticks_waiting: 0 };
                    (Some(Control::Escape), None)
                } else {
                    // Non-sequence char after escape: emit both.
                    (Some(Control::Escape), Some(Control::Char(c)))
                }
            }
            EscapeState::SawBracket => match c {
                'A' => (Some(Control::Up), None),
                'B' => (Some(Control::Down), None),
                'C' => (Some(Control::Right), None),
                'D' => (Some(Control::Left), None),
                '0'..='9' => {
                    *self = EscapeState::CsiParam(vec![c]);
                    (None, None)
                }
                _ => (None, None), // Unknown CSI sequence, drop it
            },
            EscapeState::SawE0 => {
                // Legacy cmd.exe scan codes: H=Up, P=Down, K=Left, M=Right
                match c {
                    'H' => (Some(Control::Up), None),
                    'P' => (Some(Control::Down), None),
                    'K' => (Some(Control::Left), None),
                    'M' => (Some(Control::Right), None),
                    _ => (None, None),
                }
            }
            EscapeState::CsiParam(mut params) => {
                if c.is_ascii_alphabetic() {
                    // Final byte — interpret the sequence
                    let is_shift = params == ['1', ';', '2'];
                    match (c, is_shift) {
                        ('A', _) => (Some(Control::Up), None),
                        ('B', _) => (Some(Control::Down), None),
                        ('C', true) => (Some(Control::ShiftRight), None),
                        ('D', true) => (Some(Control::ShiftLeft), None),
                        ('C', false) => (Some(Control::Right), None),
                        ('D', false) => (Some(Control::Left), None),
                        _ => (None, None),
                    }
                } else {
                    params.push(c);
                    *self = EscapeState::CsiParam(params);
                    (None, None)
                }
            }
        }
    }

    /// Called each tick (~33ms at 30fps). If we've been waiting in SawEscape
    /// for 2+ ticks (~66ms, comparable to control_reader's 50ms timeout),
    /// emit a bare Escape.
    fn tick(&mut self) -> Option<games::console::Control> {
        if let EscapeState::SawEscape { ticks_waiting } = self {
            *ticks_waiting += 1;
            if *ticks_waiting >= 2 {
                *self = EscapeState::Normal;
                return Some(games::console::Control::Escape);
            }
        }
        None
    }
}

enum GamesMode {
    Menu(games::games::menu::Menu),
    Playing {
        game: Box<dyn games::games::Game>,
        name: &'static str,
        tick_count: u32,
    },
    /// Game has ended; show game-over overlay until Escape.
    GameOver {
        game: Box<dyn games::games::Game>,
        message: String,
    },
    /// Viewing high scores; Escape returns to menu.
    HighScores(superconsole::Lines),
}

struct GamesOverlay {
    g_press_count: u8,
    active: bool,
    escape_state: EscapeState,
    mode: GamesMode,
    menu_tick_count: u32,
}

impl GamesOverlay {
    fn new() -> Self {
        Self {
            g_press_count: 0,
            active: false,
            escape_state: EscapeState::Normal,
            mode: GamesMode::Menu(Self::new_menu()),
            menu_tick_count: 0,
        }
    }

    fn new_menu() -> games::games::menu::Menu {
        let items: Vec<games::games::menu::MenuItem> = GAMES
            .iter()
            .map(|(name, desc)| games::games::menu::MenuItem {
                name,
                description: desc,
            })
            .collect();
        games::games::menu::Menu::new(items, Box::new(games_has_save))
    }

    fn activate(&mut self) {
        self.active = true;
        self.escape_state = EscapeState::Normal;
        self.mode = GamesMode::Menu(Self::new_menu());
    }

    fn deactivate(&mut self) {
        self.active = false;
        self.g_press_count = 0;
    }
}

pub const CUTOFFS: Cutoffs = Cutoffs {
    inform: Duration::from_secs(4),
    warn: Duration::from_secs(8),
    _notable: Duration::from_millis(200),
};

#[allow(clippy::large_enum_variant)]
pub enum StatefulSuperConsole {
    Running(StatefulSuperConsoleImpl),
    /// After receiving the command output, any stdout, or an event stream error, the superconsole
    /// will be "finalized" and any further events are handled by simpleconsole.
    Finalized(SimpleConsole<DebugEventObserverExtra>),
}

/// Where a `StatefulSuperConsoleImpl`'s output goes.
pub enum ConsoleCanvas {
    /// Owns the terminal: emitted lines scroll above the canvas, and this console's
    /// own render/finalize/erase calls draw to the terminal.
    Terminal(SuperConsole),
}

impl ConsoleCanvas {
    fn emit(&mut self, lines: Lines) {
        match self {
            Self::Terminal(sc) => sc.emit(lines),
        }
    }

    fn emit_aux(&mut self, lines: Lines) {
        match self {
            Self::Terminal(sc) => sc.emit_aux(lines),
        }
    }

    fn render<C: Component + ?Sized>(
        &mut self,
        root: &C,
    ) -> Result<(), superconsole::Error<C::Error>> {
        match self {
            Self::Terminal(sc) => sc.render(root),
        }
    }

    fn finalize(
        self,
        root: &BuckRootComponent,
    ) -> Result<(), superconsole::Error<buck2_error::Error>> {
        match self {
            Self::Terminal(sc) => sc.finalize(root),
        }
    }

    #[cfg(test)]
    fn testing_terminal(&mut self) -> &mut SuperConsole {
        match self {
            Self::Terminal(sc) => sc,
        }
    }
}

impl WhatRanOutputWriter for ConsoleCanvas {
    fn emit_command(&mut self, command: WhatRanOutputCommand<'_>) -> buck2_error::Result<()> {
        let msg = WhatRanCommandConsoleFormat {
            reason: command.reason,
            identity: command.identity,
            repro: command.repro,
        }
        .to_string();
        self.emit(Lines(vec![Line::sanitized(&msg)]));
        Ok(())
    }
}

pub struct StatefulSuperConsoleImpl {
    header: String,
    state: SuperConsoleState,
    canvas: ConsoleCanvas,
    verbosity: Verbosity,
    games_overlay: GamesOverlay,
    shown_interactive_console_message: bool,
}

pub struct SuperConsoleState {
    timekeeper: Timekeeper,
    /// This contains the SpanTracker, which is why it's part of the SuperConsoleState.
    simple_console: SimpleConsole<DebugEventObserverExtra>,
    config: SuperConsoleConfig,
    active_warnings: Option<Vec<DisplayReport>>,
}

impl SuperConsoleState {
    pub fn extra(&self) -> &DebugEventObserverExtra {
        self.simple_console.observer.extra()
    }
}

#[derive(Clone)]
pub struct SuperConsoleConfig {
    pub enable_dice: bool,
    pub enable_debug_events: bool,
    pub enable_detailed_re: bool,
    pub enable_io: bool,
    pub enable_commands: bool,
    pub display_platform: bool,
    pub expanded_progress: bool,
    /// Two lines for root events with single child event.
    pub two_lines: bool,
    pub max_lines: usize,
    /// Extra styled line(s) rendered at the very top of the canvas, e.g. `buck2 log
    /// snoop`'s `Snooping <trace>: <argv>` banner. Part of the canvas so that erasing
    /// or redrawing the console replaces it too. Callers are responsible for
    /// sanitizing the content (e.g. via `Span::sanitized`).
    pub banner: Option<Lines>,
    /// Do not emit "File changed:" notifications above the canvas. Used by viewers
    /// like `buck2 log snoop` where they would pile up as scrollback across
    /// invocation switches.
    pub hide_file_watcher_events: bool,
}

impl Default for SuperConsoleConfig {
    fn default() -> Self {
        Self {
            enable_dice: false,
            enable_debug_events: false,
            enable_detailed_re: false,
            enable_io: false,
            enable_commands: false,
            expanded_progress: true,
            display_platform: false,
            two_lines: false,
            max_lines: 10,
            banner: None,
            hide_file_watcher_events: false,
        }
    }
}

struct BuckRootComponent<'s> {
    header: &'s str,
    state: &'s SuperConsoleState,
    games_overlay: &'s GamesOverlay,
}

/// Adapter that wraps a `Component<Error = anyhow::Error>` to produce
/// `buck2_error::Error`, allowing games components to be used in the
/// buck2 render tree.
struct AnyhowComponentAdapter<'a, C: ?Sized>(&'a C);

impl<C: superconsole::Component<Error = anyhow::Error> + ?Sized> Component
    for AnyhowComponentAdapter<'_, C>
{
    type Error = buck2_error::Error;

    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> buck2_error::Result<Lines> {
        self.0
            .draw_unchecked(dimensions, mode)
            .map_err(|e| buck2_error::buck2_error!(buck2_error::ErrorTag::Tier0, "{:#}", e))
    }
}

/// Renders a game with a game-over overlay box on top.
struct GameOverAdapter<'a> {
    game: &'a dyn games::games::Game,
    message: &'a str,
}

impl Component for GameOverAdapter<'_> {
    type Error = buck2_error::Error;

    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> buck2_error::Result<Lines> {
        let mut output = self
            .game
            .draw_unchecked(dimensions, mode)
            .map_err(|e| buck2_error::buck2_error!(buck2_error::ErrorTag::Tier0, "{:#}", e))?;
        let msg = format!(" {} \u{2014} press Esc to exit ", self.message);
        games::games::render_overlay_box(&mut output, &msg);
        Ok(output)
    }
}

/// Renders static pre-built lines (used for high scores display).
struct StaticLinesAdapter<'a>(&'a Lines);

impl Component for StaticLinesAdapter<'_> {
    type Error = buck2_error::Error;

    fn draw_unchecked(
        &self,
        _dimensions: Dimensions,
        mode: DrawMode,
    ) -> buck2_error::Result<Lines> {
        Ok(match mode {
            DrawMode::Final => Lines::new(),
            DrawMode::Normal => self.0.clone(),
        })
    }
}

impl Component for BuckRootComponent<'_> {
    type Error = buck2_error::Error;

    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> buck2_error::Result<Lines> {
        // bound all components to our recommended grapheme-width
        let dimensions = dimensions.intersect(Dimensions {
            width: SUPERCONSOLE_WIDTH,
            height: usize::MAX,
        });

        let mut draw = DrawVertical::new(dimensions);

        if let Some(banner) = &self.state.config.banner {
            draw.draw(&StaticLinesAdapter(banner), mode)?;
        }

        // Render games overlay above normal build output when active.
        if self.games_overlay.active {
            match &self.games_overlay.mode {
                GamesMode::Menu(menu) => {
                    draw.draw(&AnyhowComponentAdapter(menu), mode)?;
                }
                GamesMode::Playing { game, .. } => {
                    draw.draw(&AnyhowComponentAdapter(game.as_ref()), mode)?;
                }
                GamesMode::GameOver { game, message } => {
                    let adapter = GameOverAdapter {
                        game: game.as_ref(),
                        message,
                    };
                    draw.draw(&adapter, mode)?;
                }
                GamesMode::HighScores(lines) => {
                    draw.draw(&StaticLinesAdapter(lines), mode)?;
                }
            }
        }

        let last_snapshot = self
            .state
            .simple_console
            .observer
            .two_snapshots()
            .last
            .as_ref()
            .map(|s| &s.1);
        let system_info = self.state.simple_console.observer.system_info();
        let health_check_reports = self.state.active_warnings.as_ref();
        {
            draw.draw(
                &SystemWarningComponent {
                    last_snapshot,
                    system_info,
                    health_check_reports,
                },
                mode,
            )?;
        }

        draw.draw(
            &SessionInfoComponent {
                session_info: self.state.session_info(),
                re_state: self.state.simple_console.observer.re_state(),
                two_snapshots: self.state.simple_console.observer.two_snapshots(),
            },
            mode,
        )?;
        draw.draw(
            &ReHeader {
                super_console_config: &self.state.config,
                re_state: self.state.simple_console.observer.re_state(),
                two_snapshots: self.state.simple_console.observer.two_snapshots(),
            },
            mode,
        )?;
        draw.draw(
            &IoHeader {
                super_console_config: &self.state.config,
                two_snapshots: self.state.simple_console.observer.two_snapshots(),
            },
            mode,
        )?;
        draw.draw(
            &TestHeader {
                session_info: self.state.session_info(),
                test_state: self.state.simple_console.observer.test_state(),
            },
            mode,
        )?;
        draw.draw(
            &DebugEventsComponent {
                super_console_config: &self.state.config,
                debug_events_state: self.state.simple_console.observer.extra().debug_events(),
            },
            mode,
        )?;
        draw.draw(
            &DiceComponent {
                super_console_config: &self.state.config,
                dice_state: self.state.simple_console.observer.dice_state(),
            },
            mode,
        )?;
        draw.draw(
            &StarlarkDebuggerComponent {
                starlark_debugger_state: self
                    .state
                    .simple_console
                    .observer
                    .starlark_debugger_state(),
            },
            mode,
        )?;
        draw.draw(
            &CommandsComponent {
                super_console_config: &self.state.config,
                action_stats: self.state.simple_console.observer.action_stats(),
            },
            mode,
        )?;
        draw.draw(&TasksHeader::new(self.header, self.state), mode)?;
        draw.draw(&TimedList::new(&CUTOFFS, self.state), mode)?;

        Ok(draw.finish())
    }
}

impl StatefulSuperConsole {
    pub const FALLBACK_SIZE: Dimensions = Dimensions {
        width: 100,
        height: 40,
    };

    pub(crate) fn new_with_root_forced(
        trace_id: TraceId,
        command_name: &str,
        verbosity: Verbosity,
        expect_spans: bool,
        timekeeper: Timekeeper,
        stream: Option<Box<dyn Write + Send + 'static + Sync>>,
        config: SuperConsoleConfig,
        health_check_reports_receiver: Option<Receiver<Vec<DisplayReport>>>,
    ) -> buck2_error::Result<Self> {
        let mut builder = Self::console_builder();
        if let Some(stream) = stream {
            builder.write_to(stream);
        }
        Self::new(
            command_name,
            trace_id,
            ConsoleCanvas::Terminal(builder.build_forced(Self::FALLBACK_SIZE)?),
            verbosity,
            expect_spans,
            timekeeper,
            config,
            health_check_reports_receiver,
        )
    }

    pub(crate) fn new(
        command_name: &str,
        trace_id: TraceId,
        canvas: ConsoleCanvas,
        verbosity: Verbosity,
        expect_spans: bool,
        timekeeper: Timekeeper,
        config: SuperConsoleConfig,
        health_check_reports_receiver: Option<Receiver<Vec<DisplayReport>>>,
    ) -> buck2_error::Result<Self> {
        let header = format!("Command {command_name}");
        Ok(Self::Running(StatefulSuperConsoleImpl {
            header,
            state: SuperConsoleState::new(
                timekeeper,
                trace_id,
                verbosity,
                expect_spans,
                config,
                health_check_reports_receiver,
            )?,
            canvas,
            verbosity,
            games_overlay: GamesOverlay::new(),
            shown_interactive_console_message: false,
        }))
    }

    /// Construct a console suitable for use by the Buck2 CLI. We use non-blocking output here
    /// because we do all our event processing on a single thread, so that if stderr is blocked
    /// (e.g.  because the client is using a resumable remote terminal and they've temporarily
    /// disconnected), we don't delay ingesting new events.
    ///
    /// This ensures we a) don't have to catch up when the client reconnects, b) don't buffer
    /// events (though we might buffer output), and c) can show an accurate "time elapsed" when the
    /// client returns (if we wait until the client returns to resume, we'll always report that the
    /// command finished "just now", because we'll have some events to catch up on).
    pub fn console_builder() -> ::superconsole::Builder {
        let mut builder = ::superconsole::Builder::new();
        builder.non_blocking();
        builder
    }

    pub fn render_result_errors(result: &buck2_cli_proto::CommandResult) -> Lines {
        let mut lines = Lines::new();
        if let buck2_cli_proto::CommandResult {
            result: Some(buck2_cli_proto::command_result::Result::Error(e)),
        } = result
        {
            let style = ContentStyle {
                foreground_color: Some(Color::DarkRed),
                ..Default::default()
            };
            lines
                .0
                .extend(Lines::from_multiline_string_raw(&e.message, style).0);
        }
        lines
    }

    async fn handle_event(&mut self, ev: &Arc<BuckEvent>) -> buck2_error::Result<()> {
        match self {
            Self::Running(c) => c.handle_event(ev).await,
            Self::Finalized(c) => c.handle_event(ev).await,
        }
    }

    fn finalize(&mut self) -> buck2_error::Result<()> {
        let mut res = Ok(());
        take_mut::take(self, |this| match this {
            Self::Running(super_console) => {
                let (state, err) = super_console.finalize();
                if let Some(err) = err {
                    res = Err(err);
                }
                Self::Finalized(state.simple_console)
            }
            v => v,
        });

        res.map_err(Into::into)
    }

    fn erase(&mut self) -> buck2_error::Result<()> {
        let mut res = Ok(());
        take_mut::take(self, |this| match this {
            Self::Running(super_console) => {
                let (state, err) = super_console.erase();
                if let Some(err) = err {
                    res = Err(err);
                }
                Self::Finalized(state.simple_console)
            }
            v => v,
        });

        res.map_err(Into::into)
    }
}

impl SuperConsoleState {
    pub fn new(
        timekeeper: Timekeeper,
        trace_id: TraceId,
        verbosity: Verbosity,
        expect_spans: bool,
        config: SuperConsoleConfig,
        health_check_reports_receiver: Option<Receiver<Vec<DisplayReport>>>,
    ) -> buck2_error::Result<SuperConsoleState> {
        Ok(SuperConsoleState {
            timekeeper,
            simple_console: SimpleConsole::with_tty(
                trace_id,
                verbosity,
                expect_spans,
                health_check_reports_receiver,
            ),
            config,
            active_warnings: None,
        })
    }

    pub async fn update_event_observer(
        &mut self,
        event: &Arc<BuckEvent>,
    ) -> buck2_error::Result<()> {
        self.simple_console.update_event_observer(event).await
    }

    pub fn session_info(&self) -> &SessionInfo {
        self.simple_console.observer.session_info()
    }

    pub fn re_state(&self) -> &ReState {
        self.simple_console.observer.re_state()
    }

    pub fn two_snapshots(&self) -> &TwoSnapshots {
        self.simple_console.observer.two_snapshots()
    }

    pub fn tick(&mut self, tick: Tick) {
        self.timekeeper.tick(tick);
    }
}

pub(crate) const BUCK_NO_INTERACTIVE_CONSOLE: &str = "BUCK_NO_INTERACTIVE_CONSOLE";

impl StatefulSuperConsoleImpl {
    async fn toggle(
        &mut self,
        what: &str,
        key: char,
        var: impl FnOnce(&mut Self) -> &mut bool,
    ) -> buck2_error::Result<()> {
        let var = var(self);
        *var = !*var;
        let on_off = match *var {
            true => "on",
            false => "off",
        };
        self.handle_stderr(&format!("{what}: {on_off}, press `{key}` to revert"))
            .await
    }

    async fn handle_event(&mut self, event: &Arc<BuckEvent>) -> buck2_error::Result<()> {
        self.state.update_event_observer(event).await?;

        self.handle_inner_event(event)
            .await
            .with_buck_error_context(|| display::InvalidBuckEvent(event.clone()).to_string())?;

        if self.verbosity.print_all_commands() {
            emit_event_if_relevant(
                event.parent_id().into(),
                event.data(),
                self.state.simple_console.observer().spans(),
                &mut self.canvas,
            )?;
        }
        Ok(())
    }

    async fn handle_inner_event(&mut self, event: &BuckEvent) -> buck2_error::Result<()> {
        match unpack_event(event)? {
            buck2_event_observer::unpack_event::UnpackedBuckEvent::SpanStart(_, _, _) => Ok(()),
            buck2_event_observer::unpack_event::UnpackedBuckEvent::SpanEnd(_, _, data) => {
                match data {
                    buck2_data::span_end_event::Data::ActionExecution(action) => {
                        self.handle_action_execution_end(action).await
                    }
                    buck2_data::span_end_event::Data::FileWatcher(file_watcher) => {
                        self.handle_file_watcher_end(file_watcher).await
                    }
                    _ => Ok(()),
                }
            }
            buck2_event_observer::unpack_event::UnpackedBuckEvent::Instant(_, _, data) => {
                match data {
                    buck2_data::instant_event::Data::ConsoleMessage(message) => {
                        self.handle_console_message(message).await
                    }
                    buck2_data::instant_event::Data::ConsoleWarning(message) => {
                        self.handle_console_warning(message).await
                    }
                    buck2_data::instant_event::Data::StructuredError(err) => {
                        self.handle_structured_error(err).await
                    }
                    buck2_data::instant_event::Data::TestResult(result) => {
                        self.handle_test_result(result).await
                    }
                    buck2_data::instant_event::Data::ConsolePreferences(preferences) => {
                        self.handle_console_preferences(preferences).await
                    }
                    buck2_data::instant_event::Data::ActionError(error) => {
                        self.handle_action_error(error).await
                    }
                    buck2_data::instant_event::Data::StreamingOutput(message) => {
                        self.handle_streaming_output(message).await
                    }
                    _ => Ok(()),
                }
            }
            buck2_event_observer::unpack_event::UnpackedBuckEvent::UnrecognizedSpanStart(_, _)
            | buck2_event_observer::unpack_event::UnpackedBuckEvent::UnrecognizedSpanEnd(_, _)
            | buck2_event_observer::unpack_event::UnpackedBuckEvent::UnrecognizedInstant(_, _) => {
                Err(VisitorError::MissingField(event.clone()).into())
            }
        }
    }

    async fn handle_stderr(&mut self, msg: &str) -> buck2_error::Result<()> {
        self.canvas.emit(msg.lines().map(Line::sanitized).collect());
        Ok(())
    }

    async fn handle_structured_error(
        &mut self,
        err: &buck2_data::StructuredError,
    ) -> buck2_error::Result<()> {
        if err.quiet {
            return Ok(());
        }

        self.canvas.emit(
            err.payload
                .lines()
                .map(|line| Line::from_iter([Span::new_colored_lossy(line, Color::DarkYellow)]))
                .collect(),
        );
        Ok(())
    }

    async fn handle_file_watcher_end(
        &mut self,
        file_watcher: &buck2_data::FileWatcherEnd,
    ) -> buck2_error::Result<()> {
        if self.verbosity.print_status() && !self.state.config.hide_file_watcher_events {
            self.canvas.emit(Lines(
                display_file_watcher_end(file_watcher)
                    .into_iter()
                    .map(|x| Line::sanitized(&x))
                    .collect::<Vec<_>>(),
            ));
        }

        Ok(())
    }

    async fn handle_console_message(
        &mut self,
        message: &buck2_data::ConsoleMessage,
    ) -> buck2_error::Result<()> {
        // TODO(nmj): Maybe better handling of messages that have color data in them. Right now
        //            they're just stripped
        self.canvas.emit(Lines::from_multiline_string_raw(
            &message.message,
            ContentStyle::default(),
        ));
        Ok(())
    }

    async fn handle_streaming_output(
        &mut self,
        message: &buck2_data::StdoutStreamingOutput,
    ) -> buck2_error::Result<()> {
        self.canvas.emit_aux(Lines::from_multiline_string_raw(
            &message.message,
            ContentStyle::default(),
        ));
        Ok(())
    }

    async fn handle_console_warning(
        &mut self,
        message: &buck2_data::ConsoleWarning,
    ) -> buck2_error::Result<()> {
        let style = ContentStyle {
            foreground_color: Some(Color::Yellow),
            ..Default::default()
        };
        self.canvas
            .emit(Lines::from_multiline_string_raw(&message.message, style));
        Ok(())
    }

    async fn handle_action_execution_end(
        &mut self,
        action: &buck2_data::ActionExecutionEnd,
    ) -> buck2_error::Result<()> {
        if action.error.is_some() {
            // Don't handle action errors here. We deal with them as a part of a separate
            // `ActionError` event
            return Ok(());
        }

        if let Some(stderr) = display::success_stderr(action, self.verbosity)? {
            match self.state.simple_console.output_limit.emit(stderr.len()) {
                EmitResult::Emit => {
                    let mut lines = vec![];
                    let display_platform = self.state.config.display_platform;
                    let action_id = StyledContent::new(
                        ContentStyle {
                            attributes: Attribute::Bold.into(),
                            ..Default::default()
                        },
                        format!(
                            "stderr for {}:",
                            display::display_action_identity(
                                action.key.as_ref(),
                                action.name.as_ref(),
                                TargetDisplayOptions::for_console(display_platform),
                            )?
                        ),
                    );
                    lines.push(Line::from_iter([Span::new_styled_lossy(action_id)]));
                    lines.extend(Lines::from_colored_multiline_string(stderr));
                    self.canvas.emit(Lines(lines));
                }
                EmitResult::Exceeded(msg) => {
                    self.canvas.emit(Lines(vec![Line::sanitized(msg)]));
                }
                EmitResult::Skipped => {}
            }
        }

        Ok(())
    }

    async fn handle_action_error(
        &mut self,
        error: &buck2_data::ActionError,
    ) -> buck2_error::Result<()> {
        let mut lines = vec![];
        let display_platform = self.state.config.display_platform;

        let error_display = display::display_action_error(
            error,
            TargetDisplayOptions::for_console(display_platform),
        )?;

        let stream_bytes = error_display.output_stream_byte_count();
        let output_format = match self.state.simple_console.output_limit.emit(stream_bytes) {
            EmitResult::Emit => display::ActionErrorOutputFormat::IncludeOutputStreams,
            EmitResult::Exceeded(msg) => {
                display::ActionErrorOutputFormat::SubstituteOutputStreams(msg)
            }
            EmitResult::Skipped => display::ActionErrorOutputFormat::ExcludeOutputStreams,
        };
        let include_output_streams = matches!(
            output_format,
            display::ActionErrorOutputFormat::IncludeOutputStreams
        );

        let display::ActionErrorDisplay {
            action_id,
            reason,
            command,
            error_diagnostics,
        } = error_display;

        lines.push(Line::from_iter([Span::new_styled_lossy(
            StyledContent::new(
                ContentStyle {
                    attributes: Attribute::Bold.into(),
                    foreground_color: Some(Color::Red),
                    ..Default::default()
                },
                format!("Action failed: {action_id}",),
            ),
        )]));

        lines.extend(
            reason.lines().map(|l| {
                Line::from_iter([Span::new_styled_lossy(l.to_owned().with(Color::DarkRed))])
            }),
        );

        if let Some(command) = command {
            lines_for_command_details(command, self.verbosity, include_output_streams, &mut lines);
        }

        if include_output_streams {
            if let Some(error_diagnostics) = error_diagnostics {
                match error_diagnostics.data.as_ref().unwrap() {
                    buck2_data::action_error_diagnostics::Data::SubErrors(sub_errors) => {
                        let sub_errors = &sub_errors.sub_errors;
                        if !sub_errors.is_empty() {
                            for sub_error in sub_errors {
                                // Display errors based on show_in_stderr flag is true
                                if sub_error.show_in_stderr {
                                    if let Some(display_msg) = sub_error.display() {
                                        lines.push(Line::from_iter([Span::new_styled_lossy(
                                            display_msg.with(Color::DarkCyan),
                                        )]))
                                    }
                                }
                            }
                        }
                    }
                    buck2_data::action_error_diagnostics::Data::HandlerInvocationError(error) => {
                        let colored_error = error.clone().with(Color::DarkRed).to_string();
                        lines.extend(Lines::from_colored_multiline_string(&colored_error));
                    }
                };
            }
        }

        if let display::ActionErrorOutputFormat::SubstituteOutputStreams(msg) = output_format {
            lines.push(Line::sanitized(msg));
        }

        self.canvas.emit(Lines(lines));

        Ok(())
    }

    async fn handle_test_result(
        &mut self,
        result: &buck2_data::TestResult,
    ) -> buck2_error::Result<()> {
        if let Some(msg) = display::format_test_result(result, self.verbosity)? {
            let byte_count: usize = msg.0.iter().map(|line| line.len()).sum();
            match self.state.simple_console.output_limit.emit(byte_count) {
                EmitResult::Emit => {
                    self.canvas.emit(msg);
                }
                EmitResult::Exceeded(msg) => {
                    self.canvas.emit(Lines(vec![Line::sanitized(msg)]));
                }
                EmitResult::Skipped => {}
            }
        }

        Ok(())
    }

    async fn handle_console_preferences(
        &mut self,
        prefs: &buck2_data::ConsolePreferences,
    ) -> buck2_error::Result<()> {
        self.state.config.max_lines = prefs.max_lines.try_into()?;

        Ok(())
    }

    async fn handle_console_interaction(
        &mut self,
        c: &ConsoleInteraction,
    ) -> buck2_error::Result<()> {
        let c = match c {
            ConsoleInteraction::Toggle(c) => c,
            ConsoleInteraction::Key(key) => {
                self.games_overlay.g_press_count = 0;
                if self.games_overlay.active {
                    use games::console::Control;

                    match key {
                        ConsoleKey::Escape => self.handle_games_control(Control::Escape),
                        ConsoleKey::Up => self.handle_games_control(Control::Up),
                        ConsoleKey::Down => self.handle_games_control(Control::Down),
                        ConsoleKey::Left => self.handle_games_control(Control::Left),
                        ConsoleKey::Right => self.handle_games_control(Control::Right),
                        ConsoleKey::ShiftLeft => self.handle_games_control(Control::ShiftLeft),
                        ConsoleKey::ShiftRight => self.handle_games_control(Control::ShiftRight),
                        ConsoleKey::Other => {}
                    }
                }
                return Ok(());
            }
            ConsoleInteraction::Resize => {
                self.canvas.render(&BuckRootComponent {
                    header: &self.header,
                    state: &self.state,
                    games_overlay: &self.games_overlay,
                })?;
                return Ok(());
            }
        };

        // When games overlay is active, route all input to games.
        if self.games_overlay.active {
            let raw_char = c.key();
            let (first, second) = self.games_overlay.escape_state.feed(raw_char);
            if let Some(control) = first {
                self.handle_games_control(control);
            }
            if let Some(control) = second {
                self.handle_games_control(control);
            }
            return Ok(());
        }

        // Games not active — check for 'g' activation sequence.
        match c {
            SuperConsoleToggle::Char('g') => {
                self.games_overlay.g_press_count += 1;
                if self.games_overlay.g_press_count >= 3 {
                    self.games_overlay.activate();
                }
                return Ok(());
            }
            _ => {
                // Any other recognized key resets the g counter.
                self.games_overlay.g_press_count = 0;
            }
        }

        // Normal toggle handling.
        match c {
            SuperConsoleToggle::Dice => {
                self.toggle(c.description(), c.key(), |s| {
                    &mut s.state.config.enable_dice
                })
                .await?
            }
            SuperConsoleToggle::DebugEvents => {
                self.toggle(c.description(), c.key(), |s| {
                    &mut s.state.config.enable_debug_events
                })
                .await?
            }
            SuperConsoleToggle::TwoLinesMode => {
                self.toggle(c.description(), c.key(), |s| &mut s.state.config.two_lines)
                    .await?
            }
            SuperConsoleToggle::DetailedRE => {
                self.toggle(c.description(), c.key(), |s| {
                    &mut s.state.config.enable_detailed_re
                })
                .await?
            }
            SuperConsoleToggle::Io => {
                self.toggle(c.description(), c.key(), |s| &mut s.state.config.enable_io)
                    .await?
            }
            SuperConsoleToggle::TargetConfigurations => {
                self.toggle(c.description(), c.key(), |s| {
                    &mut s.state.config.display_platform
                })
                .await?
            }
            SuperConsoleToggle::ExpandedProgress => {
                self.toggle(c.description(), c.key(), |s| {
                    &mut s.state.config.expanded_progress
                })
                .await?
            }
            SuperConsoleToggle::Commands => {
                self.toggle(c.description(), c.key(), |s| {
                    &mut s.state.config.enable_commands
                })
                .await?
            }
            SuperConsoleToggle::IncrLines => {
                self.state.config.max_lines = self.state.config.max_lines.saturating_add(1)
            }
            SuperConsoleToggle::DecrLines => {
                self.state.config.max_lines = self.state.config.max_lines.saturating_sub(1)
            }
            SuperConsoleToggle::IncreaseReplaySpeed => {
                if let Some(message) = self.state.timekeeper.scale_speed(1.5).await {
                    self.handle_stderr(&message).await?;
                }
            }
            SuperConsoleToggle::DecreaseReplaySpeed => {
                if let Some(message) = self.state.timekeeper.scale_speed(1.0 / 1.5).await {
                    self.handle_stderr(&message).await?;
                }
            }
            SuperConsoleToggle::PauseReplay => {
                if let Some(message) = self.state.timekeeper.toggle_pause().await {
                    self.handle_stderr(&message).await?;
                }
            }
            SuperConsoleToggle::Help => {
                // The speed keys' meaning depends on the command's clock (e.g. `log
                // replay` scales speed, `log snoop` switches invocations), so their
                // help text comes from it too.
                let speed_keys = self.state.timekeeper.speed_key_descriptions();
                let help_message = SuperConsoleToggle::iter()
                    .map(|t| {
                        let action = match t {
                            SuperConsoleToggle::IncreaseReplaySpeed => {
                                speed_keys.increase.to_owned()
                            }
                            SuperConsoleToggle::DecreaseReplaySpeed => {
                                speed_keys.decrease.to_owned()
                            }
                            _ => format!("toggle {}", t.description()),
                        };
                        format!("`{}` = {}", t.key(), action)
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                self.handle_stderr(&format!(
                    "Help:\n{}\n`g` x3 = games\nenv var {}=true disables interactive console",
                    help_message, BUCK_NO_INTERACTIVE_CONSOLE
                ))
                .await?
            }
            SuperConsoleToggle::Char(_) => {
                if !self.shown_interactive_console_message {
                    self.shown_interactive_console_message = true;
                    self.handle_stderr(&format!(
                        "Buck2 has an interactive console; input is consumed. \
                         Press `h` for help or set {}=true to disable.",
                        BUCK_NO_INTERACTIVE_CONSOLE
                    ))
                    .await?;
                }
            }
        }

        Ok(())
    }

    fn handle_games_control(&mut self, control: games::console::Control) {
        use games::console::Control;
        use games::games::menu::MenuResult;

        enum Action {
            None,
            Deactivate,
            StartGame(usize, bool),
            ReturnToMenu,
            ShowHighScores,
        }

        let action = match &mut self.games_overlay.mode {
            GamesMode::Menu(menu) => {
                if matches!(control, Control::Escape) {
                    Action::Deactivate
                } else if let Some(result) = menu.input(control) {
                    match result {
                        MenuResult::NewGame(idx) => Action::StartGame(idx, false),
                        MenuResult::Continue(idx) => Action::StartGame(idx, true),
                        MenuResult::ViewHighScores => Action::ShowHighScores,
                    }
                } else {
                    Action::None
                }
            }
            GamesMode::Playing {
                game,
                name,
                tick_count,
            } => {
                if matches!(control, Control::Escape) {
                    // Save state before returning to menu.
                    if let Some(state) = game.save_state() {
                        let envelope = serde_json::json!({
                            "tick_count": *tick_count,
                            "state": state,
                        });
                        let path = games_save_path_for(name);
                        if let Some(parent) = path.parent() {
                            drop(std::fs::create_dir_all(parent));
                        }
                        if let Ok(json) = serde_json::to_string(&envelope) {
                            drop(std::fs::write(&path, json));
                        }
                    }
                    Action::ReturnToMenu
                } else {
                    drop(game.input(control));
                    Action::None
                }
            }
            GamesMode::GameOver { .. } => {
                if matches!(control, Control::Escape) {
                    Action::ReturnToMenu
                } else {
                    Action::None
                }
            }
            GamesMode::HighScores(_) => {
                if matches!(control, Control::Escape | Control::Char('q')) {
                    Action::ReturnToMenu
                } else {
                    Action::None
                }
            }
        };

        match action {
            Action::None => {}
            Action::Deactivate => {
                self.games_overlay.deactivate();
            }
            Action::StartGame(idx, load) => {
                let game_name = GAMES[idx].0;
                if !load {
                    let path = games_save_path_for(game_name);
                    drop(std::fs::remove_file(&path));
                }
                let (mut game, name) = make_game(idx);
                if load {
                    let path = games_save_path_for(name);
                    if let Ok(data) = std::fs::read_to_string(&path) {
                        if let Ok(envelope) = serde_json::from_str::<serde_json::Value>(&data) {
                            if let Some(state) = envelope.get("state").and_then(|s| s.as_str()) {
                                game.load_state(state);
                            }
                        }
                    }
                }
                self.games_overlay.mode = GamesMode::Playing {
                    game,
                    name,
                    tick_count: 0,
                };
            }
            Action::ReturnToMenu => {
                self.games_overlay.mode = GamesMode::Menu(GamesOverlay::new_menu());
            }
            Action::ShowHighScores => {
                self.games_overlay.mode = GamesMode::HighScores(build_high_scores_lines());
            }
        }
    }

    fn try_update_active_warnings(&mut self) {
        let reports = self
            .state
            .simple_console
            .try_recv_health_check_display_reports();
        if reports.is_some() {
            self.state.active_warnings = reports;
        }
    }
    async fn tick(&mut self, tick: &Tick) -> buck2_error::Result<()> {
        self.state.timekeeper.tick(*tick);
        self.try_update_active_warnings();

        // Tick games when active.
        if self.games_overlay.active {
            // Flush pending escape if it has timed out.
            if let Some(control) = self.games_overlay.escape_state.tick() {
                self.handle_games_control(control);
            }
            self.games_overlay.menu_tick_count += 1;
            let menu_tick = self.games_overlay.menu_tick_count;
            let mut transition = None;
            match &mut self.games_overlay.mode {
                GamesMode::Menu(menu) => {
                    menu.tick(menu_tick);
                }
                GamesMode::Playing {
                    game,
                    tick_count,
                    name,
                } => {
                    *tick_count += 1;
                    let result = game.tick(*tick_count);
                    if !result.alive {
                        // Record scores.
                        let scores = if game.scores().is_empty() {
                            result.scores
                        } else {
                            game.scores()
                        };
                        if !scores.is_empty() {
                            let hs_path = games_save_dir().join("high_scores.json");
                            let mut high_scores = games::games::HighScores::load(&hs_path);
                            high_scores.record(name, &scores);
                            high_scores.save(&hs_path);
                        }
                        // Delete save file on game over.
                        let path = games_save_path_for(name);
                        drop(std::fs::remove_file(&path));
                        let message = game.game_over_message().unwrap_or("Game Over!").to_owned();
                        transition = Some(message);
                    }
                }
                GamesMode::GameOver { .. } | GamesMode::HighScores(_) => {
                    // Nothing to tick — just waiting for Escape.
                }
            };
            if let Some(message) = transition {
                // Move the game out of Playing into GameOver.
                let old = std::mem::replace(
                    &mut self.games_overlay.mode,
                    GamesMode::Menu(GamesOverlay::new_menu()),
                );
                if let GamesMode::Playing { game, .. } = old {
                    self.games_overlay.mode = GamesMode::GameOver { game, message };
                }
            }
        }

        self.canvas.render(&BuckRootComponent {
            header: &self.header,
            state: &self.state,
            games_overlay: &self.games_overlay,
        })?;
        Ok(())
    }

    async fn handle_command_result(
        &mut self,
        result: &buck2_cli_proto::CommandResult,
    ) -> buck2_error::Result<()> {
        let lines = StatefulSuperConsole::render_result_errors(result);
        self.canvas.emit(lines);
        Ok(())
    }

    fn finalize(
        self,
    ) -> (
        SuperConsoleState,
        Option<superconsole::Error<buck2_error::Error>>,
    ) {
        let err = self
            .canvas
            .finalize(&BuckRootComponent {
                header: &self.header,
                state: &self.state,
                games_overlay: &self.games_overlay,
            })
            .err();
        (self.state, err)
    }

    /// Erase the canvas from the terminal instead of leaving a final frame behind,
    /// for when the display is being replaced rather than concluded.
    fn erase(
        mut self,
    ) -> (
        SuperConsoleState,
        Option<superconsole::Error<buck2_error::Error>>,
    ) {
        let err = self
            .canvas
            .render(&superconsole::components::Blank)
            .err()
            .map(|e| match e {
                superconsole::Error::Draw(infallible) => match infallible {},
                superconsole::Error::Output(e) => superconsole::Error::Output(e),
            });
        (self.state, err)
    }
}

#[async_trait]
impl EventSubscriber for StatefulSuperConsole {
    async fn handle_events(&mut self, events: &[Arc<BuckEvent>]) -> buck2_error::Result<()> {
        for ev in events {
            self.handle_event(ev).await?;
        }
        Ok(())
    }

    async fn handle_output(&mut self, raw_output: &[u8]) -> buck2_error::Result<()> {
        self.finalize()?;
        match self {
            Self::Running(_) => unreachable!(),
            Self::Finalized(c) => c.handle_output(raw_output).await,
        }
    }

    async fn handle_tailer_stderr(&mut self, stderr: &str) -> buck2_error::Result<()> {
        match self {
            StatefulSuperConsole::Running(c) => c.handle_stderr(stderr).await,
            StatefulSuperConsole::Finalized(c) => c.handle_stderr(stderr).await,
        }
    }

    async fn handle_console_interaction(
        &mut self,
        c: &ConsoleInteraction,
    ) -> buck2_error::Result<()> {
        if let Self::Running(super_console) = self {
            super_console.handle_console_interaction(c).await?;
        }

        Ok(())
    }

    async fn handle_command_result(
        &mut self,
        result: &buck2_cli_proto::CommandResult,
    ) -> buck2_error::Result<()> {
        match self {
            Self::Running(c) => c.handle_command_result(result).await?,
            Self::Finalized(c) => c.handle_command_result(result).await?,
        }
        self.finalize()?;
        Ok(())
    }

    async fn tick(&mut self, tick: &Tick) -> buck2_error::Result<()> {
        if let Self::Running(super_console) = self {
            super_console.tick(tick).await?;
        }
        Ok(())
    }

    fn desired_ticks_per_second(&self) -> Option<u32> {
        if let Self::Running(c) = self {
            if c.games_overlay.active {
                return Some(30);
            }
        }
        None
    }

    async fn handle_error(&mut self, _error: &buck2_error::Error) -> buck2_error::Result<()> {
        self.finalize()?;
        Ok(())
    }

    async fn erase_interactive_output(&mut self) -> buck2_error::Result<()> {
        self.erase()
    }
}

fn lines_for_command_details(
    command_failed: &CommandExecutionDetails,
    verbosity: Verbosity,
    include_output_streams: bool,
    lines: &mut Vec<Line>,
) {
    if let Some(command_kind) = command_failed.command_kind.as_ref() {
        use buck2_data::command_execution_kind::Command;

        match command_kind.command.as_ref() {
            Some(Command::LocalCommand(local_command)) => {
                let command = command_to_string(local_command);
                let command = command.as_str();
                let command = if verbosity.print_failure_full_command() {
                    Cow::Borrowed(command)
                } else {
                    match truncate(command) {
                        None => Cow::Borrowed(command),
                        Some(short) => Cow::Owned(format!(
                            "{short} (run `buck2 log what-failed` to get the full command)"
                        )),
                    }
                };

                lines.push(Line::from_iter([Span::new_unstyled_lossy(format!(
                    "Reproduce locally: `{command}`"
                ))]));
            }
            Some(Command::RemoteCommand(remote_command)) => {
                let help_message = if buck2_core::is_open_source() {
                    format!("Remote action digest: `{}`", remote_command.action_digest)
                } else {
                    format!(
                        "Reproduce locally: `frecli cas download-action {}`",
                        remote_command.action_digest
                    )
                };

                lines.push(Line::from_iter([Span::new_styled_lossy(
                    help_message.with(Color::DarkRed),
                )]));
            }
            Some(Command::OmittedLocalCommand(..)) | Some(Command::InFlightCommand(..)) | None => {
                // Nothing to show in this case.
            }
            Some(Command::WorkerInitCommand(worker_init_command)) => {
                let command = command_to_string(worker_init_command);
                let command = command.as_str();
                let command = if verbosity.print_failure_full_command() {
                    Cow::Borrowed(command)
                } else {
                    match truncate(command) {
                        None => Cow::Borrowed(command),
                        Some(short) => Cow::Owned(format!(
                            "{short} (run `buck2 log what-failed` to get the full command)"
                        )),
                    }
                };

                lines.push(Line::from_iter([Span::new_unstyled_lossy(format!(
                    "Reproduce locally: `{command}`"
                ))]));
            }
            Some(Command::WorkerCommand(worker_command)) => {
                let command = worker_command_as_fallback_to_string(worker_command);
                let command = command.as_str();
                let command = if verbosity.print_failure_full_command() {
                    Cow::Borrowed(command)
                } else {
                    match truncate(command) {
                        None => Cow::Borrowed(command),
                        Some(short) => Cow::Owned(format!(
                            "{short} (run `buck2 log what-failed` to get the full command)"
                        )),
                    }
                };

                lines.push(Line::from_iter([Span::new_unstyled_lossy(format!(
                    "Reproduce locally: `{command}`"
                ))]));
            }
        };
    }

    if !include_output_streams {
        return;
    }

    lines.push(Line::from_iter([Span::new_styled_lossy(
        "stdout:"
            .to_owned()
            .with(Color::DarkRed)
            .attribute(Attribute::Bold),
    )]));
    lines.extend(Lines::from_colored_multiline_string(
        &command_failed.cmd_stdout,
    ));
    lines.push(Line::from_iter([Span::new_styled_lossy(
        "stderr:"
            .to_owned()
            .with(Color::DarkRed)
            .attribute(Attribute::Bold),
    )]));
    lines.extend(Lines::from_colored_multiline_string(
        &command_failed.cmd_stderr,
    ));

    if let Some(additional_message) = &command_failed.additional_message {
        if !additional_message.is_empty() {
            lines.push(Line::from_iter([Span::new_styled_lossy(
                "info:"
                    .to_owned()
                    .with(Color::DarkRed)
                    .attribute(Attribute::Bold),
            )]));
            lines.extend(Lines::from_colored_multiline_string(additional_message));
        }
    }
}

// Truncates a string to a reasonable number characters, or returns None if it doesn't need truncating.
fn truncate(contents: &str) -> Option<String> {
    const MAX_LENGTH: usize = 200;
    const BUFFER: usize = " ...<omitted>... ".len();
    if contents.len() > MAX_LENGTH + BUFFER {
        Some(format!(
            "{} ...<omitted>... {}",
            &contents[0..contents.ceil_char_boundary(MAX_LENGTH / 2)],
            &contents
                [contents.floor_char_boundary(contents.len() - MAX_LENGTH / 2)..contents.len()]
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use buck2_cli_proto::CommandResult;
    use buck2_cli_proto::GenericResponse;
    use buck2_data::LoadBuildFileEnd;
    use buck2_data::LoadBuildFileStart;
    use buck2_data::SpanEndEvent;
    use buck2_data::SpanStartEvent;
    use buck2_error::BuckErrorOptionContext;
    use buck2_event_observer::span_tracker::EventTimestamp;
    use buck2_events::span::SpanId;
    use dupe::Dupe;
    use superconsole::testing::SuperConsoleTestingExt;
    use superconsole::testing::assert_frame_contains;
    use superconsole::testing::test_console;

    use super::*;
    use crate::subscribers::superconsole::timekeeper::RealtimeClock;

    #[tokio::test]
    async fn test_transfer_state_to_simpleconsole() -> buck2_error::Result<()> {
        let trace_id = TraceId::new();
        let mut console = StatefulSuperConsole::new_with_root_forced(
            trace_id.dupe(),
            "test",
            Verbosity::default(),
            true,
            Timekeeper::new(
                Box::new(RealtimeClock),
                EventTimestamp(SystemTime::now().into()),
            ),
            None,
            Default::default(),
            None,
        )
        .unwrap();

        // start a new event.
        let id = SpanId::next();
        let event = Arc::new(BuckEvent::new(
            SystemTime::now(),
            trace_id,
            Some(id),
            None,
            buck2_data::buck_event::Data::SpanStart(SpanStartEvent {
                data: Some(buck2_data::span_start_event::Data::Load(
                    LoadBuildFileStart {
                        module_id: "foo".to_owned(),
                        cell: "bar".to_owned(),
                    },
                )),
            }),
        ));
        console.handle_event(&event).await.unwrap();

        // drop into simple console
        console
            .handle_command_result(&CommandResult {
                result: Some(buck2_cli_proto::command_result::Result::GenericResponse(
                    GenericResponse {},
                )),
            })
            .await
            .unwrap();

        // finish the event from before
        // expect to successfully close event.
        let event = Arc::new(BuckEvent::new(
            SystemTime::now(),
            TraceId::new(),
            Some(id),
            None,
            buck2_data::buck_event::Data::SpanEnd(SpanEndEvent {
                data: Some(buck2_data::span_end_event::Data::Load(LoadBuildFileEnd {
                    module_id: "foo".to_owned(),
                    cell: "bar".to_owned(),
                    error: None,
                    starlark_peak_allocated_bytes: Some(0),
                    cpu_instruction_count: None,
                    target_count: Some(10),
                    starlark_tick_count: None,
                })),
                stats: None,
                duration: None,
            }),
        ));
        assert!(console.handle_event(&event).await.is_ok());
        Ok(())
    }

    #[tokio::test]
    async fn test_default_layout() -> buck2_error::Result<()> {
        let trace_id = TraceId::new();
        let now = SystemTime::now();
        let tick = Tick::now();

        let mut console = StatefulSuperConsole::new(
            "build",
            trace_id.dupe(),
            ConsoleCanvas::Terminal(test_console()),
            Verbosity::default(),
            true,
            Timekeeper::new(
                Box::new(RealtimeClock),
                EventTimestamp(SystemTime::now().into()),
            ),
            Default::default(),
            None,
        )?;

        console
            .handle_event(&Arc::new(BuckEvent::new(
                now,
                trace_id.dupe(),
                Some(SpanId::next()),
                None,
                buck2_data::buck_event::Data::SpanStart(SpanStartEvent {
                    data: Some(
                        buck2_data::CommandStart {
                            data: Some(buck2_data::BuildCommandStart {}.into()),
                            ..Default::default()
                        }
                        .into(),
                    ),
                }),
            )))
            .await?;

        console
            .handle_event(&Arc::new(BuckEvent::new(
                now,
                trace_id.dupe(),
                None,
                None,
                buck2_data::InstantEvent {
                    data: Some(
                        buck2_data::RemoteExecutionSessionCreated {
                            session_id: "reSessionID-123".to_owned(),
                            experiment_name: "".to_owned(),
                            persistent_cache_mode: None,
                        }
                        .into(),
                    ),
                }
                .into(),
            )))
            .await?;

        console
            .handle_event(&Arc::new(BuckEvent::new(
                now,
                trace_id.dupe(),
                Some(SpanId::next()),
                None,
                SpanStartEvent {
                    data: Some(
                        LoadBuildFileStart {
                            module_id: "foo".to_owned(),
                            cell: "bar".to_owned(),
                        }
                        .into(),
                    ),
                }
                .into(),
            )))
            .await?;

        console.tick(&tick).await?;

        let frame = match &mut console {
            StatefulSuperConsole::Running(c) => c
                .canvas
                .testing_terminal()
                .test_output_mut()
                .frames
                .pop()
                .internal_error("No frame was emitted")?,
            StatefulSuperConsole::Finalized(_) => {
                panic!("Console was downgraded");
            }
        };

        // Verify we have the right output on intermediate frames
        if cfg!(fbcode_build) {
            assert_frame_contains(&frame, "Buck UI:");
        } else {
            assert_frame_contains(&frame, "Build ID:");
        }
        assert_frame_contains(&frame, "Loading targets");

        console
            .handle_command_result(&buck2_cli_proto::CommandResult { result: None })
            .await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_banner_rendered() -> buck2_error::Result<()> {
        let trace_id = TraceId::new();
        let tick = Tick::now();

        let config = SuperConsoleConfig {
            banner: Some(Lines(vec![Line::from_iter([Span::new_styled_lossy(
                " SNOOP v2 ".to_owned().bold().reverse(),
            )])])),
            ..Default::default()
        };
        let mut console = StatefulSuperConsole::new(
            "build",
            trace_id.dupe(),
            ConsoleCanvas::Terminal(test_console()),
            Verbosity::default(),
            true,
            Timekeeper::new(
                Box::new(RealtimeClock),
                EventTimestamp(SystemTime::now().into()),
            ),
            config,
            None,
        )?;

        console.tick(&tick).await?;

        let frame = match &mut console {
            StatefulSuperConsole::Running(c) => c
                .canvas
                .testing_terminal()
                .test_output_mut()
                .frames
                .pop()
                .internal_error("No frame was emitted")?,
            StatefulSuperConsole::Finalized(_) => {
                panic!("Console was downgraded");
            }
        };

        assert_frame_contains(&frame, "SNOOP v2");

        Ok(())
    }

    #[test]
    fn test_session_info() -> buck2_error::Result<()> {
        let info = SessionInfo {
            trace_id: TraceId::null(),
            test_session: Some(buck2_data::TestSessionInfo {
                info: (0..100).map(|_| "a").collect(),
                test_session_id: None,
            }),
            legacy_dice: false,
        };

        let re_state = ReState::new();
        let two_snapshots = TwoSnapshots::default();

        let full = SessionInfoComponent {
            session_info: &info,
            re_state: &re_state,
            two_snapshots: &two_snapshots,
        }
        .draw_unchecked(
            Dimensions {
                // Enough to print everything on one line (we need 109 in fbcode and 110 in OSS)
                width: 110,
                height: 1,
            },
            DrawMode::Normal,
        )?;

        // Buck UI/Build ID + Test UI, each on one line
        assert_eq!(full.len(), 2);

        let multiline = SessionInfoComponent {
            session_info: &info,
            re_state: &re_state,
            two_snapshots: &two_snapshots,
        }
        .draw_unchecked(
            Dimensions {
                // Just long enough to print each on one line.
                width: 100,
                height: 1,
            },
            DrawMode::Normal,
        )?;

        // Width too narrow for all on one line, so each splits into header + value:
        // Buck UI header, Buck UI value, Test UI header, Test UI value
        assert_eq!(multiline.len(), 4);

        let too_small = SessionInfoComponent {
            session_info: &info,
            re_state: &re_state,
            two_snapshots: &two_snapshots,
        }
        .draw_unchecked(
            Dimensions {
                width: 1,
                height: 1,
            },
            DrawMode::Normal,
        )?;

        assert_eq!(too_small.len(), 1);

        Ok(())
    }

    #[test]
    fn test_session_info_network() -> buck2_error::Result<()> {
        let info = SessionInfo {
            trace_id: TraceId::null(),
            test_session: None,
            legacy_dice: false,
        };

        let mut re_state = ReState::new();
        re_state.add_re_session(&buck2_data::RemoteExecutionSessionCreated {
            session_id: "reSessionID-123".to_owned(),
            experiment_name: "".to_owned(),
            persistent_cache_mode: None,
        });
        re_state.update(&buck2_data::Snapshot::default());

        let mut two_snapshots = TwoSnapshots::default();
        let start = std::time::SystemTime::UNIX_EPOCH;
        two_snapshots.update(start, &buck2_data::Snapshot::default());
        two_snapshots.update(
            start + std::time::Duration::from_secs(10),
            &buck2_data::Snapshot {
                re_upload_bytes: 10 * 1024 * 1024,
                re_download_bytes: 1024 * 1024 * 1024,
                http_download_bytes: 512 * 1024 * 1024,
                ..Default::default()
            },
        );

        let component = SessionInfoComponent {
            session_info: &info,
            re_state: &re_state,
            two_snapshots: &two_snapshots,
        };

        let normal = component
            .draw_unchecked(
                Dimensions {
                    width: 100,
                    height: 10,
                },
                DrawMode::Normal,
            )?
            .fmt_for_test()
            .to_string();
        let expected_network = if cfg!(fbcode_build) {
            "Network: up    10MiB 1.0MiB/s\n         down 1.5GiB 154MiB/s"
        } else {
            "Network:  up    10MiB 1.0MiB/s\n          down 1.5GiB 154MiB/s"
        };
        assert!(
            normal.contains(expected_network),
            "unexpected render:\n{normal}"
        );

        let final_render = component
            .draw_unchecked(
                Dimensions {
                    width: 100,
                    height: 10,
                },
                DrawMode::Final,
            )?
            .fmt_for_test()
            .to_string();
        let expected_network = if cfg!(fbcode_build) {
            "Network: up 10MiB  down 1.5GiB"
        } else {
            "Network:  up 10MiB  down 1.5GiB"
        };
        assert!(
            final_render.contains(expected_network),
            "unexpected render:\n{final_render}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_tailer_stderr() -> buck2_error::Result<()> {
        let trace_id = TraceId::new();
        let tick = Tick::now();

        let mut console = StatefulSuperConsole::new(
            "build",
            trace_id.dupe(),
            ConsoleCanvas::Terminal(test_console()),
            Verbosity::default(),
            true,
            Timekeeper::new(
                Box::new(RealtimeClock),
                EventTimestamp(SystemTime::now().into()),
            ),
            Default::default(),
            None,
        )?;

        console.handle_tailer_stderr("some stderr output").await?;
        console.tick(&tick).await?;

        let frame = match &mut console {
            StatefulSuperConsole::Running(c) => c
                .canvas
                .testing_terminal()
                .test_output_mut()
                .frames
                .pop()
                .internal_error("No frame was emitted")?,
            StatefulSuperConsole::Finalized(_) => {
                panic!("Console was downgraded");
            }
        };

        assert_frame_contains(&frame, "some stderr output");
        Ok(())
    }
}
