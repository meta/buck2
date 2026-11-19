/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::pin::Pin;
use std::time::Duration;

use buck2_core::soft_error_or_panic;
use futures::future::Either;
use futures::future::Future;
use futures::stream::Stream;
use futures::stream::StreamExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

enum ForwardingState<'a, T> {
    Waiting,
    HavePermit(mpsc::Permit<'a, T>),
    HaveItem { item: T, logged_wait: bool },
}

enum ForwardingStateTransition<'a, T> {
    Next(ForwardingState<'a, T>),
    InputEof,
    RxClosed,
}

impl<'a, T> ForwardingState<'a, T> {
    async fn compute_next_transition<S>(
        self,
        tx: &'a mpsc::Sender<T>,
        mut stream: Pin<&mut S>,
    ) -> ForwardingStateTransition<'a, T>
    where
        S: Stream<Item = T> + ?Sized,
    {
        match self {
            ForwardingState::Waiting => {
                // We do not yet have either an input item or a sender permit, so whichever side
                // becomes ready first becomes the next explicit state. EOF is only observed here
                // when stdin itself ends before we have buffered anything.
                tokio::select! {
                    permit = tx.reserve() => match permit {
                        Ok(permit) => ForwardingStateTransition::Next(ForwardingState::HavePermit(permit)),
                        // Note: it's the work = f(rx) future that caused this to be closed,
                        // we'll allow it to return the actual error/cause if needed.
                        Err(..) => ForwardingStateTransition::RxClosed,
                    },
                    next = stream.next() => match next {
                        Some(item) => ForwardingStateTransition::Next(ForwardingState::HaveItem {
                            item,
                            logged_wait: false,
                        }),
                        None => ForwardingStateTransition::InputEof,
                    }
                }
            }
            ForwardingState::HavePermit(permit) => {
                // Once we have capacity to send a message reserved, the next input item should be
                // forwarded immediately. If the receiver disappears before another item arrives,
                // we drop the reserved permit and let the RPC future determine the overall
                // command result.
                tokio::select! {
                    _ = tx.closed() => ForwardingStateTransition::RxClosed,
                    next = stream.next() => match next {
                        Some(item) => {
                            permit.send(item);
                            ForwardingStateTransition::Next(ForwardingState::Waiting)
                        }
                        None => ForwardingStateTransition::InputEof,
                    }
                }
            }
            ForwardingState::HaveItem { item, logged_wait } => {
                // Once we have already read an item from stdin, keep waiting for either channel
                // capacity or receiver closure. A trailing EOF on stdin does not discard the
                // buffered item, but a closed receiver still does.
                //
                // The wait is done in two phases. The first logs a soft error after 90 seconds
                // and stores that fact in the state, after which we wait forever for the streams
                // to move forward or close.
                if logged_wait {
                    tokio::select! {
                        _ = tx.closed() => ForwardingStateTransition::RxClosed,
                        permit = tx.reserve() => match permit {
                            Ok(permit) => {
                                permit.send(item);
                                ForwardingStateTransition::Next(ForwardingState::Waiting)
                            }
                            Err(..) => ForwardingStateTransition::RxClosed,
                        }
                    }
                } else {
                    tokio::select! {
                        _ = tx.closed() => ForwardingStateTransition::RxClosed,
                        permit = tx.reserve() => match permit {
                            Ok(permit) => {
                                permit.send(item);
                                ForwardingStateTransition::Next(ForwardingState::Waiting)
                            }
                            Err(..) => ForwardingStateTransition::RxClosed,
                        },
                        _ = tokio::time::sleep(Duration::from_secs(90)) => {
                            soft_error_or_panic!(
                                "stream_util_buffered_item_waiting_for_permit",
                                buck2_error::buck2_error!(
                                    buck2_error::ErrorTag::Tier0,
                                    "Buffered stdin item has been waiting 90 seconds for request-stream capacity"
                                ));
                            ForwardingStateTransition::Next(ForwardingState::HaveItem {
                                item,
                                logged_wait: true,
                            })
                        }
                    }
                }
            }
        }
    }
}

/// This function is used for commands where the client wants to stream `stdin`
/// to the server and return back responses. Usually this is for commands that
/// act as API endpoints over stdin/stdout like language servers, etc.
///
/// We need to provide a 'static stream for Tonic to send to the Buck2 daemon,
/// but we don't want to borrow stdin statically (though in practice that
/// doesn't really matter because the way the command ends is when stdin is
/// empty). So, what we do instead is that we forward stdin only while the
/// command is ongoing.
///
/// The `hangup` parameter allows producing a message to the server when stdin
/// ends. The `hangup_timeout` parameter allows returning a fallback result
/// after stdin ends and the optional hangup message has been sent. If it
/// returns `None`, this helper stays pending after EOF and relies on the work
/// future to finish independently.
///
/// Note that the tx/rx channel pair is used by us to send messages to the
/// remote/RPC service implemented by `f`. If `f` causes that channel to close,
/// the state machine will transition to `RxClosed`, and wait forever. It's
/// imperative that the caller's `f` function complete in these cases and return
/// the overall status of the command. We do not propagate errors from the closure
/// of this internal channel object.
pub async fn reborrow_stream_for_static<'a, T, R, F>(
    stream: impl Stream<Item = T> + 'a,
    f: impl FnOnce(ReceiverStream<T>) -> F,
    hangup: impl FnOnce() -> Option<T>,
    hangup_timeout: impl FnOnce() -> Option<(Duration, R)>,
) -> R
where
    F: Future<Output = R> + 'a,
    T: 'static,
{
    let (tx, rx) = mpsc::channel(1);

    // Forwarding is a two-sided state machine: we may observe either the next
    // input item from `stream` or the next sender permit from the `tx` portion
    // of the above channel first, so we stash whichever one arrives and then
    // wait for the other. That lets us distinguish a real input EOF (which
    // should run the hangup policy) from the output stream's receiver
    // disappearing early (which should not be treated as EOF).
    //
    // Once we receive one or the other, we still wait on additional events from
    // that side to ensure we detect when those close, too.
    let forward = async move {
        futures::pin_mut!(stream);
        let mut state = ForwardingState::Waiting;

        loop {
            let transition = std::mem::replace(&mut state, ForwardingState::Waiting)
                .compute_next_transition(&tx, stream.as_mut())
                .await;

            match transition {
                ForwardingStateTransition::Next(next_state) => state = next_state,
                ForwardingStateTransition::InputEof => {
                    break;
                }
                // The receiver stream `rx` we passed into `work = f(rx)` has
                // been closed somewhere in that future's execution. Since we
                // can no longer `tx.send()`, we wait forever, relying on the
                // `work = f(rx)` future (which must have closed the `rx` it
                // owns) to resolve and return a result.
                ForwardingStateTransition::RxClosed => futures::future::pending().await,
            };
        }

        if let Some(hangup) = hangup() {
            let _ignored = tx.send(hangup).await;
        }

        let out = if let Some((timeout, out)) = hangup_timeout() {
            tokio::time::sleep(timeout).await;
            out
        } else {
            // Some server-side streaming commands do not handle hangups. Until they do, we must
            // keep this side alive and rely on the RPC finishing independently.
            futures::future::pending().await
        };

        // We only get here when hangup_timeout() provides a fallback result. Without one, this
        // branch stays pending forever and relies on the work future to finish independently.
        drop(state);
        drop(tx);

        out
    };

    let rx = ReceiverStream::new(rx);
    let work = f(rx);

    futures::pin_mut!(work);
    futures::pin_mut!(forward);

    Either::factor_first(futures::future::select(work, forward).await).0
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio_stream::StreamExt;

    use super::*;

    #[tokio::test]
    async fn test_reborrow_stream_for_static_prefers_work_result_after_hangup() {
        let result = reborrow_stream_for_static(
            futures::stream::empty::<u8>(),
            |mut stream| async move {
                assert_eq!(stream.next().await, Some(7));
                "work"
            },
            || Some(7),
            || Some((Duration::from_secs(1), "timeout")),
        )
        .await;

        assert_eq!(result, "work");
    }

    #[tokio::test]
    async fn test_reborrow_stream_for_static_times_out_after_hangup() {
        let result = reborrow_stream_for_static(
            futures::stream::empty::<u8>(),
            |mut stream| async move {
                assert_eq!(stream.next().await, Some(7));
                futures::future::pending::<&'static str>().await
            },
            || Some(7),
            || Some((Duration::from_millis(10), "timeout")),
        )
        .await;

        assert_eq!(result, "timeout");
    }

    #[tokio::test]
    async fn test_reborrow_stream_for_static_waits_for_work_if_receiver_drops_first() {
        let result = reborrow_stream_for_static(
            futures::stream::pending::<u8>(),
            |stream| async move {
                drop(stream);
                tokio::time::sleep(Duration::from_millis(20)).await;
                "work"
            },
            || Some(7),
            || Some((Duration::ZERO, "timeout")),
        )
        .await;

        assert_eq!(result, "work");
    }

    #[tokio::test]
    async fn test_reborrow_stream_for_static_forwards_item_before_capacity_is_available() {
        let result = reborrow_stream_for_static(
            futures::stream::iter([1u8, 2]),
            |mut stream| async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
                assert_eq!(stream.next().await, Some(1));
                tokio::time::sleep(Duration::from_millis(10)).await;
                assert_eq!(stream.next().await, Some(2));
                "work"
            },
            || None,
            || Some((Duration::from_millis(100), "timeout")),
        )
        .await;

        assert_eq!(result, "work");
    }

    #[tokio::test]
    async fn test_reborrow_stream_for_static_does_not_treat_receiver_closure_as_eof() {
        let result = tokio::time::timeout(
            Duration::from_millis(20),
            reborrow_stream_for_static(
                futures::stream::pending::<u8>(),
                |stream| async move {
                    drop(stream);
                    futures::future::pending::<&'static str>().await
                },
                || Some(7),
                || Some((Duration::ZERO, "timeout")),
            ),
        )
        .await;

        assert!(
            result.is_err(),
            "receiver closure should not trigger EOF fallback"
        );
    }
}
