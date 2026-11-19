/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![allow(dead_code)]

use std::ffi::c_void;
use std::mem;
use std::ptr;
use std::sync::Arc;

use buck2_error::BuckErrorContext;
use buck2_wrapper_common::win::winapi_handle::WinapiHandle;
use dupe::Dupe;
use windows_sys::Win32::Foundation::FALSE;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::System::IO::CreateIoCompletionPort;
use windows_sys::Win32::System::IO::GetQueuedCompletionStatus;
use windows_sys::Win32::System::IO::OVERLAPPED;
use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
use windows_sys::Win32::System::JobObjects::CreateJobObjectW;
use windows_sys::Win32::System::JobObjects::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
use windows_sys::Win32::System::JobObjects::JOBOBJECT_ASSOCIATE_COMPLETION_PORT;
use windows_sys::Win32::System::JobObjects::JOBOBJECT_BASIC_ACCOUNTING_INFORMATION;
use windows_sys::Win32::System::JobObjects::JOBOBJECT_EXTENDED_LIMIT_INFORMATION;
use windows_sys::Win32::System::JobObjects::JobObjectAssociateCompletionPortInformation;
use windows_sys::Win32::System::JobObjects::JobObjectBasicAccountingInformation;
use windows_sys::Win32::System::JobObjects::JobObjectExtendedLimitInformation;
use windows_sys::Win32::System::JobObjects::QueryInformationJobObject;
use windows_sys::Win32::System::JobObjects::SetInformationJobObject;
use windows_sys::Win32::System::JobObjects::TerminateJobObject;
use windows_sys::Win32::System::SystemServices::JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO;
use windows_sys::Win32::System::Threading::INFINITE;

use crate::win::utils::result_bool;

pub(crate) struct JobObject {
    job_handle: Arc<WinapiHandle>,
    completion_handle: Arc<WinapiHandle>,
}

impl JobObject {
    pub(crate) fn new() -> buck2_error::Result<Self> {
        let job_handle = unsafe {
            WinapiHandle::new_check_last_os_error(CreateJobObjectW(ptr::null(), ptr::null()))
                .buck_error_context("CreateJobObject")?
        };

        let completion_handle = unsafe {
            WinapiHandle::new_check_last_os_error(CreateIoCompletionPort(
                INVALID_HANDLE_VALUE, // FileHandle
                ptr::null_mut(),      // ExistingCompletionPort
                0,                    // CompletionKey
                1,                    // NumberOfConcurrentThreads
            ))
            .buck_error_context("CreateIoCompletionPort")?
        };

        associate_job_with_completion_port(&job_handle, &completion_handle)?;
        set_job_limits(&job_handle)?;

        Ok(Self {
            job_handle: Arc::new(job_handle),
            completion_handle: Arc::new(completion_handle),
        })
    }

    pub(crate) fn assign_process(&self, process: HANDLE) -> buck2_error::Result<()> {
        result_bool(unsafe { AssignProcessToJobObject(self.job_handle.handle(), process) })
    }

    pub(crate) async fn terminate(&self, exit_code: u32) -> buck2_error::Result<()> {
        result_bool(unsafe { TerminateJobObject(self.job_handle.handle(), exit_code) })?;
        self.wait().await
    }

    // waits until all processes in a job have exited
    // https://devblogs.microsoft.com/oldnewthing/20130405-00/?p=4743
    async fn wait(&self) -> buck2_error::Result<()> {
        const MAX_RETRY_ATTEMPT: usize = 10;
        let job = self.job_handle.dupe();
        let completion_port = self.completion_handle.dupe();

        // try to wait all the processes exit before spawn a blocking task
        for _ in 0..MAX_RETRY_ATTEMPT {
            if let Ok(false) = has_active_processes(&job, &completion_port, 0) {
                // ACTIVE_PROCESS_ZERO is posted once, and this poll just consumed it.
                return Ok(());
            }
        }

        tokio::task::spawn_blocking(move || {
            let completion_port = completion_port;
            // Job notifications are best-effort (delivery is not guaranteed), so a
            // lost ACTIVE_PROCESS_ZERO must not block forever; poll the count too.
            while has_active_processes(&job, &completion_port, ACTIVE_PROCESS_POLL_MS)? {
                if active_process_count(&job)? == 0 {
                    break;
                }
            }
            Ok(())
        })
        .await?
    }
}

const ACTIVE_PROCESS_POLL_MS: u32 = 100;

fn active_process_count(job: &WinapiHandle) -> buck2_error::Result<u32> {
    // SAFETY: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION is a plain C struct of integers;
    // all-zeros is a valid representation. windows-sys structs have no Default impl.
    let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { mem::zeroed() };
    // SAFETY: `job` borrows a live job handle; `info` is a writable buffer of the type and
    // size this info class expects; the optional `lpReturnLength` out-pointer may be null.
    result_bool(unsafe {
        QueryInformationJobObject(
            job.handle(),
            JobObjectBasicAccountingInformation,
            &mut info as *mut _ as *mut c_void,
            mem::size_of_val(&info)
                .try_into()
                .expect("cannot safely cast to u32"),
            ptr::null_mut(),
        )
    })?;
    Ok(info.ActiveProcesses)
}

fn has_active_processes(
    job: &WinapiHandle,
    completion_port: &WinapiHandle,
    timeout: u32,
) -> buck2_error::Result<bool> {
    let mut completion_code: u32 = 0;
    let mut completion_key: usize = 0;
    let mut overlapped = mem::MaybeUninit::<OVERLAPPED>::uninit();
    let mut lp_overlapped = overlapped.as_mut_ptr();

    let result = unsafe {
        GetQueuedCompletionStatus(
            completion_port.handle(),
            &mut completion_code,
            &mut completion_key,
            &mut lp_overlapped,
            timeout,
        )
    };

    // ignore timeout errors unless the timeout was specified to INFINITE
    // https://docs.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-getqueuedcompletionstatus
    if timeout != INFINITE && result == FALSE && lp_overlapped.is_null() {
        return Ok(true);
    }

    result_bool(result)?;

    // we are interested only in the specific event from the job object
    // ignore the rest in case some other I/O gets queued to our completion port
    if completion_key != job.handle() as usize
        || completion_code != JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO
    {
        return Ok(true);
    }

    Ok(false)
}

fn associate_job_with_completion_port(
    job: &WinapiHandle,
    completion_port: &WinapiHandle,
) -> buck2_error::Result<()> {
    let mut associate_completion = JOBOBJECT_ASSOCIATE_COMPLETION_PORT {
        CompletionKey: job.handle(),
        CompletionPort: completion_port.handle(),
    };

    result_bool(unsafe {
        SetInformationJobObject(
            job.handle(),
            JobObjectAssociateCompletionPortInformation,
            &mut associate_completion as *mut _ as *const c_void,
            mem::size_of_val(&associate_completion)
                .try_into()
                .expect("cannot safely cast to u32"),
        )
    })
}

fn set_job_limits(job: &WinapiHandle) -> buck2_error::Result<()> {
    // SAFETY: JOBOBJECT_EXTENDED_LIMIT_INFORMATION is a plain C struct of integers/pointers;
    // all-zeros is a valid representation. windows-sys structs have no Default impl.
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

    result_bool(unsafe {
        SetInformationJobObject(
            job.handle(),
            JobObjectExtendedLimitInformation,
            &mut info as *mut _ as *const c_void,
            mem::size_of_val(&info)
                .try_into()
                .expect("cannot safely cast to u32"),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::os::windows::io::AsRawHandle;
    use std::thread;
    use std::time::Duration;

    use buck2_util::process::background_command;

    use super::*;

    /// Returns a job whose only process has been terminated and has exited.
    fn job_with_exited_process() -> JobObject {
        let job = JobObject::new().unwrap();
        let mut child = background_command("ping")
            .args(["-n", "30", "127.0.0.1"])
            .spawn()
            .unwrap();
        job.assign_process(child.as_raw_handle() as HANDLE).unwrap();
        // SAFETY: `job` is owned here, so its handle is live; `CreateJobObjectW` returns
        // a handle with `JOB_OBJECT_ALL_ACCESS`, which includes `JOB_OBJECT_TERMINATE`.
        result_bool(unsafe { TerminateJobObject(job.job_handle.handle(), 1) }).unwrap();
        child.wait().unwrap();
        // Give the kernel time to post the job's notifications to the port.
        thread::sleep(Duration::from_millis(500));
        job
    }

    /// Runs `job.wait()` with a timeout. The runtime is shut down with a timeout
    /// too, so a blocking thread stuck in `wait()` fails the test instead of
    /// hanging it.
    fn wait_with_timeout(job: &JobObject) -> bool {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let finished = rt
            .block_on(async { tokio::time::timeout(Duration::from_secs(5), job.wait()).await })
            .is_ok();
        rt.shutdown_timeout(Duration::from_secs(1));
        finished
    }

    #[test]
    fn test_wait_returns_when_processes_exited_before_wait() {
        let job = job_with_exited_process();
        assert!(wait_with_timeout(&job), "`wait` did not return");
    }

    #[test]
    fn test_wait_returns_without_active_process_zero() {
        let job = job_with_exited_process();
        let mut drained = false;
        for _ in 0..100 {
            if !has_active_processes(&job.job_handle, &job.completion_handle, 0).unwrap() {
                drained = true;
                break;
            }
        }
        assert!(drained, "ACTIVE_PROCESS_ZERO was never posted");
        assert!(wait_with_timeout(&job), "`wait` did not return");
    }
}
