/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fs::File;
use std::ops::ControlFlow;
use std::ops::Range;
use std::sync::LazyLock;

use buck2_data::error::ErrorTag;
use object::Object;
use object::ObjectSegment;

use crate::daemon::client::connect::get_daemon_exe;

pub(crate) fn classify_server_stderr(
    error: buck2_error::Error,
    stderr: &str,
    disk_near_full: bool,
) -> buck2_error::Error {
    let mut tag = if stderr.is_empty() {
        None
    } else if stderr.contains("<jemalloc>: size mismatch detected") {
        // P1181704561
        Some(ErrorTag::ServerJemallocAssert)
    } else if stderr.contains("panicked at") {
        // Sample output of `buck2 debug crash`: P1159041719
        Some(ErrorTag::ServerPanicked)
    } else if stderr.contains("has overflowed its stack") {
        // Stderr looks like this:
        // ```
        // thread 'buck2-dm' has overflowed its stack
        // ```
        Some(ErrorTag::ServerStackOverflow)
    } else if stderr.contains("Resource temporarily unavailable") {
        Some(ErrorTag::EAgain)
    } else if stderr.contains("Signal 11 (SIGSEGV)") {
        // P1180289404
        Some(ErrorTag::ServerSegv)
    } else if stderr.contains("Signal 15 (SIGTERM)") {
        Some(ErrorTag::ServerSigterm)
    } else if stderr.contains("Signal 6 (SIGABRT)") {
        Some(ErrorTag::ServerSigabrt)
    } else if stderr.contains("(SIGBUS)") {
        // Signal 7 or Signal 10 depending on OS
        Some(ErrorTag::ServerSigbus)
    } else {
        None
    };
    if tag.is_none() && error.has_tag(ErrorTag::ClientGrpcStream) {
        tag = Some(ErrorTag::DaemonDisconnect);
    }

    let mut tags = tag.into_iter().collect::<Vec<_>>();
    if tag == Some(ErrorTag::ServerSigbus)
        && sigbus_fault_address(stderr).is_some_and(is_daemon_executable_address)
    {
        tags.push(ErrorTag::ServerSigbusExecutable);
    }
    // On a full disk, a SIGBUS is most likely the kernel failing to back a page of a
    // memory-mapped file being written to.
    if tag == Some(ErrorTag::ServerSigbus) && disk_near_full {
        tags.push(ErrorTag::IoStorageFull);
    }

    let error = if let Some(trace) = extract_trace(stderr) {
        if tag != Some(ErrorTag::ServerSigterm) {
            if trace
                .stack_trace_lines
                .iter()
                .any(|line| line.contains("remote_execution"))
            {
                tags.push(ErrorTag::ReClientCrash);
            }

            // A trace with no frames (e.g. because writing it failed on a full disk) would give
            // unrelated crashes the same key. SIGBUS frames show which code touched a page that
            // couldn't be loaded, which is unrelated to the cause and gives nearly every crash its
            // own key; SIGBUS crashes are grouped by their tags instead.
            if trace.stack_trace_lines.is_empty() || tag == Some(ErrorTag::ServerSigbus) {
                error
            } else {
                error.string_tag(&format!("crash({})", trace.trace_key()))
            }
        } else if let Some(signal_line) = trace.signal_line {
            // Keep this because the PID that (might have) sent it could be useful.
            // *** Signal 15 (SIGTERM) (0x2b08100000ab5) received by PID 1762297 (pthread TID 0x7f6650339640) (linux TID 1762297) (maybe from PID 2741, UID 176257) (code: 0), stack trace: ***
            error.context(signal_line)
        } else {
            error
        }
    } else {
        error
    };

    error.tag(tags)
}

// *** Signal 7 (SIGBUS) (0x25921d78) received by PID 5126 (pthread TID 0xfff1e6ba2380) ...
static SIGBUS_FAULT_ADDRESS: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\(SIGBUS\) \(0x([0-9a-f]+)\)").unwrap());

fn sigbus_fault_address(stderr: &str) -> Option<u64> {
    let address = SIGBUS_FAULT_ADDRESS.captures(stderr)?.get(1)?.as_str();
    u64::from_str_radix(address, 16).ok()
}

fn is_daemon_executable_address(address: u64) -> bool {
    daemon_executable_segments()
        .is_some_and(|segments| segments.iter().any(|segment| segment.contains(&address)))
}

fn daemon_executable_segments() -> Option<Vec<Range<u64>>> {
    let file = File::open(get_daemon_exe().ok()?).ok()?;
    let cache = object::ReadCache::new(file);
    executable_segments(&object::File::parse(&cache).ok()?)
}

/// Address ranges the executable's segments are loaded at. `None` for position-independent
/// executables, whose load address is only known at runtime, and for non-ELF binaries.
fn executable_segments<'data, R: object::ReadRef<'data>>(
    file: &object::File<'data, R>,
) -> Option<Vec<Range<u64>>> {
    if file.format() != object::BinaryFormat::Elf || file.kind() != object::ObjectKind::Executable {
        return None;
    }
    Some(
        file.segments()
            .map(|segment| segment.address()..segment.address() + segment.size())
            .collect(),
    )
}

//    0: rust_begin_unwind
//       at ./xplat/rust/toolchain/sysroot/1.80.1/library/std/src/panicking.rs:652:5
//    1: <buck2_server::daemon::server::BuckdServer as buck2_cli_proto::daemon_api_server::DaemonApi>::unstable_crash::{closure#0}
//       at ./fbcode/buck2/app/buck2_server/src/daemon/crash.rs:18:13
static RUST_STACK_FRAME: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\s*\d*:\s*(.*)$").unwrap());
static RUST_CONTEXT: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\s*at \S*$").unwrap());

fn extract_rust_frame(line: &str) -> ControlFlow<(), Option<String>> {
    if let Some(capture) = RUST_STACK_FRAME
        .captures(line)
        .and_then(|captures| captures.get(1))
    {
        ControlFlow::Continue(Some(capture.as_str().to_owned()))
    } else if RUST_CONTEXT.is_match(line) {
        ControlFlow::Continue(None)
    } else {
        ControlFlow::Break(())
    }
}

// 0   buck2                0x0000000107f548c0 _ZN5folly10symbolizer17getStackTraceSafeEPmm + 12
static FOLLY_MAC_STACK_FRAME: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\d*\s*\S*\s*0x\w*\s*(\S*) \+ \d*$").unwrap());

// @ 000000000004455f (unknown)
//                    /home/engshare/third-party2/glibc/2.34/src/glibc-2.34/signal/../sysdeps/unix/sysv/linux/libc_sigaction.c:8
//                    -> /home/engshare/third-party2/glibc/2.34/src/glibc-2.34/signal/../sysdeps/unix/sysv/linux/x86_64/libc_sigaction.c
static FOLLY_LINUX_STACK_FRAME: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\s*@\s*\w*\s*(.*)$").unwrap());
static FOLLY_LINUX_CONTEXT: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\s*(-> )?.*$").unwrap());

fn extract_folly_frame(line: &str) -> ControlFlow<(), Option<String>> {
    if let Some(capture) = FOLLY_MAC_STACK_FRAME
        .captures(line)
        .and_then(|captures| captures.get(1))
    {
        ControlFlow::Continue(Some(capture.as_str().to_owned()))
    } else if let Some(capture) = FOLLY_LINUX_STACK_FRAME
        .captures(line)
        .and_then(|captures| captures.get(1))
    {
        ControlFlow::Continue(Some(capture.as_str().to_owned()))
    } else if FOLLY_LINUX_CONTEXT.is_match(line) {
        ControlFlow::Continue(None)
    } else {
        ControlFlow::Break(())
    }
}

enum TraceType {
    Rust,
    Folly,
}

const UNINTERESTING_SEGMENTS: [&str; 7] = [
    "rust_begin_unwind",
    "core::panicking::panic_fmt",
    "(unknown)",
    "folly::symbolizer",
    "folly10symbolizer",
    "__GI_",
    "_sigtramp",
];

struct StackTraceInfo {
    signal_line: Option<String>,
    stack_trace_lines: Vec<String>,
}

impl StackTraceInfo {
    fn sanitized_trace(self) -> Vec<String> {
        // Exclude some lines to reduce churn in trace keys.
        // Changes higher up the stack are less likely to be related to the crash.
        if self.stack_trace_lines.len() > 5 {
            // 'uninteresting' lines shouldn't cause churn but exclude them if we are shortening the stack
            // to get enough unique data.

            self.stack_trace_lines
                .into_iter()
                .filter(|line| !UNINTERESTING_SEGMENTS.iter().any(|s| line.contains(s)))
                .take(5)
                .collect()
        } else {
            self.stack_trace_lines.clone()
        }
    }

    fn trace_key(self) -> String {
        // blake3 just because the default rust hasher isn't intended to be stable.
        let mut hasher = blake3::Hasher::new();
        for s in self.sanitized_trace() {
            hasher.update(s.as_bytes());
        }
        let mut digest = hasher.finalize().to_string();
        // Truncate to keep category_key relatively short and readable.
        // This should be enough to avoid collisions since there should be very few unique crashes.
        digest.truncate(6);
        digest
    }
}

fn extract_trace(stderr: &str) -> Option<StackTraceInfo> {
    let mut stack_trace_lines: Vec<String> = vec![];
    let mut signal_line = None;
    let mut stack_trace_type = None;
    for line in stderr.split('\n') {
        if line.starts_with("*** Signal") {
            signal_line = Some(line.to_owned());
            stack_trace_type = Some(TraceType::Folly);
        } else if line.starts_with("stack backtrace:") {
            stack_trace_type = Some(TraceType::Rust);
        } else if let Some(ref trace_type) = stack_trace_type {
            let res = match trace_type {
                TraceType::Rust => extract_rust_frame(line),
                TraceType::Folly => extract_folly_frame(line),
            };
            match res {
                ControlFlow::Continue(Some(line)) => stack_trace_lines.push(line),
                ControlFlow::Continue(None) => (),
                ControlFlow::Break(_) => break,
            }
        }
    }

    stack_trace_type.map(|_| StackTraceInfo {
        signal_line,
        stack_trace_lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_trace_lines(trace: &str) -> Vec<String> {
        trace
            .split('\n')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect()
    }

    const SIGBUS_NO_FRAMES: &str = "
*** Aborted at 1792565221 (Unix time, try 'date -d @1792565221') ***
*** Signal 7 (SIGBUS) (0x7f90e726bcb0) received by PID 7166 (pthread TID 0x7f902d3ff640) (linux TID 7627) (code: nonexistent physical address), stack trace: ***
";

    const SIGBUS_WITH_FRAMES: &str = "
*** Signal 7 (SIGBUS) (0x25921d78) received by PID 5126 (pthread TID 0xfff1e6ba2380) (linux TID 6859) (code: nonexistent physical address), stack trace: ***
    @ 000000000ddb2c8b folly::symbolizer::(anonymous namespace)::signalHandler(int, siginfo_t*, void*)
    @ 000000000e5c7f3f folly::FunctionScheduler::runOneFunction(std::unique_lock<std::mutex>&)
";

    fn daemon_disconnect_error() -> buck2_error::Error {
        buck2_error::buck2_error!(ErrorTag::ClientGrpcStream, "broken pipe")
    }

    #[test]
    fn test_sigbus_without_frames_has_no_crash_key() {
        let error = classify_server_stderr(daemon_disconnect_error(), SIGBUS_NO_FRAMES, false);
        assert!(error.has_tag(ErrorTag::ServerSigbus));
        assert!(error.string_tags().is_empty());
    }

    #[test]
    fn test_sigbus_with_frames_has_no_crash_key() {
        let error = classify_server_stderr(daemon_disconnect_error(), SIGBUS_WITH_FRAMES, false);
        assert!(error.has_tag(ErrorTag::ServerSigbus));
        assert!(error.string_tags().is_empty());
    }

    #[test]
    fn test_segv_with_frames_has_crash_key() {
        let stderr = SIGBUS_WITH_FRAMES.replace("Signal 7 (SIGBUS)", "Signal 11 (SIGSEGV)");
        let error = classify_server_stderr(daemon_disconnect_error(), &stderr, false);
        assert!(error.has_tag(ErrorTag::ServerSegv));
        assert_eq!(error.string_tags().len(), 1);
        assert!(error.string_tags()[0].starts_with("crash("));
    }

    #[test]
    fn test_sigbus_on_full_disk() {
        let error = classify_server_stderr(daemon_disconnect_error(), SIGBUS_NO_FRAMES, true);
        assert!(error.has_tag(ErrorTag::ServerSigbus));
        assert!(error.has_tag(ErrorTag::IoStorageFull));
        assert_eq!(error.best_tag(), Some(ErrorTag::IoStorageFull));
    }

    #[test]
    fn test_sigbus_on_disk_not_full() {
        let error = classify_server_stderr(daemon_disconnect_error(), SIGBUS_NO_FRAMES, false);
        assert!(!error.has_tag(ErrorTag::IoStorageFull));
    }

    #[test]
    fn test_full_disk_only_tags_sigbus() {
        let stderr = "*** Signal 11 (SIGSEGV) (0x0) received by PID 1 (code: address not mapped), stack trace: ***\n";
        let error = classify_server_stderr(daemon_disconnect_error(), stderr, true);
        assert!(error.has_tag(ErrorTag::ServerSegv));
        assert!(!error.has_tag(ErrorTag::IoStorageFull));
    }

    #[test]
    fn test_sigbus_fault_address() {
        assert_eq!(sigbus_fault_address(SIGBUS_WITH_FRAMES), Some(0x25921d78));
        assert_eq!(sigbus_fault_address(SIGBUS_NO_FRAMES), Some(0x7f90e726bcb0));
        assert_eq!(sigbus_fault_address("panicked at"), None);
    }

    #[test]
    fn test_executable_segments_contain_own_code() {
        let data = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        let file = object::File::parse(&*data).unwrap();
        // Only non-position-independent ELF executables have fixed segment addresses.
        if let Some(segments) = executable_segments(&file) {
            let code_address =
                test_executable_segments_contain_own_code as *const () as usize as u64;
            assert!(
                segments
                    .iter()
                    .any(|segment| segment.contains(&code_address))
            );
        }
    }

    #[test]
    fn test_generated_stack_trace() {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let stderr = format!("stack backtrace:\n{backtrace}");
        assert!(extract_trace(&stderr).is_some());
    }

    #[test]
    fn test_rust_stack_trace_hash() {
        // from `buck2 debug crash panic`
        let panic_trace = "
stack backtrace:
   0: rust_begin_unwind
             at ./xplat/rust/toolchain/sysroot/1.80.1/library/std/src/panicking.rs:652:5
   1: core::panicking::panic_fmt
             at ./xplat/rust/toolchain/sysroot/1.80.1/library/core/src/panicking.rs:72:14
   2: <buck2_server::daemon::server::BuckdServer as buck2_cli_proto::daemon_api_server::DaemonApi>::unstable_crash::{closure#0}
             at ./fbcode/buck2/app/buck2_server/src/daemon/crash.rs:18:13
   3: <<buck2_cli_proto::daemon_api_server::DaemonApiServer<_> as tower_service::Service<http::request::Request<_>>>::call::Unstable_CrashSvc<buck2_server::daemon::server::BuckdServer> as tonic::server::service::UnaryService<buck2_cli_proto::UnstableCrashRequest>>::call::{closure#0}
             at ./xplat/rust/toolchain/sysroot/1.80.1/library/core/src/future/future.rs:123:9
   4: <buck2_cli_proto::daemon_api_server::DaemonApiServer<buck2_server::daemon::server::BuckdServer> as tower_service::Service<http::request::Request<hyper::body::body::Body>>>::call::{closure#20}
             at ./xplat/rust/toolchain/sysroot/1.80.1/library/core/src/future/future.rs:123:9
   5: <futures_util::future::future::map::Map<futures_util::future::try_future::into_future::IntoFuture<core::pin::Pin<alloc::boxed::Box<dyn core::future::future::Future<Output = core::result::Result<http::response::Response<http_body::combinators::box_body::UnsyncBoxBody<bytes::bytes::Bytes, tonic::status::Status>>, core::convert::Infallible>> + core::marker::Send>>>, futures_util::fns::MapOkFn<<tonic::transport::service::router::Routes>::add_service<buck2_test_proto::test_executor_server::TestExecutorServer<buck2_test_api::grpc::executor::Service<buck2_test_runner::executor::Buck2TestExecutor>>>::{closure#0}>> as core::future::future::Future>::poll
             at ./xplat/rust/toolchain/sysroot/1.80.1/library/core/src/future/future.rs:123:9
   6: <futures_util::future::future::map::Map<futures_util::future::try_future::into_future::IntoFuture<tower::util::map_response::MapResponseFuture<core::pin::Pin<alloc::boxed::Box<dyn core::future::future::Future<Output = core::result::Result<http::response::Response<http_body::combinators::box_body::UnsyncBoxBody<bytes::bytes::Bytes, tonic::status::Status>>, core::convert::Infallible>> + core::marker::Send>>, <tonic::transport::service::router::Routes>::add_service<buck2_forkserver_proto::forkserver_server::ForkserverServer<buck2_forkserver::unix::service::UnixForkserverService>>::{closure#0}>>, futures_util::fns::MapOkFn<<http::response::Response<http_body::combinators::box_body::UnsyncBoxBody<bytes::bytes::Bytes, axum_core::error::Error>> as axum_core::response::into_response::IntoResponse>::into_response>> as core::future::future::Future>::poll
             at ./third-party/rust/vendor/futures-util-0.3.30/src/lib.rs:91:13
   7: <tower::util::map_response::MapResponseFuture<tower::util::map_response::MapResponseFuture<core::pin::Pin<alloc::boxed::Box<dyn core::future::future::Future<Output = core::result::Result<http::response::Response<http_body::combinators::box_body::UnsyncBoxBody<bytes::bytes::Bytes, tonic::status::Status>>, core::convert::Infallible>> + core::marker::Send>>, <tonic::transport::service::router::Routes>::add_service<buck2_cli_proto::daemon_api_server::DaemonApiServer<buck2_server::daemon::server::BuckdServer>>::{closure#0}>, <http::response::Response<http_body::combinators::box_body::UnsyncBoxBody<bytes::bytes::Bytes, axum_core::error::Error>> as axum_core::response::into_response::IntoResponse>::into_response> as core::future::future::Future>::poll
             at ./third-party/rust/vendor/futures-util-0.3.30/src/lib.rs:91:13
   8: <tower::util::oneshot::Oneshot<tower::util::boxed_clone::BoxCloneService<http::request::Request<hyper::body::body::Body>, http::response::Response<http_body::combinators::box_body::UnsyncBoxBody<bytes::bytes::Bytes, axum_core::error::Error>>, core::convert::Infallible>, http::request::Request<hyper::body::body::Body>> as core::future::future::Future>::poll
             at ./xplat/rust/toolchain/sysroot/1.80.1/library/core/src/future/future.rs:123:9
             ";

        let panic_sanitized_trace = test_trace_lines("
        <buck2_server::daemon::server::BuckdServer as buck2_cli_proto::daemon_api_server::DaemonApi>::unstable_crash::{closure#0}
        <<buck2_cli_proto::daemon_api_server::DaemonApiServer<_> as tower_service::Service<http::request::Request<_>>>::call::Unstable_CrashSvc<buck2_server::daemon::server::BuckdServer> as tonic::server::service::UnaryService<buck2_cli_proto::UnstableCrashRequest>>::call::{closure#0}
        <buck2_cli_proto::daemon_api_server::DaemonApiServer<buck2_server::daemon::server::BuckdServer> as tower_service::Service<http::request::Request<hyper::body::body::Body>>>::call::{closure#20}
        <futures_util::future::future::map::Map<futures_util::future::try_future::into_future::IntoFuture<core::pin::Pin<alloc::boxed::Box<dyn core::future::future::Future<Output = core::result::Result<http::response::Response<http_body::combinators::box_body::UnsyncBoxBody<bytes::bytes::Bytes, tonic::status::Status>>, core::convert::Infallible>> + core::marker::Send>>>, futures_util::fns::MapOkFn<<tonic::transport::service::router::Routes>::add_service<buck2_test_proto::test_executor_server::TestExecutorServer<buck2_test_api::grpc::executor::Service<buck2_test_runner::executor::Buck2TestExecutor>>>::{closure#0}>> as core::future::future::Future>::poll
        <futures_util::future::future::map::Map<futures_util::future::try_future::into_future::IntoFuture<tower::util::map_response::MapResponseFuture<core::pin::Pin<alloc::boxed::Box<dyn core::future::future::Future<Output = core::result::Result<http::response::Response<http_body::combinators::box_body::UnsyncBoxBody<bytes::bytes::Bytes, tonic::status::Status>>, core::convert::Infallible>> + core::marker::Send>>, <tonic::transport::service::router::Routes>::add_service<buck2_forkserver_proto::forkserver_server::ForkserverServer<buck2_forkserver::unix::service::UnixForkserverService>>::{closure#0}>>, futures_util::fns::MapOkFn<<http::response::Response<http_body::combinators::box_body::UnsyncBoxBody<bytes::bytes::Bytes, axum_core::error::Error>> as axum_core::response::into_response::IntoResponse>::into_response>> as core::future::future::Future>::poll
        ");
        assert_eq!(
            extract_trace(panic_trace)
                .expect("no trace found")
                .sanitized_trace(),
            panic_sanitized_trace
        );
    }

    #[test]
    fn test_folly_stack_trace() {
        // from `buck2 debug crash abort`
        let linux_folly_trace = "
*** Aborted at 1724968759 (Unix time, try 'date -d @1724968759') ***
*** Signal 6 (SIGABRT) (0x261c500150b2c) received by PID 1379116 (pthread TID 0x7f211b600640) (linux TID 1379195) (maybe from PID 1379116, UID 156101) (code: -6), stack trace: ***
    @ 00000000081eca3f folly::symbolizer::(anonymous namespace)::signalHandler(int, siginfo_t*, void*)
                       ./fbcode/folly/debugging/symbolizer/SignalHandler.cpp:453
    @ 000000000004455f (unknown)
                       /home/engshare/third-party2/glibc/2.34/src/glibc-2.34/signal/../sysdeps/unix/sysv/linux/libc_sigaction.c:8
                       -> /home/engshare/third-party2/glibc/2.34/src/glibc-2.34/signal/../sysdeps/unix/sysv/linux/x86_64/libc_sigaction.c
    @ 000000000009c993 __GI___pthread_kill
                       /home/engshare/third-party2/glibc/2.34/src/glibc-2.34/nptl/pthread_kill.c:46
    @ 00000000000444ac __GI_raise
                       /home/engshare/third-party2/glibc/2.34/src/glibc-2.34/signal/../sysdeps/posix/raise.c:26
    @ 000000000002c432 __GI_abort
                       /home/engshare/third-party2/glibc/2.34/src/glibc-2.34/stdlib/abort.c:79
    @ 0000000007f217e8 std::sys::pal::unix::abort_internal
                       xplat/rust/toolchain/sysroot/1.80.1/library/std/src/sys/pal/unix/mod.rs:366
    @ 0000000007edad48 std::process::abort
                       xplat/rust/toolchain/sysroot/1.80.1/library/std/src/process.rs:2369
    @ 000000001875604b <buck2_server::daemon::server::BuckdServer as buck2_cli_proto::daemon_api_server::DaemonApi>::unstable_crash::{closure#0}
                       fbcode/buck2/app/buck2_server/src/daemon/crash.rs:27
    @ 0000000018e12834 <<buck2_cli_proto::daemon_api_server::DaemonApiServer<_> as tower_service::Service<http::request::Request<_>>>::call::Unstable_CrashSvc<buck2_server::daemon::server::BuckdServer> as tonic::server::service::UnaryService<buck2_cli_proto::UnstableCrashRequest>>::call::{closure#0}
                       xplat/rust/toolchain/sysroot/1.80.1/library/core/src/future/future.rs:123
    @ 0000000018dff24b <buck2_cli_proto::daemon_api_server::DaemonApiServer<buck2_server::daemon::server::BuckdServer> as tower_service::Service<http::request::Request<hyper::body::body::Body>>>::call::{closure#20}
                       xplat/rust/toolchain/sysroot/1.80.1/library/core/src/future/future.rs:123
    @ 0000000019a82766 <futures_util::future::future::map::Map<futures_util::future::try_future::into_future::IntoFuture<core::pin::Pin<alloc::boxed::Box<dyn core::future::future::Future<Output = core::result::Result<http::response::Response<http_body::combinators::box_body::UnsyncBoxBody<bytes::bytes::Bytes, tonic::status::Status>>, core::convert::Infallible>> + core::marker::Send>>>, futures_util::fns::MapOkFn<<tonic::transport::service::router::Routes>::add_service<buck2_cli_proto::daemon_api_server::DaemonApiServer<buck2_server::daemon::server::BuckdServer>>::{closure#0}>> as core::future::future::Future>::poll
                       xplat/rust/toolchain/sysroot/1.80.1/library/core/src/future/future.rs:123
    ";

        let linux_sanitized_trace = test_trace_lines("
        std::sys::pal::unix::abort_internal
        std::process::abort
        <buck2_server::daemon::server::BuckdServer as buck2_cli_proto::daemon_api_server::DaemonApi>::unstable_crash::{closure#0}
        <<buck2_cli_proto::daemon_api_server::DaemonApiServer<_> as tower_service::Service<http::request::Request<_>>>::call::Unstable_CrashSvc<buck2_server::daemon::server::BuckdServer> as tonic::server::service::UnaryService<buck2_cli_proto::UnstableCrashRequest>>::call::{closure#0}
        <buck2_cli_proto::daemon_api_server::DaemonApiServer<buck2_server::daemon::server::BuckdServer> as tower_service::Service<http::request::Request<hyper::body::body::Body>>>::call::{closure#20}
        ");
        assert_eq!(
            extract_trace(linux_folly_trace)
                .expect("no trace found")
                .sanitized_trace(),
            linux_sanitized_trace
        );

        let mac_folly_trace = "
*** Aborted at 1709157873 (Unix time, try 'date -d @1709157873') ***
*** Signal 11 (SIGSEGV) (0x0) received by PID 43312 (pthread TID 0x9026bf000) (code: invalid permissions for mapped object), stack trace: ***
0   buck2                               0x0000000108c7b940 _ZN5folly10symbolizer17getStackTraceSafeEPmm + 12
1   buck2                               0x0000000108c6f8b8 _ZN5folly10symbolizer21SafeStackTracePrinter15printStackTraceEb + 72
2   buck2                               0x000000010a55f0f8 _ZN5folly10symbolizer12_GLOBAL__N_113signalHandlerEiP9__siginfoPv + 1536
3   libsystem_platform.dylib            0x00000001810c5a24 _sigtramp + 56
4   buck2                               0x0000000109ff94c4 _ZN8facebook16remote_execution3cas18ManifoldHttpClient14co_materializeENSt3__110shared_ptrINS1_14CASCallContextEEENS4_INS1_20IDigestStatusTrackerEEENS3_6vectorINS3_4pairIN5build5bazel6remote9execution2v26DigestENS4_INS1_11FileWrapperEEEEENS3_9allocatorISJ_EEEENS0_6crypto8HashAlgoE.resume + 648
5   buck2                               0x0000000109ff54e4 _ZZN5folly4coro18collectAllTryRangeINS0_6detail9MoveRangeINSt3__16vectorINS0_16TaskWithExecutorIvEENS4_9allocatorIS7_EEEEEEEENS0_4TaskINS5_INS_3TryINS2_22decay_rvalue_referenceINS2_21lift_lvalue_referenceINS0_12await_resultIDTclL_ZNS0_11folly_cpo__13co_viaIfAsyncEEclL_ZNS4_7declvalINS_8Executor9KeepAliveISJ_EEEEDTcl9__declvalIT_ELi0EEEvEEclsr3stdE7declvalINS4_15iterator_traitsIDTclL_ZNS_6access5beginEEclsr3stdE7declvalIRSM_EEEEE9referenceEEEEEvE4typeEE4typeEE4typeEEENS8_IS11_EEEEEESM_ENKUlS7_mE_clES7_m.resume + 560
6   buck2                               0x0000000108c7b588 _ZN5folly36resumeCoroutineWithNewAsyncStackRootENSt12experimental13coroutines_v116coroutine_handleIvEERNS_15AsyncStackFrameE + 84
7   buck2                               0x0000000108c4edd4 _ZN5folly6detail8function14FunctionTraitsIFvvEE9callSmallIZNS_4coro6detail23ViaCoroutinePromiseBase20scheduleContinuationEvEUlvE_EEvRNS1_4DataE + 48
8   buck2                               0x0000000108caabdc _ZN5folly18ThreadPoolExecutor7runTaskERKNSt3__110shared_ptrINS0_6ThreadEEEONS0_4TaskE + 304
9   buck2                               0x0000000108e129f4 _ZN5folly21CPUThreadPoolExecutor9threadRunENSt3__110shared_ptrINS_18ThreadPoolExecutor6ThreadEEE + 484
(safe mode, symbolizer not available)
    ";

        let mac_sanitized_trace = test_trace_lines("
        _ZN8facebook16remote_execution3cas18ManifoldHttpClient14co_materializeENSt3__110shared_ptrINS1_14CASCallContextEEENS4_INS1_20IDigestStatusTrackerEEENS3_6vectorINS3_4pairIN5build5bazel6remote9execution2v26DigestENS4_INS1_11FileWrapperEEEEENS3_9allocatorISJ_EEEENS0_6crypto8HashAlgoE.resume
        _ZZN5folly4coro18collectAllTryRangeINS0_6detail9MoveRangeINSt3__16vectorINS0_16TaskWithExecutorIvEENS4_9allocatorIS7_EEEEEEEENS0_4TaskINS5_INS_3TryINS2_22decay_rvalue_referenceINS2_21lift_lvalue_referenceINS0_12await_resultIDTclL_ZNS0_11folly_cpo__13co_viaIfAsyncEEclL_ZNS4_7declvalINS_8Executor9KeepAliveISJ_EEEEDTcl9__declvalIT_ELi0EEEvEEclsr3stdE7declvalINS4_15iterator_traitsIDTclL_ZNS_6access5beginEEclsr3stdE7declvalIRSM_EEEEE9referenceEEEEEvE4typeEE4typeEE4typeEEENS8_IS11_EEEEEESM_ENKUlS7_mE_clES7_m.resume
        _ZN5folly36resumeCoroutineWithNewAsyncStackRootENSt12experimental13coroutines_v116coroutine_handleIvEERNS_15AsyncStackFrameE
        _ZN5folly6detail8function14FunctionTraitsIFvvEE9callSmallIZNS_4coro6detail23ViaCoroutinePromiseBase20scheduleContinuationEvEUlvE_EEvRNS1_4DataE
        _ZN5folly18ThreadPoolExecutor7runTaskERKNSt3__110shared_ptrINS0_6ThreadEEEONS0_4TaskE
        ");

        assert_eq!(
            extract_trace(mac_folly_trace)
                .expect("no trace found")
                .sanitized_trace(),
            mac_sanitized_trace
        );
    }
}
