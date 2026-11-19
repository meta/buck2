/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A future bundled with data it borrows, so that the two can be spawned as one.

use std::future::Future;
use std::mem;
use std::pin::Pin;
use std::ptr::NonNull;
use std::task::Context;
use std::task::Poll;
use std::task::ready;

use futures::future::BoxFuture;

use crate::maybe_future::MaybeFuture;

pub struct OwningFuture<T, D> {
    /// Borrows `*data`. The `'static` is a lie that `take_data` keeps honest by disposing of the
    /// future before the data.
    fut: MaybeFuture<BoxFuture<'static, T>>,
    /// Behind a pointer rather than stored inline because every `poll` reborrows `self`. Under
    /// the aliasing rules a reborrow covers what is stored inline in `self`, and so would
    /// invalidate `fut`'s borrow of the data, but not what a raw pointer points at.
    ///
    /// `None` once `take_data` has run.
    data: Option<NonNull<D>>,
}

// SAFETY: The `D` behind `data` is owned outright, as it would be by a `Box<D>` field. `fut` is a
// `BoxFuture`, which is `Send` by construction.
unsafe impl<T, D: Send> Send for OwningFuture<T, D> {}

impl<T, D> OwningFuture<T, D> {
    pub fn new<F>(data: D, f: F) -> Pin<Box<Self>>
    where
        F: for<'a> FnOnce(&'a mut D) -> BoxFuture<'a, T>,
    {
        let mut data = NonNull::from(Box::leak(Box::new(data)));
        // SAFETY: `data` is freshly allocated and nothing else refers to it. The borrow handed to
        // `f` lives on in `fut`, which `take_data` disposes of before freeing `data`.
        let fut =
            unsafe { mem::transmute::<BoxFuture<'_, T>, BoxFuture<'static, T>>(f(data.as_mut())) };
        Box::pin(OwningFuture {
            fut: MaybeFuture::Fut(fut),
            data: Some(data),
        })
    }

    /// Drops the future and takes the data it borrowed.
    fn take_data(&mut self) -> Option<D> {
        self.fut = MaybeFuture::None;
        let data = self.data.take()?;
        // SAFETY: `data` came from `Box::leak` in `new`, and its only borrower is gone.
        Some(*unsafe { Box::from_raw(data.as_ptr()) })
    }

    /// Maps the future with the given 'f', except it takes the data that the original future
    /// owns as part of the mapping.
    pub fn map_taking_data<U>(
        self: Pin<Box<OwningFuture<T, D>>>,
        f: impl FnOnce(T, D) -> U + Unpin,
    ) -> impl Future<Output = U> {
        #[must_use = "futures do nothing unless you `.await` or poll them"]
        pub enum Map<T, D, F> {
            Incomplete {
                future: Pin<Box<OwningFuture<T, D>>>,
                f: F,
            },
            Complete,
        }

        impl<T, D, F, U> Future for Map<T, D, F>
        where
            F: FnOnce(T, D) -> U + Unpin,
        {
            type Output = U;

            fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                let (output, d) = match self.as_mut().get_mut() {
                    Map::Incomplete { future, .. } => {
                        let output = ready!(future.as_mut().poll(cx));
                        let d = future.take_data().expect("taken twice");
                        (output, d)
                    }
                    Map::Complete => {
                        panic!("Map must not be polled after it returned `Poll::Ready`")
                    }
                };

                match mem::replace(self.get_mut(), Map::Complete) {
                    Map::Incomplete { f, .. } => Poll::Ready(f(output, d)),
                    Map::Complete => {
                        unreachable!()
                    }
                }
            }
        }

        Map::Incomplete { future: self, f }
    }
}

impl<T, D> Drop for OwningFuture<T, D> {
    fn drop(&mut self) {
        drop(self.take_data());
    }
}

impl<T, D> Future for OwningFuture<T, D> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.fut).poll(cx)
    }
}

// `#[tokio::test]` starts tokio's IO driver: epoll, kqueue or IOCP by OS, and Miri models only
// epoll.
#[cfg(all(test, any(not(miri), target_os = "linux")))]
mod tests {
    use futures::FutureExt;
    use futures::future::BoxFuture;

    use crate::owning_future::OwningFuture;

    #[tokio::test]
    async fn test_future() {
        struct SomeData(usize);

        fn some_fn(d: &mut SomeData) -> BoxFuture<'_, usize> {
            async move { d.0 }.boxed()
        }

        let data = SomeData(2);
        let fut = OwningFuture::new(data, some_fn);

        // we can spawn the future such that it owns the data
        let data = tokio::spawn(fut).await.unwrap();
        assert_eq!(data, 2);
    }

    #[tokio::test]
    async fn test_future_mapping() {
        struct SomeData(usize);

        fn some_fn(d: &mut SomeData) -> BoxFuture<'_, usize> {
            async move { d.0 }.boxed()
        }

        let data = SomeData(2);
        let fut = OwningFuture::new(data, some_fn);
        let fut = fut.map_taking_data(|x, data| x + data.0);

        // we can spawn the future such that it owns the data
        let data = tokio::spawn(fut).await.unwrap();
        assert_eq!(data, 4);
    }
}
