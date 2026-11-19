/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::convert::Infallible;
use std::ops::ControlFlow;
use std::ops::FromResidual;
use std::ops::Residual;
use std::ops::Try;

use crate::exit_result::ExitResult;

/// The final outcome returned to the client of running a command in the daemon.
///
/// Either "successful", in which case `R`, the response type, is available, or "failure",
/// where a general `CommandError` was returned. Consider this a "failed successfully" indicator.
/// At the point where this is returned, all event processing / logging should be handled.
#[must_use]
pub enum CommandOutcome<R> {
    /// The buckd client successfully returned the expected response.
    ///
    /// Additional processing of this response may be necessary to determine overall success or
    /// failure within the client.
    Success(R),
    /// The buckd client successfully returned a response, but that response was a general failure.
    ///
    /// The user has already been presented an error message, and the CLI should exit with
    /// this status code.
    Failure(ExitResult),
}

/// Compatibility alias for code that names the residual type directly.
pub type CommandFailure = CommandOutcome<Infallible>;

/// Allow the usage of '?' when going from a CommandOutcome -> ExitResult
impl<R> Try for CommandOutcome<R> {
    type Output = R;
    type Residual = CommandFailure;

    fn from_output(output: Self::Output) -> Self {
        Self::Success(output)
    }

    fn branch(self) -> ControlFlow<Self::Residual, Self::Output> {
        match self {
            CommandOutcome::Success(res) => ControlFlow::Continue(res),
            CommandOutcome::Failure(status) => ControlFlow::Break(CommandOutcome::Failure(status)),
        }
    }
}

impl<R> Residual<R> for CommandFailure {
    type TryType = CommandOutcome<R>;
}

impl FromResidual<CommandFailure> for ExitResult {
    fn from_residual(residual: CommandFailure) -> Self {
        match residual {
            CommandOutcome::Failure(status) => status,
        }
    }
}

impl<T> FromResidual<CommandFailure> for Result<T, ExitResult> {
    fn from_residual(residual: CommandFailure) -> Self {
        match residual {
            CommandOutcome::Failure(status) => Err(status),
        }
    }
}

impl<R> FromResidual<CommandFailure> for CommandOutcome<R> {
    fn from_residual(residual: CommandFailure) -> Self {
        match residual {
            CommandOutcome::Failure(status) => Self::Failure(status),
        }
    }
}

impl<R, E> FromResidual<Result<Infallible, E>> for CommandOutcome<R>
where
    E: Into<buck2_error::Error>,
{
    fn from_residual(result: Result<Infallible, E>) -> Self {
        match result {
            Err(err) => Self::Failure(ExitResult::err(err.into())),
        }
    }
}
