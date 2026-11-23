/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Event forwarding: the daemon gives a helper program the events of each command that carries
//! event forwarding keys, in the format of Buck2's own event logs.
//!
//! A command carries the event forwarding keys that the client gets from the
//! `BUCK2_EVENT_FORWARDING_KEY` environment variable, separated by commas, and from its
//! `--event-forwarding-key` flags, one key each. When the daemon's Buck settings turn event
//! forwarding on (`event_forwarding.enabled`) and name a helper program (`event_forwarding.helper`,
//! resolved against the project root when relative), the daemon starts the helper once for each
//! keyed command, as the command starts: with no arguments, in the project root, with the daemon's
//! environment plus `BUCK2_EVENT_FORWARDING_KEY` set to the command's keys, separated by commas,
//! its standard input a pipe, and its standard output and error discarded. Only the repository's
//! `.bucksettings.toml` and rollouts set the two settings: local settings files and `--setting`
//! cannot change them.
//!
//! Keys are opaque to Buck2. Several tools can each give a command a key, and a helper picks out
//! the keys it knows, for example by a prefix of its own.
//!
//! The helper reads a zstd-compressed protobuf event log on its standard input, in the format of
//! the `.pb.zst` event logs that `buck2 log` commands read: one zstd frame holding these
//! length-delimited messages:
//!
//! 1. A `buck.data.Invocation`: the command line, sanitized, the client's working directory, the
//!    trace ID and the start time. `expanded_command_line_args` is empty, as the daemon does not
//!    see how the client expands `@` arguments.
//! 2. Each of the command's events, as the client gets them: a `buck.daemon.CommandProgress`
//!    holding a `buck.data.BuckEvent`.
//! 3. The command's result, as the client gets it: a `buck.daemon.CommandProgress` holding a
//!    `buck.daemon.CommandResult`. A command whose client went away ends without one.
//!
//! Then the frame ends and the input closes. The daemon flushes the stream after each batch of
//! messages it writes, so a helper can decode each message as soon as it arrives. The messages
//! change only in the ways protobuf keeps compatible, so a helper keeps reading the logs of newer
//! Buck2 versions. Saved to a file whose name ends in `.pb.zst`, the input is a log that
//! `buck2 log` commands read.
//!
//! The daemon forwards its streaming commands: builds, tests, queries, `bxl`, `install` and the
//! like. Commands the client runs alone, such as `help-env` and `log`, and requests the daemon
//! answers directly, such as `status` and `kill`, get no helper. `buck2 run` forwards its build,
//! not the program it runs afterwards.
//!
//! A helper must exit once its input closes; the daemon kills one that still runs 300 seconds
//! later. On Unix the helper runs in a process group of its own, and once the helper has exited
//! or been killed, the daemon also kills every process still in that group. A daemon that exits
//! leaves its helpers to exit as their input closes.
//!
//! Starting the helper and writing to it never holds up a command: a helper that does not start,
//! reads slowly, or exits early only misses messages. The daemon gives up on writing the result
//! 60 seconds after the command ended, and then the input closes without the frame's end.
//!
//! A helper that falls more than 16 MiB of events behind misses the command's later events, but
//! still gets its result.

mod encode;
mod helper;
mod writer;

use std::sync::LazyLock;

use buck2_server::command_observer::COMMAND_LIFECYCLE_OBSERVER;

use crate::helper::ProcessLauncher;
use crate::writer::EventForwarder;

static FORWARDER: LazyLock<EventForwarder> =
    LazyLock::new(|| EventForwarder::new(Box::new(ProcessLauncher)));

pub fn init_late_bindings() {
    COMMAND_LIFECYCLE_OBSERVER.init(Some(&*FORWARDER));
}
