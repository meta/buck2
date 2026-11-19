/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::thread;
use std::time::Duration;

use buck2_cli_proto::GenericResponse;
use buck2_cli_proto::UnstableCrashRequest;
use buck2_cli_proto::unstable_crash_request::CrashType;
use buck2_error::BuckErrorContext;

pub(crate) async fn crash(req: UnstableCrashRequest) -> buck2_error::Result<GenericResponse> {
    let crash_type = CrashType::try_from(req.crash_type).map_err(|_| {
        buck2_error::buck2_error!(buck2_error::ErrorTag::CrashRequested, "{}", "bad request")
    })?;
    match crash_type {
        CrashType::Panic => {
            crash_on_dedicated_thread(|| panic!("explicitly requested panic (via unstable_crash)"))
        }
        CrashType::Abort => {
            // Crash with SIGABRT.
            // Should trigger folly signal handler to dump stack trace.
            // SIGSEGV,SIGTERM,SIGBUS,SIGILL,etc. should behave similarly.
            // https://fburl.com/code/ap385ats
            crash_on_dedicated_thread(|| std::process::abort())
        }
        CrashType::Oom => allocate_memory(req.bytes).await,
    }
}

fn crash_on_dedicated_thread(
    crash: impl FnOnce() + Send + 'static,
) -> buck2_error::Result<GenericResponse> {
    // Keep intentional crashes on a dedicated thread so their backtraces are isolated
    // from whichever async stack happens to be servicing the request.
    thread::Builder::new()
        .name("buck2-crash".to_owned())
        .spawn(crash)
        .buck_error_context("Failed to spawn crash thread")?
        .join()
        .expect("crash thread should terminate the daemon");
    Ok(GenericResponse {})
}

const NUM_THREADS: usize = 4;
const SLEEP_DURATION: Duration = Duration::from_millis(10);

#[cfg(unix)]
fn query_page_size() -> buck2_error::Result<usize> {
    buck2_util::os::unix_like::sc_page_size::sc_page_size()
}

#[cfg(windows)]
fn query_page_size() -> buck2_error::Result<usize> {
    buck2_util::os::win::page_size::page_size()
}

async fn allocate_memory(bytes: u64) -> buck2_error::Result<GenericResponse> {
    let page_size = query_page_size()?;

    tokio::task::spawn_blocking(move || {
        let bytes = bytes as usize;
        let chunk_size = bytes / NUM_THREADS;

        thread::scope(|s| {
            for i in 0..NUM_THREADS {
                let size = if i == NUM_THREADS - 1 {
                    bytes - chunk_size * (NUM_THREADS - 1)
                } else {
                    chunk_size
                };

                s.spawn(move || {
                    allocate_and_pressure(size, page_size);
                });
            }
        });

        unreachable!("allocate_and_pressure loops forever")
    })
    .await
    .buck_error_context("Failed to spawn allocate_memory task")?
}

fn allocate_and_pressure(bytes: usize, page_size: usize) -> ! {
    // Allocate memory and touch every page to force physical allocation.
    let mut mem = vec![0u8; bytes];
    for i in (0..bytes).step_by(page_size) {
        mem[i] = 1;
    }

    // Continuously modify every page in a loop. This forces pages that
    // have been paged out back into physical memory, generating sustained
    // memory pressure.
    loop {
        for i in (0..bytes).step_by(page_size) {
            mem[i] = mem[i].wrapping_add(1);
        }
        // Prevent the compiler from optimizing away the allocation.
        std::hint::black_box(&mem);
        thread::sleep(SLEEP_DURATION);
    }
}
