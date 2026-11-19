/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::OnceLock;

use fbinit::FacebookInit;

fn run_init() -> FacebookInit {
    // SAFETY: Only called within a oncelock
    let fb = unsafe { fbinit::perform_init() };

    #[cfg(fbcode_build)]
    {
        use gflags::GflagValue;

        // There are two sources of log spew when building buck2 with Buck and linking against fbcode:
        //   1. folly/logging/xlog, which can be configured via a special configuration string, which we use to
        //      log only critical-level logs. https://github.com/facebook/folly/blob/master/folly/logging/docs/Config.md
        //   2. google log (glog), which is older but still used, which can configured using a flag at runtime.
        //
        // This first call handles the folly config.
        logging::update_logging_config(fb, "CRITICAL");
        drop(gflags::set_gflag_value(
            fb,
            "minloglevel",
            GflagValue::U32(5),
        ));
        drop(gflags::set_gflag_value(
            fb,
            "stderrthreshold",
            GflagValue::U32(5),
        ));

        // force-disable Agent context rehydration for all builds
        drop(gflags::set_gflag_value(
            fb,
            "agentlib_force_disable_rehydration_in_binary",
            GflagValue::Bool(true),
        ));
    }

    fb
}

/// Gets an fbinit token.
///
/// This function is lazy and safe to call from multiple threads, however:
///  1. You should still prefer to explicit pass `FacebookInit` around where possible. Use of this
///     function is primarily intended for a very early point in buck2's lifecycle where fbinit
///     initialization is lazy.
///  2. This function may not be called before any forks, as it may spawn threads.
pub fn get_or_init_fbcode_globals() -> FacebookInit {
    static FB: OnceLock<fbinit::FacebookInit> = OnceLock::new();

    *FB.get_or_init(run_init)
}
