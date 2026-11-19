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
use std::sync::OnceLock;

use allocative::Allocative;
use futures::Future;
use tokio::sync::Mutex;

#[derive(Allocative)]
pub struct AsyncOnceCell<T> {
    cell: OnceLock<T>,
    #[allocative(skip)] // Mutex has allocations, but no simple way to measure it.
    initialized: Mutex<()>,
}

impl<T> AsyncOnceCell<T> {
    pub fn new() -> Self {
        Self {
            cell: OnceLock::new(),
            initialized: Mutex::new(()),
        }
    }

    pub fn get(&self) -> Option<&T> {
        self.cell.get()
    }

    pub async fn get_or_try_init<E, F: Future<Output = Result<T, E>>>(
        &self,
        fut: F,
    ) -> Result<&T, E> {
        if let Some(val) = self.cell.get() {
            return Ok(val);
        }

        let _guard = self.initialized.lock().await;

        if let Some(val) = self.cell.get() {
            return Ok(val);
        }

        let val = fut.await?;

        match self.cell.set(val) {
            Ok(()) => Ok(self.cell.get().unwrap()),
            Err(_) => unreachable!(),
        }
    }

    pub async fn get_or_init<F: Future<Output = T>>(&self, fut: F) -> &T {
        match self
            .get_or_try_init(async { Ok::<_, Infallible>(fut.await) })
            .await
        {
            Ok(val) => val,
        }
    }
}

// `#[tokio::test]` starts tokio's IO driver: epoll, kqueue or IOCP by OS, and Miri models only
// epoll.
#[cfg(all(test, any(not(miri), target_os = "linux")))]
mod tests {
    use buck2_error::buck2_error;

    use super::*;

    #[tokio::test]
    async fn test() -> buck2_error::Result<()> {
        let cell1 = AsyncOnceCell::new();

        let counter = cell1.get_or_init(async { 42u32 }).await;
        assert_eq!(42, *counter);

        let counter = cell1.get_or_init(async { 11111111u32 }).await;
        assert_eq!(42, *counter);

        // There's no guarantee that these two actually race with each other, we can only hope.
        let cell2 = AsyncOnceCell::new();
        let ref1 = cell2.get_or_init(async { tokio::spawn(async { 1u32 }).await.unwrap() });
        let ref2 = cell2.get_or_init(async { tokio::spawn(async { 2u32 }).await.unwrap() });

        let (v1, v2) = tokio::join!(ref1, ref2);

        assert!(*v1 == 1 || *v1 == 2);
        assert_eq!(v1, v2);

        Ok(())
    }

    #[tokio::test]
    async fn test_get_or_try_init() -> buck2_error::Result<()> {
        let cell1 = AsyncOnceCell::new();

        assert_eq!(
            &43,
            cell1
                .get_or_try_init(async { buck2_error::Ok(43) })
                .await
                .unwrap()
        );
        assert_eq!(
            &43,
            cell1
                .get_or_try_init(async { buck2_error::Ok(55) })
                .await
                .unwrap()
        );

        let cell2 = AsyncOnceCell::new();
        cell2
            .get_or_try_init(async { Err(buck2_error!(buck2_error::ErrorTag::TestOnly, "foo")) })
            .await
            .unwrap_err();
        assert_eq!(
            &44,
            cell2
                .get_or_try_init(async { buck2_error::Ok(44) })
                .await
                .unwrap()
        );
        assert_eq!(
            &44,
            cell2
                .get_or_try_init(async { buck2_error::Ok(56) })
                .await
                .unwrap()
        );

        Ok(())
    }
}
