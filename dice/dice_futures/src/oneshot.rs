/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A channel for one value whose sender can store the value first and wake the receiver later.
//!
//! Waking a task costs the sender much more than storing the value does, and from some threads
//! it costs more again. [`Sender::store`] does the storing and hands back a [`Stored`], which does
//! the waking when dropped, wherever that is. Otherwise this is `tokio::sync::oneshot` without the
//! sender-side notifications and without participation in tokio's cooperative budget, so that a
//! thread can block on the receiver from inside a task's poll.

use std::cell::UnsafeCell;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::Ordering::AcqRel;
use std::sync::atomic::Ordering::Acquire;
use std::task::Context;
use std::task::Poll;
use std::task::Waker;

/// Creates a channel for one value.
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let inner = Arc::new(Inner {
        state: AtomicU8::new(0),
        value: UnsafeCell::new(None),
        waker: UnsafeCell::new(None),
    });
    (
        Sender {
            inner: Some(inner.clone()),
        },
        Receiver { inner: Some(inner) },
    )
}

pub struct Sender<T> {
    /// `None` once the value is stored.
    inner: Option<Arc<Inner<T>>>,
}

/// Resolves to the value, or to an error if the sender was dropped without sending one.
pub struct Receiver<T> {
    /// `None` once resolved.
    inner: Option<Arc<Inner<T>>>,
}

/// A value stored in a channel whose receiver has not been woken for it yet. Dropping this wakes
/// it, so holding one back is holding the receiver back.
#[must_use = "the receiver is woken when this is dropped"]
pub struct Stored(Arc<dyn WakeReceiver + Send + Sync>);

/// The sender was dropped without sending a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("sender dropped without sending")]
pub struct RecvError;

/// The receiver has stored its waker in `waker`.
const RX_WAKER: u8 = 0b001;
/// The sender is done: it has stored its value in `value`, if it is going to.
const COMPLETE: u8 = 0b010;
/// The receiver is gone.
const CLOSED: u8 = 0b100;

struct Inner<T> {
    state: AtomicU8,
    /// Written by the sender before it sets `COMPLETE`, and read only by whoever observes
    /// `COMPLETE` set, with acquire ordering: the receiver, or the sender's `store` itself when
    /// it failed to set `COMPLETE` because the receiver was gone.
    value: UnsafeCell<Option<T>>,
    /// Written by the receiver only while `RX_WAKER` and `COMPLETE` are both clear, which it
    /// confirms from the result of the atomic operation that clears `RX_WAKER` before it writes.
    /// Read by whoever observes `RX_WAKER` set, with acquire ordering: the receiver, to compare its
    /// waker, and the sender, to wake, which it does only after setting `COMPLETE`. So a read by
    /// the sender never overlaps a write: the write would have to follow a clearing of `RX_WAKER`
    /// that observed `COMPLETE` clear, when no sender read had begun, and that same clearing hides
    /// the slot from any later read.
    waker: UnsafeCell<Option<Waker>>,
}

// SAFETY: Every access to the cells is governed by `state` as documented on them; `T` crosses
// threads once, from the sender to the receiver.
unsafe impl<T: Send> Send for Inner<T> {}
unsafe impl<T: Send> Sync for Inner<T> {}

impl<T> Inner<T> {
    /// Sets `COMPLETE`, unless the receiver is gone, in which case `false` and the state is left
    /// as it was.
    fn complete(&self) -> bool {
        let mut state = self.state.load(Acquire);
        loop {
            if state & CLOSED != 0 {
                return false;
            }
            match self
                .state
                .compare_exchange_weak(state, state | COMPLETE, AcqRel, Acquire)
            {
                Ok(_) => return true,
                Err(actual) => state = actual,
            }
        }
    }

    /// # Safety
    ///
    /// The caller is the one party allowed to access `value` at this point, see there.
    unsafe fn take_value(&self) -> Option<T> {
        unsafe { (*self.value.get()).take() }
    }
}

trait WakeReceiver {
    /// Wakes the receiver, if it is waiting. Called only after `COMPLETE` is set.
    fn wake_receiver(&self);
}

impl<T> WakeReceiver for Inner<T> {
    fn wake_receiver(&self) {
        let state = self.state.load(Acquire);
        if state & RX_WAKER != 0 && state & CLOSED == 0 {
            // SAFETY: `RX_WAKER` was observed set with acquire ordering after `COMPLETE` was set,
            // see `waker`.
            if let Some(waker) = unsafe { &*self.waker.get() } {
                waker.wake_by_ref();
            }
        }
    }
}

impl<T: Send + 'static> Sender<T> {
    /// Stores the value for the receiver without waking it; the returned [`Stored`] does that.
    /// Returns the value if the receiver is gone.
    pub fn store(mut self, value: T) -> Result<Stored, T> {
        let inner = self.inner.take().expect("a sender stores at most once");
        // SAFETY: `COMPLETE` is clear, since only this method sets it and it consumes the sender,
        // so nothing else accesses `value`.
        unsafe {
            *inner.value.get() = Some(value);
        }
        if inner.complete() {
            Ok(Stored(inner))
        } else {
            // SAFETY: `COMPLETE` is still clear and stays so, and the receiver is gone.
            Err(unsafe { inner.take_value() }.expect("stored above"))
        }
    }

    /// Stores the value and wakes the receiver. Returns the value if the receiver is gone.
    pub fn send(self, value: T) -> Result<(), T> {
        self.store(value).map(Stored::wake)
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take()
            && inner.complete()
        {
            inner.wake_receiver();
        }
    }
}

impl Stored {
    /// Wakes the receiver now.
    pub fn wake(self) {
        drop(self)
    }
}

impl Drop for Stored {
    fn drop(&mut self) {
        self.0.wake_receiver();
    }
}

impl<T> Future for Receiver<T> {
    type Output = Result<T, RecvError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let inner = self
            .inner
            .as_ref()
            .expect("`Receiver` polled after completion");
        let mut state = inner.state.load(Acquire);
        if state & COMPLETE == 0 {
            if state & RX_WAKER != 0 {
                // SAFETY: `RX_WAKER` is set and only this receiver clears it, see `waker`.
                let registered = unsafe { &*inner.waker.get() };
                if registered.as_ref().is_some_and(|w| w.will_wake(cx.waker())) {
                    return Poll::Pending;
                }
                state = inner.state.fetch_and(!RX_WAKER, AcqRel) & !RX_WAKER;
                if state & COMPLETE != 0 {
                    // The sender may be reading the registered waker; leaving it in place, and
                    // `RX_WAKER` clear, is what keeps it from waking for nothing.
                    let inner = self.inner.take().expect("checked above");
                    // SAFETY: `COMPLETE` was observed with acquire ordering, see `value`.
                    return Poll::Ready(unsafe { inner.take_value() }.ok_or(RecvError));
                }
            }
            // SAFETY: `RX_WAKER` is clear, and `COMPLETE` was clear when it was last observed so,
            // see `waker`.
            unsafe {
                *inner.waker.get() = Some(cx.waker().clone());
            }
            state = inner.state.fetch_or(RX_WAKER, AcqRel) | RX_WAKER;
            if state & COMPLETE == 0 {
                return Poll::Pending;
            }
            // The value arrived meanwhile. The waker stays registered until the channel goes;
            // the sender may wake it, which is harmless.
        }
        let inner = self.inner.take().expect("checked above");
        // SAFETY: `COMPLETE` was observed with acquire ordering, see `value`.
        Poll::Ready(unsafe { inner.take_value() }.ok_or(RecvError))
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            let prev = inner.state.fetch_or(CLOSED, AcqRel);
            if prev & COMPLETE != 0 {
                // SAFETY: `COMPLETE` was observed with acquire ordering, see `value`.
                drop(unsafe { inner.take_value() });
            }
            // A registered waker stays until the channel goes: a sender that completed may be
            // about to read it.
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering::SeqCst;
    use std::task::Wake;

    use super::*;

    struct Counter(AtomicUsize);

    impl Wake for Counter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, SeqCst);
        }
    }

    fn counting_waker() -> (Arc<Counter>, Waker) {
        let counter = Arc::new(Counter(AtomicUsize::new(0)));
        (counter.clone(), Waker::from(counter))
    }

    fn poll<T>(rx: &mut Receiver<T>, waker: &Waker) -> Poll<Result<T, RecvError>> {
        Pin::new(rx).poll(&mut Context::from_waker(waker))
    }

    #[test]
    fn send_before_poll() {
        let (tx, rx) = channel();
        tx.send(1).unwrap();
        assert_eq!(futures::executor::block_on(rx), Ok(1));
    }

    #[test]
    fn stored_value_is_there_before_the_wake() {
        let (tx, mut rx) = channel();
        let (count, waker) = counting_waker();
        assert!(poll(&mut rx, &waker).is_pending());
        let stored = tx.store(1).unwrap();
        assert_eq!(count.0.load(SeqCst), 0);
        assert_eq!(poll(&mut rx, &waker), Poll::Ready(Ok(1)));
        // Waking a receiver that has already taken the value is allowed, and harmless.
        stored.wake();
    }

    #[test]
    fn store_then_wake() {
        let (tx, mut rx) = channel();
        let (count, waker) = counting_waker();
        assert!(poll(&mut rx, &waker).is_pending());
        let stored = tx.store(1).unwrap();
        assert_eq!(count.0.load(SeqCst), 0);
        stored.wake();
        assert_eq!(count.0.load(SeqCst), 1);
        assert_eq!(poll(&mut rx, &waker), Poll::Ready(Ok(1)));
    }

    #[test]
    fn dropped_sender_is_an_error() {
        let (tx, mut rx) = channel::<i32>();
        let (count, waker) = counting_waker();
        assert!(poll(&mut rx, &waker).is_pending());
        drop(tx);
        assert_eq!(count.0.load(SeqCst), 1);
        assert_eq!(poll(&mut rx, &waker), Poll::Ready(Err(RecvError)));
    }

    #[test]
    fn dropped_receiver_returns_the_value() {
        let (tx, rx) = channel();
        drop(rx);
        assert_eq!(tx.store(1).err(), Some(1));
    }

    #[test]
    fn receiver_dropped_after_store() {
        let (tx, mut rx) = channel();
        let (count, waker) = counting_waker();
        assert!(poll(&mut rx, &waker).is_pending());
        let stored = tx.store(Arc::new(1)).unwrap();
        drop(rx);
        stored.wake();
        assert_eq!(count.0.load(SeqCst), 0);
    }

    #[test]
    fn the_latest_waker_is_woken() {
        let (tx, mut rx) = channel();
        let (first, first_waker) = counting_waker();
        let (second, second_waker) = counting_waker();
        assert!(poll(&mut rx, &first_waker).is_pending());
        assert!(poll(&mut rx, &second_waker).is_pending());
        tx.send(1).unwrap();
        assert_eq!(first.0.load(SeqCst), 0);
        assert_eq!(second.0.load(SeqCst), 1);
        assert_eq!(poll(&mut rx, &second_waker), Poll::Ready(Ok(1)));
    }

    #[test]
    fn unpolled_receiver_gets_the_value() {
        let (tx, rx) = channel();
        tx.store(1).unwrap().wake();
        assert_eq!(futures::executor::block_on(rx), Ok(1));
    }

    /// Senders and receivers on different threads, with the receiver re-registering a fresh waker
    /// on every poll so that the replacement races the store.
    #[test]
    fn concurrent() {
        let rounds = if cfg!(miri) { 30 } else { 3000 };
        for round in 0..rounds {
            let (tx, mut rx) = channel();
            let receiver = std::thread::spawn(move || {
                loop {
                    let (_count, waker) = counting_waker();
                    if let Poll::Ready(v) = poll(&mut rx, &waker) {
                        return v;
                    }
                    std::thread::yield_now();
                }
            });
            if round % 2 == 0 {
                std::thread::yield_now();
            }
            let stored = tx.store(round).unwrap();
            if round % 3 == 0 {
                std::thread::yield_now();
            }
            stored.wake();
            assert_eq!(receiver.join().unwrap(), Ok(round));
        }
    }

    /// As `concurrent`, with receivers that give up, and senders that are dropped.
    #[test]
    fn concurrent_with_drops() {
        let rounds = if cfg!(miri) { 30 } else { 3000 };
        for round in 0..rounds {
            let (tx, mut rx) = channel();
            let receiver = std::thread::spawn(move || {
                let (_count, waker) = counting_waker();
                let mut result = poll(&mut rx, &waker);
                if round % 4 == 1 {
                    // Gives up after one poll, dropping the receiver.
                    return result.is_ready();
                }
                loop {
                    if let Poll::Ready(v) = result {
                        return v.is_ok();
                    }
                    std::thread::yield_now();
                    result = poll(&mut rx, &waker);
                }
            });
            if round % 2 == 0 {
                std::thread::yield_now();
            }
            if round % 4 == 3 {
                drop(tx);
                assert!(!receiver.join().unwrap());
                continue;
            }
            let stored = tx.store(Arc::new(round));
            std::thread::yield_now();
            drop(stored);
            let received = receiver.join().unwrap();
            if round % 4 != 1 {
                assert!(received);
            }
        }
    }
}
