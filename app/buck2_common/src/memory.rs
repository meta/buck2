/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Utilities for interacting with the jemalloc heap used by buck2.
//!
//! In order to make use of jemalloc's heap dump or profiling utilities, you must set jemalloc's
//! configuration environment variable to a suitable value prior to launching the daemon, such as:
//!  `export MALLOC_CONF=prof:true,prof_final:false,prof_prefix:/tmp/jeprof`
//! This turns on the profiler (`prof:true`), tells the profile to not take heap dump on process exit,
//! (`prof_final:false`), and write dumps to `/tmp/jeprof` if a file argument isn't given to `mallctl`
//! (`prof_prefix:/tmp/jeprof`).
//!
//! Note: on macOS the env variable is named `JE_MALLOC_CONF`.

#[cfg(fbcode_build)]
mod imp {
    use std::env;
    use std::ffi::CString;
    use std::io::BufWriter;
    use std::io::IntoInnerError;
    use std::path::Path;

    use allocator_stats::MALLOC_CONF_ENV;
    use buck2_error::BuckErrorContext;
    use buck2_error::conversion::from_any_with_tag;
    use tempfile::NamedTempFile;

    /// Output the current state of the heap to the filename specified.
    /// Intended to be used for debugging purposes.
    /// Requires `prof:true` in [`MALLOC_CONF_ENV`] to be set in environment
    /// variables when run, though must be built without it.
    pub fn write_heap_to_file(filename: &str) -> buck2_error::Result<()> {
        // `has_jemalloc` works correctly on macOS too
        if !allocator_stats::has_jemalloc() {
            return Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Input,
                "not using jemalloc; are you building with @//mode/dev or @//mode/dbgo?"
            ));
        }

        let prof_enabled = allocator_stats::mallctl_read::<bool>(c"opt.prof")
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Mallctl))?;
        if !prof_enabled {
            if env::var_os(MALLOC_CONF_ENV).is_some() {
                return Err(buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "the environment variable {MALLOC_CONF_ENV} is set, but profiling is not enabled. {MALLOC_CONF_ENV} must contain prof:true to enable the profiler"
                ));
            }

            return Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Input,
                "profiling is not enabled for this process; you must set the environment variable {MALLOC_CONF_ENV} to contain at least prof:true in order to profile"
            ));
        }

        let path = CString::new(filename)
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Input))?;
        eprintln!("dumping heap to: {filename:?}");
        allocator_stats::mallctl_write(c"prof.dump", path.as_c_str())
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Mallctl))?;
        Ok(())
    }

    /// Dump jemalloc's recent-allocations ring (`experimental.prof_recent`) to a
    /// file as JSON. Unlike the heap dump, each entry records an individual
    /// (sampled) allocation with its size, alloc/free stacks, and timestamps, so
    /// it can be sliced by size class and lifetime. Requires the daemon to have
    /// been started with `prof:true,prof_recent_alloc_max:N` in [`MALLOC_CONF_ENV`];
    /// with the default of 0 the ring is empty, -1 unlimited.
    pub fn write_recent_alloc_to_file(filename: &str) -> buck2_error::Result<()> {
        if !allocator_stats::has_jemalloc() {
            return Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Input,
                "not using jemalloc; are you building with @//mode/dev or @//mode/dbgo?"
            ));
        }

        let prof_enabled = allocator_stats::mallctl_read::<bool>(c"opt.prof")
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Mallctl))?;
        if !prof_enabled {
            return Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Input,
                "profiling is not enabled for this process; restart the daemon with the environment variable {MALLOC_CONF_ENV} set to at least `prof:true,prof_recent_alloc_max:1000000`"
            ));
        }

        // `opt.prof` being on is necessary but not sufficient: the recent-alloc
        // ring is separately sized by `prof_recent_alloc_max`, which defaults to
        // 0 (records nothing). Without this check a daemon started with just
        // `prof:true` would silently produce an empty dump that looks like a
        // success. A value of -1 means unlimited, so only 0 is disabled.
        let ring_max =
            allocator_stats::mallctl_read::<isize>(c"experimental.prof_recent.alloc_max")
                .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Mallctl))?;
        if ring_max == 0 {
            return Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Input,
                "the recent-allocation ring is disabled (`prof_recent_alloc_max` is 0); restart the daemon with the environment variable {MALLOC_CONF_ENV} set to at least `prof:true,prof_recent_alloc_max:1000000`"
            ));
        }

        eprintln!("dumping recent allocations to: {filename:?}");
        // Dump into a temporary file next to the destination and only move it into place once it
        // is complete, so that a failed dump never leaves a truncated file at `filename`.
        // A bare filename's parent is `Some("")`, which `NamedTempFile::new_in` rejects.
        let dir = Path::new(filename)
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut file =
            BufWriter::new(NamedTempFile::new_in(dir).with_buck_error_context(|| {
                format!("creating a temporary file next to `{filename}`")
            })?);
        allocator_stats::recent_alloc_dump(&mut file)
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Mallctl))?;
        file.into_inner()
            .map_err(IntoInnerError::into_error)
            .with_buck_error_context(|| format!("writing `{filename}`"))?
            .persist(filename)
            .map_err(|e| e.error)
            .with_buck_error_context(|| format!("writing `{filename}`"))?;
        Ok(())
    }

    /// Dump allocator stats from JEMalloc. Intended for debug purposes
    pub fn allocator_stats(options: &str) -> buck2_error::Result<String> {
        allocator_stats::malloc_stats(options)
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::MallocStats))
    }

    /// Enables background threads for jemalloc. See [here](http://jemalloc.net/jemalloc.3.html#background_thread) for
    /// jemalloc documentation. When set, jemalloc will use background threads to purge unused dirty pages instead of
    /// doing it synchronously.
    ///
    /// This function has no effect if not using jemalloc.
    pub fn enable_background_threads() -> buck2_error::Result<()> {
        if memory::is_using_jemalloc() {
            memory::mallctl_write("background_thread", true)
                .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Mallctl))?;
        }
        Ok(())
    }

    pub fn has_jemalloc_stats() -> bool {
        memory::is_using_jemalloc()
    }

    /// Whether jemalloc is the allocator in this build. False on platforms it is
    /// not used on, notably Windows, even though those are `fbcode_build` too.
    pub fn is_using_jemalloc() -> bool {
        memory::is_using_jemalloc()
    }

    /// Force jemalloc to return dirty and muzzy pages to the OS immediately.
    pub fn purge_jemalloc() -> buck2_error::Result<()> {
        if !memory::is_using_jemalloc() {
            return Ok(());
        }
        // 4096=MALLCTL_ARENAS_ALL — see folly::getJEMallocMallctlArenasAll().
        memory::mallctl_call("arena.4096.purge")
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Mallctl))?;
        Ok(())
    }
}

#[cfg(not(fbcode_build))]
mod imp {
    pub fn write_heap_to_file(_filename: &str) -> buck2_error::Result<()> {
        // TODO(swgillespie) the `jemalloc_ctl` crate is probably capable of doing this
        // and we already link against it
        Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::Unimplemented,
            "not implemented: heap dump for Cargo builds"
        ))
    }

    pub fn write_recent_alloc_to_file(_filename: &str) -> buck2_error::Result<()> {
        Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::Unimplemented,
            "not implemented: recent-alloc dump for Cargo builds"
        ))
    }

    pub fn allocator_stats(_: &str) -> buck2_error::Result<String> {
        Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::Unimplemented,
            "not implemented: allocator stats  for Cargo builds"
        ))
    }

    pub fn enable_background_threads() -> buck2_error::Result<()> {
        Ok(())
    }

    pub fn has_jemalloc_stats() -> bool {
        false
    }

    pub fn is_using_jemalloc() -> bool {
        false
    }

    pub fn purge_jemalloc() -> buck2_error::Result<()> {
        Ok(())
    }
}

pub use imp::allocator_stats;
pub use imp::enable_background_threads;
pub use imp::has_jemalloc_stats;
pub use imp::is_using_jemalloc;
pub use imp::purge_jemalloc;
pub use imp::write_heap_to_file;
pub use imp::write_recent_alloc_to_file;
