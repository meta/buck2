/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Promise is a handle to a DiceTask that will be completed

use std::future::Future;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

use dice_error::cycles::CycleError;
use pin_project::pin_project;

use crate::epoch::task::dice::DiceTaskDependentFuture;
use crate::value::MaybeResidentComputedValue;

/// A strong reference to a 'DiceTask' that is pollable as a future.
/// This is only awoken when the result is ready, as none of the pollers are responsible for
/// running the task to completion.
///
/// Fails if the task is found to be in a dependency cycle with the key waiting on it; that is the
/// only way it fails.
#[pin_project]
pub(crate) struct DicePromise<'d, T = MaybeResidentComputedValue>(
    #[pin] pub(super) DicePromiseInternal<'d, T>,
);

#[pin_project(project = DicePromiseInternalProj)]
pub(super) enum DicePromiseInternal<'d, T = MaybeResidentComputedValue> {
    Ready {
        result: &'d T,
    },
    Pending {
        #[pin]
        future: DiceTaskDependentFuture<'d, T>,
    },
    Done,
}

impl<'d, T> DicePromise<'d, T> {
    pub(crate) fn ready(result: &'d T) -> Self {
        Self(DicePromiseInternal::Ready { result })
    }

    pub(crate) fn pending(future: DiceTaskDependentFuture<'d, T>) -> Self {
        Self(DicePromiseInternal::Pending { future })
    }
}

impl<'d, T> Future for DicePromise<'d, T> {
    type Output = Result<&'d T, CycleError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let res = match self.as_mut().project().0.project() {
            DicePromiseInternalProj::Ready { result } => Poll::Ready(Ok(*result)),
            DicePromiseInternalProj::Pending { future } => future.poll(cx),
            DicePromiseInternalProj::Done => panic!("poll after ready"),
        };

        if matches!(res, Poll::Ready(..)) {
            self.project().0.set(DicePromiseInternal::Done);
        }

        res
    }
}
