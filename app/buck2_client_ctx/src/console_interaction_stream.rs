/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::soft_error_or_panic;
use crossterm::event::Event;
use crossterm::event::EventStream;
use crossterm::event::KeyCode;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use futures::StreamExt;
use strum::EnumIter;
use superconsole::Stdin;

pub struct ConsoleInteractionStream<'a> {
    _stdin: &'a mut Stdin,
    term: InteractiveTerminal,
    events: EventStream,
}

impl<'a> ConsoleInteractionStream<'a> {
    pub fn new(stdin: &'a mut Stdin) -> Option<Self> {
        let term = match InteractiveTerminal::enable() {
            Ok(Some(term)) => term,
            Ok(None) => {
                tracing::debug!("Not enabling interactive terminal");
                return None;
            }
            Err(e) => {
                soft_error_or_panic!("interactive_terminal_enable_failed", e);
                return None;
            }
        };

        Some(Self {
            _stdin: stdin,
            term,
            events: EventStream::new(),
        })
    }
}

impl Drop for ConsoleInteractionStream<'_> {
    fn drop(&mut self) {
        if let Err(e) = self.term.disable() {
            soft_error_or_panic!("interactive_terminal_disable_failed", e);
        }
    }
}

#[cfg(unix)]
mod interactive_terminal {
    use std::io::IsTerminal;
    use std::os::unix::io::AsRawFd;

    use buck2_error::BuckErrorContext;
    use termios::*;

    pub struct InteractiveTerminal {
        orig: Termios,
    }

    impl InteractiveTerminal {
        pub fn enable() -> buck2_error::Result<Option<Self>> {
            let fd = std::io::stdin().as_raw_fd();

            if !std::io::stdin().is_terminal() {
                return Ok(None);
            }

            // If stdout is redirected, let's also no turn this on since it might be redirected to
            // something that wants stdout. We can always debug those with `debug replay` now.
            if !std::io::stdout().is_terminal() {
                return Ok(None);
            }

            // We also check for stderr, since if a user is starting a bunch of bucks in the
            // background those may end up clobbering the termios state and the following can
            // happen:
            //
            // - Process starts 1 buck, which sets noecho.
            // - Process starts another buck, which reads the tty state, reads noecho.
            // - The first buck exits and resets echo.
            // - The second buck exits and resets noecho (since that's what it read)
            //
            // We check stderr because if stderr is a TTY the user will see a bunch of consoles
            // interleaving and that would probably tell them something's wrong.
            if !std::io::stderr().is_terminal() {
                return Ok(None);
            }

            let orig =
                Termios::from_fd(fd).buck_error_context("Failed to access current termios")?;

            let mut termios = orig;

            // Switch to non-canonical mode to get input immediately, and disable echo.
            termios.c_lflag &= !(ICANON | ECHO);

            // Keep blocking reads.
            termios.c_cc[VMIN] = 1;
            termios.c_cc[VTIME] = 0;

            tcsetattr(fd, TCSANOW, &termios).buck_error_context("Failed to set termios")?;

            Ok(Some(Self { orig }))
        }

        pub fn disable(&mut self) -> buck2_error::Result<()> {
            let fd = std::io::stdin().as_raw_fd();
            tcsetattr(fd, TCSANOW, &self.orig).buck_error_context("Failed to reset termios")?;
            Ok(())
        }
    }
}

#[cfg(windows)]
mod interactive_terminal {
    use std::io::IsTerminal;
    use std::os::windows::io::AsRawHandle;

    use buck2_error::BuckErrorContext;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Console::ENABLE_ECHO_INPUT;
    use windows_sys::Win32::System::Console::ENABLE_LINE_INPUT;
    use windows_sys::Win32::System::Console::GetConsoleMode;
    use windows_sys::Win32::System::Console::SetConsoleMode;

    fn get_console_mode(handle: HANDLE) -> buck2_error::Result<u32> {
        let mut mode: u32 = 0;
        if unsafe { GetConsoleMode(handle, &mut mode) } != 0 {
            Ok(mode)
        } else {
            Err(std::io::Error::last_os_error()).buck_error_context("Failed to get console mode")
        }
    }

    fn set_console_mode(handle: HANDLE, mode: u32) -> buck2_error::Result<()> {
        if unsafe { SetConsoleMode(handle, mode) != 0 } {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error()).buck_error_context("Failed to set console mode")
        }
    }

    pub struct InteractiveTerminal {
        mode: u32,
    }

    impl InteractiveTerminal {
        pub fn enable() -> buck2_error::Result<Option<Self>> {
            let handle = std::io::stdin().as_raw_handle() as HANDLE;

            if !std::io::stdin().is_terminal()
                || !std::io::stdout().is_terminal()
                || !std::io::stderr().is_terminal()
            {
                return Ok(None);
            }

            let mode = get_console_mode(handle)?;
            // Switch to non-canonical mode to get input immediately, and disable echo.
            set_console_mode(handle, mode & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT))?;
            Ok(Some(Self { mode }))
        }

        pub fn disable(&mut self) -> buck2_error::Result<()> {
            let handle = std::io::stdin().as_raw_handle() as HANDLE;
            set_console_mode(handle, self.mode)?;
            Ok(())
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod interactive_terminal {
    pub struct InteractiveTerminal;

    impl InteractiveTerminal {
        pub fn enable() -> buck2_error::Result<Option<Self>> {
            Ok(None)
        }

        pub fn disable(&mut self) -> buck2_error::Result<()> {
            Ok(())
        }
    }
}

use interactive_terminal::InteractiveTerminal;

#[derive(Debug, EnumIter)]
pub enum SuperConsoleToggle {
    Dice,
    DebugEvents,
    TwoLinesMode,
    DetailedRE,
    Io,
    TargetConfigurations,
    ExpandedProgress,
    Commands,
    IncrLines,
    DecrLines,
    IncreaseReplaySpeed,
    DecreaseReplaySpeed,
    PauseReplay,
    Help,
    /// Raw character that didn't match any known toggle.
    /// Not included in help or iteration.
    #[strum(disabled)]
    Char(char),
}

pub enum ConsoleInteraction {
    Toggle(SuperConsoleToggle),
    Key(ConsoleKey),
    Resize,
}

#[derive(Debug, Clone, Copy)]
pub enum ConsoleKey {
    Escape,
    Up,
    Down,
    Left,
    Right,
    ShiftLeft,
    ShiftRight,
    Other,
}

impl SuperConsoleToggle {
    pub fn description(&self) -> &str {
        match self {
            SuperConsoleToggle::Dice => "DICE",
            SuperConsoleToggle::DebugEvents => "debug events",
            SuperConsoleToggle::TwoLinesMode => "two lines mode",
            SuperConsoleToggle::DetailedRE => "network details",
            SuperConsoleToggle::Io => "I/O counters",
            SuperConsoleToggle::TargetConfigurations => "target configurations",
            SuperConsoleToggle::ExpandedProgress => "expanded progress",
            SuperConsoleToggle::Commands => "commands",
            SuperConsoleToggle::IncrLines => "more lines",
            SuperConsoleToggle::DecrLines => "less lines",
            SuperConsoleToggle::IncreaseReplaySpeed => "increase replay speed",
            SuperConsoleToggle::DecreaseReplaySpeed => "decrease replay speed",
            SuperConsoleToggle::PauseReplay => "pause replay",
            SuperConsoleToggle::Help => "help",
            SuperConsoleToggle::Char(_) => "char",
        }
    }

    pub fn key(&self) -> char {
        match self {
            SuperConsoleToggle::Dice => 'd',
            SuperConsoleToggle::DebugEvents => 'e',
            SuperConsoleToggle::TwoLinesMode => '2',
            SuperConsoleToggle::DetailedRE => 'r',
            SuperConsoleToggle::Io => 'i',
            SuperConsoleToggle::TargetConfigurations => 'p',
            SuperConsoleToggle::ExpandedProgress => 'x',
            SuperConsoleToggle::Commands => 'c',
            SuperConsoleToggle::IncrLines => '+',
            SuperConsoleToggle::DecrLines => '-',
            SuperConsoleToggle::IncreaseReplaySpeed => 'k',
            SuperConsoleToggle::DecreaseReplaySpeed => 'j',
            SuperConsoleToggle::PauseReplay => 'y',
            SuperConsoleToggle::Help => '?',
            SuperConsoleToggle::Char(c) => *c,
        }
    }
}

#[async_trait::async_trait]
pub trait SuperConsoleInteraction: Send + Sync {
    async fn interaction(&mut self) -> buck2_error::Result<ConsoleInteraction>;
}

#[async_trait::async_trait]
impl SuperConsoleInteraction for ConsoleInteractionStream<'_> {
    async fn interaction(&mut self) -> buck2_error::Result<ConsoleInteraction> {
        loop {
            let Some(event) = self.events.next().await else {
                futures::future::pending().await
            };
            let event = event
                .map_err(|e| buck2_error::Error::from(e).context("Error reading console event"))?;
            match event {
                Event::Key(key) => {
                    // Windows emits a Release event (and, with the keyboard
                    // enhancement protocol, a Repeat) for every physical
                    // keypress; act only on Press so one keystroke fires once.
                    match key.kind {
                        KeyEventKind::Press => {}
                        KeyEventKind::Repeat | KeyEventKind::Release => continue,
                    }
                    let interaction = match key.code {
                        KeyCode::Char(c) => ConsoleInteraction::Toggle(match c {
                            'd' => SuperConsoleToggle::Dice,
                            'e' => SuperConsoleToggle::DebugEvents,
                            '2' => SuperConsoleToggle::TwoLinesMode,
                            'r' => SuperConsoleToggle::DetailedRE,
                            'i' => SuperConsoleToggle::Io,
                            'p' => SuperConsoleToggle::TargetConfigurations,
                            'x' => SuperConsoleToggle::ExpandedProgress,
                            'c' => SuperConsoleToggle::Commands,
                            '+' => SuperConsoleToggle::IncrLines,
                            '-' => SuperConsoleToggle::DecrLines,
                            'k' => SuperConsoleToggle::IncreaseReplaySpeed,
                            'j' => SuperConsoleToggle::DecreaseReplaySpeed,
                            'y' => SuperConsoleToggle::PauseReplay,
                            '?' | 'h' => SuperConsoleToggle::Help,
                            c => SuperConsoleToggle::Char(c),
                        }),
                        KeyCode::Enter => {
                            ConsoleInteraction::Toggle(SuperConsoleToggle::Char('\n'))
                        }
                        KeyCode::Tab => ConsoleInteraction::Toggle(SuperConsoleToggle::Char('\t')),
                        KeyCode::Esc => ConsoleInteraction::Key(ConsoleKey::Escape),
                        KeyCode::Up => ConsoleInteraction::Key(ConsoleKey::Up),
                        KeyCode::Down => ConsoleInteraction::Key(ConsoleKey::Down),
                        KeyCode::Left => ConsoleInteraction::Key(
                            if key.modifiers.contains(KeyModifiers::SHIFT) {
                                ConsoleKey::ShiftLeft
                            } else {
                                ConsoleKey::Left
                            },
                        ),
                        KeyCode::Right => ConsoleInteraction::Key(
                            if key.modifiers.contains(KeyModifiers::SHIFT) {
                                ConsoleKey::ShiftRight
                            } else {
                                ConsoleKey::Right
                            },
                        ),
                        KeyCode::Backspace
                        | KeyCode::Home
                        | KeyCode::End
                        | KeyCode::PageUp
                        | KeyCode::PageDown
                        | KeyCode::BackTab
                        | KeyCode::Delete
                        | KeyCode::Insert
                        | KeyCode::F(_)
                        | KeyCode::Null
                        | KeyCode::CapsLock
                        | KeyCode::ScrollLock
                        | KeyCode::NumLock
                        | KeyCode::PrintScreen
                        | KeyCode::Pause
                        | KeyCode::Menu
                        | KeyCode::KeypadBegin
                        | KeyCode::Media(_)
                        | KeyCode::Modifier(_) => ConsoleInteraction::Key(ConsoleKey::Other),
                    };
                    return Ok(interaction);
                }
                Event::Resize(..) => return Ok(ConsoleInteraction::Resize),
                Event::FocusGained | Event::FocusLost | Event::Mouse(_) | Event::Paste(_) => {}
            }
        }
    }
}

pub struct NoopSuperConsoleInteraction;

#[async_trait::async_trait]
impl SuperConsoleInteraction for NoopSuperConsoleInteraction {
    async fn interaction(&mut self) -> buck2_error::Result<ConsoleInteraction> {
        futures::future::pending().await
    }
}
