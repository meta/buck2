/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

// The code in this module is adapted from [`complete_pty`](https://crates.io/crates/completest_pty).

//! Run completions for your program
//!
//! # Example
//!
//! ```rust,no_run
//! # #[cfg(unix)] {
//! # use std::path::Path;
//! # let bin_root = Path::new("").to_owned();
//! # let completion_script = "";
//! # let home = std::env::current_dir().unwrap();
//! let term = completest_pty::Term::new();
//!
//! let mut runtime = completest_pty::BashRuntime::new(bin_root, home).unwrap();
//! runtime.register("foo", completion_script).unwrap();
//! let output = runtime.complete("foo \t\t", &term).unwrap();
//! # }
//! ```

use std::ffi::OsStr;
use std::io::Read as _;
use std::io::Write as _;
use std::num::NonZeroU16;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use ptyprocess::PtyProcess;

use crate::Shell;

/// Zsh runtime
#[derive(Debug)]
pub(crate) struct ZshRuntime {
    home: PathBuf,
}

impl ZshRuntime {
    /// Reuse an existing runtime's home
    pub(crate) fn with_home(home: PathBuf) -> std::io::Result<Self> {
        Ok(Self { home })
    }

    /// Register a completion script
    pub(crate) fn register(&mut self, name: &str, content: &str) -> std::io::Result<()> {
        let path = self.home.join(format!("zsh/_{name}"));
        std::fs::create_dir_all(path.parent().expect("path created with parent"))?;
        std::fs::write(path, content)
    }

    /// Get the output from typing `input` into the shell
    pub(crate) fn complete(&self, input: &str) -> std::io::Result<String> {
        let mut command = Shell::Zsh.find()?;
        command.arg("--noglobalrcs");
        command.env("TERM", "xterm").env("ZDOTDIR", &self.home);
        let echo = false;
        comptest(command, echo, input, &self.home)
    }
}

/// Bash runtime
#[derive(Debug)]
pub(crate) struct BashRuntime {
    home: PathBuf,
    config: PathBuf,
}

impl BashRuntime {
    /// Reuse an existing runtime's home
    pub(crate) fn with_home(home: PathBuf) -> std::io::Result<Self> {
        let config_path = home.join(".bashrc");

        Ok(Self {
            home,
            config: config_path,
        })
    }

    /// Register a completion script
    pub(crate) fn register(&mut self, _name: &str, content: &str) -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.config)?;
        writeln!(&mut file, "{content}")?;
        Ok(())
    }

    /// Get the output from typing `input` into the shell
    pub(crate) fn complete(&self, input: &str) -> std::io::Result<String> {
        let mut command = Shell::Bash.find()?;
        let inputrc_path = self.home.join(".inputrc");
        command
            .env("TERM", "xterm")
            .env("INPUTRC", &inputrc_path)
            .env("BASH_SILENCE_DEPRECATION_WARNING", "1")
            .args([
                OsStr::new("--noprofile"),
                OsStr::new("--rcfile"),
                self.config.as_os_str(),
            ]);
        let echo = !input.contains("\t\t");
        comptest(command, echo, input, &self.home)
    }
}

/// Fish runtime
#[derive(Debug)]
pub(crate) struct FishRuntime {
    home: PathBuf,
    completion_script: Option<PathBuf>,
}

impl FishRuntime {
    /// Initialize a new runtime's home
    pub(crate) fn new(home: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&home)?;
        Ok(Self {
            home,
            completion_script: None,
        })
    }

    /// Register a completion script
    pub(crate) fn register(&mut self, name: &str, content: &str) -> std::io::Result<()> {
        let path = self.home.join(format!("{name}.fish"));
        std::fs::write(&path, content)?;
        self.completion_script = Some(path);
        Ok(())
    }

    /// Ask Fish to compute completions for `input` without rendering an interactive prompt.
    pub(crate) fn complete(&self, input: &str) -> std::io::Result<String> {
        let completion = self
            .completion_script
            .as_ref()
            .ok_or_else(|| std::io::Error::other("no Fish completion has been registered"))?;
        let mut command = Shell::Fish.find()?;
        command
            .env("TERM", "xterm")
            .env("XDG_CONFIG_HOME", &self.home)
            .args([
                "-c",
                "source \"$argv[1]\"; complete --do-complete=\"$argv[2]\"",
                "--",
            ]);
        command.arg(completion).arg(input);

        let output = command.output()?;
        if !output.status.success() {
            return Err(std::io::Error::other(format!(
                "fish completion failed with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr),
            )));
        }

        String::from_utf8(output.stdout)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }
}

const TERM_WIDTH: u16 = 120;
const TERM_HEIGHT: u16 = 60;

fn comptest(
    mut command: Command,
    echo: bool,
    input: &str,
    lockfile_dir: &Path,
) -> std::io::Result<String> {
    #![allow(clippy::unwrap_used)] // some unwraps need extra investigation

    let lockfile = lockfile_dir.join("completion_verify_lockfile");

    command.env("COMPLETION_VERIFY_LOCKFILE", &lockfile);

    // spawn a new process, pass it the input was.
    //
    // This triggers completion loading process which takes some time in shell so we should let it
    // run for some time
    let mut process = PtyProcess::spawn(command)?;
    process.set_window_size(TERM_WIDTH, TERM_HEIGHT)?;
    // for some reason bash does not produce anything with echo disabled...
    process.set_echo(echo, None)?;

    let mut parser = vt100::Parser::new(
        NonZeroU16::new(TERM_HEIGHT).unwrap(),
        NonZeroU16::new(TERM_WIDTH).unwrap(),
        0,
    );

    let mut stream = process.get_raw_handle()?;
    // pass the completion input
    write!(stream, "{input}")?;
    stream.flush()?;

    let (snd, rcv) = std::sync::mpsc::channel();

    let shutdown = std::sync::atomic::AtomicBool::new(false);
    let shutdown_ref = &shutdown;
    std::thread::scope(|scope| {
        scope.spawn(move || {
            // The lockfile can be created by a completions impl to indicate that it hasn't finished
            // yet
            let check_lockfile = || lockfile.exists();

            // First wait for anything to be produced. This is usually the prompt
            rcv.recv().unwrap();
            // Then, wait for a potentially extended amount of time for the next data to be
            // produced. This will only not happen if there are no completions to output
            if rcv.recv_timeout(Duration::from_millis(5000)).is_ok() || check_lockfile() {
                // Finally, wait for shorter intervals until new output stops being produced
                while rcv.recv_timeout(Duration::from_millis(1000)).is_ok() || check_lockfile() {}
            }

            shutdown_ref.store(true, std::sync::atomic::Ordering::SeqCst);
            process.exit(false).unwrap();
        });

        let mut buf = [0; 2048];
        let mut seen_prompt = false;
        while let Ok(n) = stream.read(&mut buf) {
            if shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                // fish clears completions on process teardown
                break;
            }
            let buf = &buf[..n];
            if buf.is_empty() {
                break;
            }
            parser.process(buf);

            // We know that we will see at least one prompt, so we never need to consider exiting
            // before that comes through
            match seen_prompt {
                false => {
                    if buf.contains(&b'%') {
                        seen_prompt = true;
                        _ = snd.send(());
                    }
                }
                true => {
                    _ = snd.send(());
                }
            }
        }
    });

    let content = parser.screen().contents();
    Ok(content)
}
