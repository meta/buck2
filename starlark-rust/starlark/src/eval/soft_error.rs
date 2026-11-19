/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

/// Deprecation handler provided by a user.
pub trait SoftErrorHandler {
    /// Handle deprecation error. If this function returns `Ok`, error will be ignored,
    /// otherwise error will be propagated.
    fn soft_error(&self, category: &str, error: crate::Error) -> Result<(), crate::Error>;
}

/// Default handler: warnings are treated as errors.
pub(crate) struct HardErrorSoftErrorHandler;

impl SoftErrorHandler for HardErrorSoftErrorHandler {
    fn soft_error(&self, _category: &str, error: crate::Error) -> Result<(), crate::Error> {
        Err(error)
    }
}

static GLOBAL_SOFT_ERROR_HANDLER: std::sync::OnceLock<&'static (dyn SoftErrorHandler + Sync)> =
    std::sync::OnceLock::new();

/// Installs a process-global fallback handler, used to report recoverable
/// internal errors from places where no [`Evaluator`](crate::eval::Evaluator)
/// is in reach, such as value freezing or span resolution. Without one, such
/// reports go to stderr.
///
/// The first installation wins; later calls are ignored, so a process with two
/// initialization sites keeps reporting through the handler installed first.
pub fn set_global_soft_error_handler(handler: &'static (dyn SoftErrorHandler + Sync)) {
    let _first_installation_wins = GLOBAL_SOFT_ERROR_HANDLER.set(handler);

    // `starlark_syntax` cannot reach this handler directly, so bridge its
    // corrupt-span reports through here.
    starlark_syntax::codemap::set_corrupt_span_reporter(|message| {
        global_soft_error(
            "corrupt_codemap_span",
            crate::Error::new_other(std::io::Error::other(message.to_owned())),
        );
    });
}

/// Reports through the global handler; the report is advisory, so a handler
/// that chooses to fail cannot propagate from here.
pub(crate) fn global_soft_error(category: &str, error: crate::Error) {
    match GLOBAL_SOFT_ERROR_HANDLER.get() {
        Some(handler) => {
            let _cannot_propagate = handler.soft_error(category, error);
        }
        None => {
            static REPORTED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !REPORTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!("starlark: {category}: {error}");
            }
        }
    }
}
