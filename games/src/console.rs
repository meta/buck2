/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use futures::Future;
use futures::Stream;
use futures::future;
use futures::future::Either;
use itertools::Itertools;
use superconsole::Lines;
pub use superconsole::Stdin;
use superconsole::SuperConsole;
use tokio::io::AsyncReadExt;

#[cfg(unix)]
mod raw_mode {
    use std::io::IsTerminal;
    use std::os::unix::io::AsRawFd;

    use termios::*;

    /// RAII guard that enables terminal raw mode on creation and restores it on drop.
    /// Raw mode disables line buffering and echo so individual keystrokes (including
    /// escape sequences for arrow keys) are delivered immediately.
    pub struct RawMode {
        orig: Termios,
    }

    impl RawMode {
        pub fn enable() -> anyhow::Result<Self> {
            let fd = std::io::stdin().as_raw_fd();

            if !std::io::stdin().is_terminal() {
                return Err(anyhow::anyhow!("stdin is not a terminal"));
            }

            let orig = Termios::from_fd(fd)?;

            let mut termios = orig;
            termios.c_lflag &= !(ICANON | ECHO);
            termios.c_cc[VMIN] = 1;
            termios.c_cc[VTIME] = 0;
            tcsetattr(fd, TCSANOW, &termios)?;

            Ok(RawMode { orig })
        }
    }

    impl Drop for RawMode {
        fn drop(&mut self) {
            let fd = std::io::stdin().as_raw_fd();
            drop(tcsetattr(fd, TCSANOW, &self.orig));
        }
    }
}

#[cfg(unix)]
pub use raw_mode::RawMode;

#[cfg(not(unix))]
pub struct RawMode;

#[cfg(not(unix))]
impl RawMode {
    pub fn enable() -> anyhow::Result<Self> {
        Ok(RawMode)
    }
}

/// Races `work` against ctrl+c. When ctrl+c is received, the `work` future is
/// dropped, which ensures RAII guards (like `RawMode`) run their destructors.
pub async fn with_sigint_handler<F: Future>(work: F) -> Option<F::Output> {
    let exit = tokio::signal::ctrl_c();

    futures::pin_mut!(work);
    futures::pin_mut!(exit);

    match future::select(work, exit).await {
        Either::Left((res, _)) => Some(res),
        Either::Right((_, _)) => None,
    }
}

/// A parsed terminal input event (character, escape, or arrow key).
pub enum Control {
    Char(char),
    Escape,
    UnknownEscape(Vec<char>),
    Up,
    Down,
    Left,
    Right,
    ShiftLeft,
    ShiftRight,
}

#[async_trait::async_trait]
pub trait ConsoleInteraction: Send + Sync {
    async fn char(&mut self) -> anyhow::Result<char>;
}

#[async_trait::async_trait]
impl ConsoleInteraction for Stdin {
    async fn char(&mut self) -> anyhow::Result<char> {
        match self.read_u8().await {
            Ok(c) => Ok(c.into()),
            // NOTE: An EOF here would be reported as "unexpected" because we asked for a u8.
            Err(e)
                if e.kind() == std::io::ErrorKind::UnexpectedEof
                    || e.kind() == std::io::ErrorKind::WouldBlock =>
            {
                futures::future::pending().await
            }
            Err(e) => Err(anyhow::Error::from(e).context("Error reading char from console")),
        }
    }
}

pub struct NoopConsoleInteraction;

#[async_trait::async_trait]
impl ConsoleInteraction for NoopConsoleInteraction {
    async fn char(&mut self) -> anyhow::Result<char> {
        futures::future::pending().await
    }
}

pub fn control_reader<'a>(
    console: &'a mut dyn ConsoleInteraction,
) -> impl Stream<Item = anyhow::Result<Control>> + 'a {
    async_stream::try_stream! {
        loop {
            let c = console.char().await?;

            // Legacy cmd.exe sends 0xe0 or 0x00 prefix for arrow/function keys.
            if c as u32 == 0xe0 || c as u32 == 0x00 {
                let scan = console.char().await?;
                match scan {
                    'H' => yield Control::Up,
                    'P' => yield Control::Down,
                    'K' => yield Control::Left,
                    'M' => yield Control::Right,
                    _ => {}
                }
                continue;
            }

            if (c as u32) != 0x1b {
                yield Control::Char(c);
                continue;
            }

            // After seeing 0x1b, wait briefly for a follow-up byte. If none
            // arrives, this is a bare Escape keypress rather than the start of
            // an escape sequence (like arrow keys which send 0x1b 0x5b <letter>).
            let next = tokio::time::timeout(
                std::time::Duration::from_millis(50),
                console.char(),
            ).await;

            let c = match next {
                Err(_) => {
                    yield Control::Escape;
                    continue;
                }
                Ok(c) => c?,
            };

            if (c as u32) != 0x5b {
                yield Control::UnknownEscape(vec![c]);
                continue;
            }

            let c = console.char().await?;
            match c {
                'A' => yield Control::Up,
                'B' => yield Control::Down,
                'D' => yield Control::Left,
                'C' => yield Control::Right,
                // CSI sequences with parameters: ESC [ 1 ; <mod> <letter>
                // Shift+Arrow sends ESC [ 1 ; 2 <A/B/C/D>
                '1' => {
                    let mut params = vec!['1'];
                    let final_char;
                    loop {
                        let nc = console.char().await?;
                        if nc.is_ascii_alphabetic() {
                            final_char = nc;
                            break;
                        }
                        params.push(nc);
                    }
                    // params = ['1', ';', '2'] for Shift modifier
                    let is_shift = params == ['1', ';', '2'];
                    match (final_char, is_shift) {
                        ('C', true) => yield Control::ShiftRight,
                        ('D', true) => yield Control::ShiftLeft,
                        ('C', false) => yield Control::Right,
                        ('D', false) => yield Control::Left,
                        ('A', false) => yield Control::Up,
                        ('B', false) => yield Control::Down,
                        _ => {
                            let mut esc = vec![char::from_u32(0x5b).unwrap()];
                            esc.extend(&params);
                            esc.push(final_char);
                            yield Control::UnknownEscape(esc);
                        }
                    }
                }
                c => yield Control::UnknownEscape(vec![char::from_u32(0x5b).unwrap(), c]),
            }
        }
    }
}

#[derive(Clone, Copy)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

pub fn control_to_direction(control: &Control) -> Option<Direction> {
    match control {
        Control::Left | Control::Char('a') => Some(Direction::Left),
        Control::Right | Control::Char('d') => Some(Direction::Right),
        Control::Down | Control::Char('s') => Some(Direction::Down),
        Control::Up | Control::Char('w') => Some(Direction::Up),
        _ => None,
    }
}

pub fn emit_unknown_control(renderer: &mut SuperConsole, control: Control) -> anyhow::Result<()> {
    match control {
        Control::Char(x) => {
            renderer.emit(Lines(vec![
                vec![format!("{:x} `{}`", x as u32, x)].try_into()?,
            ]));
        }
        Control::UnknownEscape(x) => {
            renderer.emit(Lines(vec![
                vec![format!(
                    "unknown: [{}]",
                    x.into_iter()
                        .map(|x| format!("{:x} `{}`", x as u32, x))
                        .join(", ")
                )]
                .try_into()?,
            ]));
        }
        _ => {}
    }
    Ok(())
}
