/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::Cell;
use std::future::Future;
use std::hint;
use std::pin::Pin;
use std::pin::pin;
use std::sync::OnceLock;
use std::task::Poll;
use std::thread;

use buck2_error::BuckErrorOptionContext;
use buck2_error::internal_error;

/// Concurrency for filesystem operations that modify directory structure (unlinks, rmdirs,
/// renames). Their throughput peaks at a small number of concurrent workers on all filesystems
/// we care about (sometimes earlier, but 4 is the one-size-fits-all value) and degrades with
/// more — D33922298 has benchmark details. Clamped to the host's available parallelism.
pub fn directory_mutation_parallelism() -> usize {
    const DIRECTORY_MUTATION_THREADS: usize = 4;
    DIRECTORY_MUTATION_THREADS.min(available_parallelism())
}

/// Get the available parallelism
///
/// This value is cached for the lifetime of the process. The reason is that there are various
/// components that cannot be updated to reflect the new value if it changes during the lifetime of
/// the daemon. Caching this sacrifices some accuracy of this value in exchange for putting the
/// daemon into a more predictable state.
///
/// Use `available_parallelism_fresh` if the caching is not desired
pub fn available_parallelism() -> usize {
    static PARALLELISM: OnceLock<usize> = OnceLock::new();

    *PARALLELISM.get_or_init(available_parallelism_fresh)
}

/// Get the available parallelism
///
/// Unlike `available_parallelism`, this is not cached - callers using this should ensure that this
/// value is logged somewhere
pub fn available_parallelism_fresh() -> usize {
    // NB: num_cpus and tokio both also use 1 as the default in case of an error
    std::thread::available_parallelism().map_or(1, |v| v.get())
}

/// Default stack size for buck2.
///
/// We want to be independent of possible future changes to the default stack size in Rust.
pub(crate) const THREAD_DEFAULT_STACK_SIZE: usize = {
    if cfg!(debug_assertions) {
        // Need 4MB for windows-debug according to D60449433.
        4 << 20
    } else {
        2 << 20
    }
};

fn thread_builder(name: &str) -> thread::Builder {
    thread::Builder::new()
        .stack_size(THREAD_DEFAULT_STACK_SIZE)
        .name(name.to_owned())
}

pub fn thread_spawn<T, F>(name: &str, code: F) -> std::io::Result<thread::JoinHandle<T>>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    thread_builder(name).spawn(move || {
        on_thread_start();
        let r = code();
        on_thread_stop();
        r
    })
}

pub fn thread_spawn_scoped<'scope, 'env: 'scope, T, F>(
    name: &str,
    scope: &'scope thread::Scope<'scope, 'env>,
    code: F,
) -> std::io::Result<thread::ScopedJoinHandle<'scope, T>>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'scope,
{
    thread_builder(name).spawn_scoped(scope, move || {
        on_thread_start();
        let r = code();
        on_thread_stop();
        r
    })
}

pub(crate) fn stack_pointer() -> *const () {
    let mut x: u32 = 0;
    hint::black_box(&mut x as *const u32 as *const ())
}

#[derive(Copy, Clone)]
struct ValidStackRange {
    start: *const (),
    end: *const (),
}

impl ValidStackRange {
    fn full_range() -> ValidStackRange {
        let start = usize::MAX as *const ();
        let end = usize::MIN as *const ();
        ValidStackRange { start, end }
    }
}

thread_local! {
    static STACK_RANGE: Cell<Option<ValidStackRange >> = const { Cell::new(None) };
}

pub(crate) fn on_thread_start() {
    assert!(
        STACK_RANGE.get().is_none(),
        "stack range must not be set in a new thread"
    );
    let stack_pointer = stack_pointer();
    // Stack grows downwards. So we add to the start and subtract from the end.
    // Add a little bit to the start because we don't really know where the stack starts.
    let start = (stack_pointer as usize).checked_add(0x1000).unwrap() as *const ();
    // Subtract 3/4 to catch stack overflow before program crashes.
    let end = (stack_pointer as usize)
        .checked_sub(THREAD_DEFAULT_STACK_SIZE / 4 * 3)
        .unwrap() as *const ();
    let stack_range = ValidStackRange { start, end };
    STACK_RANGE.set(Some(stack_range));
}

pub(crate) fn on_thread_stop() {
    let range = STACK_RANGE.replace(None);
    assert!(range.is_some(), "stack range must be set in a thread");
}

pub fn check_stack_overflow() -> buck2_error::Result<()> {
    let stack_range = STACK_RANGE.get().internal_error("stack range not set")?;
    let stack_pointer = stack_pointer();
    if stack_pointer > stack_range.start {
        return Err(internal_error!("stack underflow, should not happen"));
    }
    if stack_pointer < stack_range.end {
        return Err(internal_error!("stack overflow"));
    }
    Ok(())
}

#[must_use]
pub struct IgnoreStackOverflowChecksForCurrentThread {
    prev: Option<ValidStackRange>,
}

impl Drop for IgnoreStackOverflowChecksForCurrentThread {
    fn drop(&mut self) {
        STACK_RANGE.set(self.prev.take());
    }
}

/// For tests.
pub fn ignore_stack_overflow_checks_for_current_thread() -> IgnoreStackOverflowChecksForCurrentThread
{
    let prev = STACK_RANGE.replace(Some(ValidStackRange::full_range()));
    IgnoreStackOverflowChecksForCurrentThread { prev }
}

/// For tests.
pub async fn ignore_stack_overflow_checks_for_future<F: Future>(f: F) -> F::Output {
    let f = pin!(f);

    struct IgnoreStackOverflowChecksForFuture<'a, F> {
        f: Pin<&'a mut F>,
    }

    impl<F: Future> Future for IgnoreStackOverflowChecksForFuture<'_, F> {
        type Output = F::Output;

        fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
            let _ignore = ignore_stack_overflow_checks_for_current_thread();
            self.f.as_mut().poll(cx)
        }
    }

    IgnoreStackOverflowChecksForFuture { f }.await
}

// Overflowing the stack and catching it is signal handling, which Miri does not model.
#[cfg(all(test, not(miri)))]
pub(crate) mod tests {
    use std::hint;

    use crate::threads::check_stack_overflow;
    use crate::threads::thread_spawn;

    pub(crate) fn recursive_function(frames: u32) -> buck2_error::Result<()> {
        let Some(frames) = frames.checked_sub(1) else {
            return Ok(());
        };

        check_stack_overflow()?;

        // Allocate a string on the stack so the compiler won't optimize the recursion away.
        let mut x = String::new();
        hint::black_box(&mut x);
        recursive_function(frames)?;
        hint::black_box(&mut x);
        Ok(())
    }

    #[test]
    fn test_catch_stack_overflow() {
        let error = thread_spawn("test", || recursive_function(u32::MAX))
            .unwrap()
            .join()
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("stack overflow"), "{error:?}");
    }

    #[test]
    fn test_no_stack_overflow() {
        let () = thread_spawn("test", || recursive_function(1000))
            .unwrap()
            .join()
            .unwrap()
            .unwrap();
    }
}
