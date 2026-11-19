/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::ffi::OsStr;
use std::path::Path;

#[cfg(not(windows))]
pub(crate) fn spawn_background_process_on_windows<'a>(
    _working_dir: &Path,
    _exe: &Path,
    _args: impl IntoIterator<Item = &'a str>,
    _daemon_env_vars: &[(&OsStr, &OsStr)],
    _daemon_env_removals: &[&str],
) -> buck2_error::Result<()> {
    #[derive(Debug, buck2_error::Error)]
    #[error("not Windows")]
    #[buck2(tag = Tier0)]
    struct NotWindows;

    Err(NotWindows.into())
}

#[cfg(windows)]
pub(crate) fn spawn_background_process_on_windows<'a>(
    working_dir: &Path,
    exe: &Path,
    args: impl IntoIterator<Item = &'a str>,
    daemon_env_vars: &[(&OsStr, &OsStr)],
    daemon_env_removals: &[&str],
) -> buck2_error::Result<()> {
    use std::ffi::OsString;
    use std::ffi::c_void;
    use std::io;
    use std::mem;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    use buck2_error::BuckErrorContext;
    use buck2_error::buck2_error;
    use buck2_hash::BuckMutMap;
    use buck2_util::os::win::os_str::os_str_to_wide_null_term;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Foundation::FALSE;
    use windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;
    use windows_sys::Win32::System::Threading::CREATE_UNICODE_ENVIRONMENT;
    use windows_sys::Win32::System::Threading::CreateProcessW;
    use windows_sys::Win32::System::Threading::DETACHED_PROCESS;
    use windows_sys::Win32::System::Threading::PROCESS_INFORMATION;
    use windows_sys::Win32::System::Threading::STARTUPINFOW;

    // We have to call CreateProcessW manually because std::process::Command
    // doesn't allow to set 'bInheritHandles' to false. Without this waiting on
    // parent process will also wait on inherited handles of daemon process.

    // Translated from ArgvQuote at http://tinyurl.com/zmgtnls
    fn append_quoted(arg: &OsStr, cmdline: &mut Vec<u16>) {
        if !arg.is_empty()
            && !arg.encode_wide().any(|c| {
                c == ' ' as u16
                    || c == '\t' as u16
                    || c == '\n' as u16
                    || c == '\x0b' as u16
                    || c == '\"' as u16
            })
        {
            cmdline.extend(arg.encode_wide());
            return;
        }
        cmdline.push('"' as u16);

        let arg: Vec<_> = arg.encode_wide().collect();
        let mut i = 0;
        while i < arg.len() {
            let mut num_backslashes = 0;
            while i < arg.len() && arg[i] == '\\' as u16 {
                i += 1;
                num_backslashes += 1;
            }

            if i == arg.len() {
                for _ in 0..num_backslashes * 2 {
                    cmdline.push('\\' as u16);
                }
                break;
            } else if arg[i] == b'"' as u16 {
                for _ in 0..num_backslashes * 2 + 1 {
                    cmdline.push('\\' as u16);
                }
                cmdline.push(arg[i]);
            } else {
                for _ in 0..num_backslashes {
                    cmdline.push('\\' as u16);
                }
                cmdline.push(arg[i]);
            }
            i += 1;
        }
        cmdline.push('"' as u16);
    }

    #[allow(clippy::vec_init_then_push)]
    fn make_command_line<'a>(program: &OsStr, args: impl IntoIterator<Item = &'a str>) -> Vec<u16> {
        let mut cmd: Vec<u16> = Vec::new();
        cmd.push(b'"' as u16);
        cmd.extend(program.encode_wide());
        cmd.push(b'"' as u16);
        for arg in args.into_iter().map(OsString::from) {
            cmd.push(b' ' as u16);
            append_quoted(&arg, &mut cmd);
        }
        cmd.push(0); // null terminator
        cmd
    }

    // Inspiration from rust stdlib at `std/src/sys/windows/process.rs`
    fn ensure_no_nuls(s: &OsStr) -> buck2_error::Result<&OsStr> {
        if s.encode_wide().any(|b| b == 0) {
            Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "{}",
                format!("null byte found in provided data: {s:?}")
            ))
        } else {
            Ok(s)
        }
    }

    fn make_envp(
        extra_env_vars: &[(&OsStr, &OsStr)],
        removed_env_vars: &[&str],
    ) -> buck2_error::Result<(*mut c_void, Box<[u16]>)> {
        if extra_env_vars.is_empty() && removed_env_vars.is_empty() {
            Ok((ptr::null_mut(), Box::new([])))
        } else {
            let mut env: BuckMutMap<_, _> = std::env::vars_os().collect();
            for (key, val) in extra_env_vars.iter() {
                env.insert(OsString::from(key), OsString::from(val));
            }
            env.retain(|key, _| !removed_env_vars.iter().any(|r| key.eq_ignore_ascii_case(r)));
            // On Windows we pass an "environment block" which is not a char**, but
            // rather a concatenation of null-terminated k=v\0 sequences, with a final
            // \0 to terminate.
            let mut blk = Vec::new();

            for (k, v) in env.into_iter() {
                blk.extend(
                    ensure_no_nuls(&k)
                        .with_buck_error_context(|| format!("Reading environment variable {k:?}"))?
                        .encode_wide(),
                );
                blk.push('=' as u16);
                blk.extend(
                    ensure_no_nuls(&v)
                        .with_buck_error_context(|| {
                            format!("Reading value {v:?} of environment variable {k:?}")
                        })?
                        .encode_wide(),
                );
                blk.push(0);
            }
            blk.push(0);

            let mut blk = blk.into_boxed_slice();
            Ok((blk.as_mut_ptr() as *mut c_void, blk))
        }
    }

    let program = exe.as_os_str();
    let cwd = working_dir.as_os_str();

    let mut cmd = make_command_line(program, args);

    let mut sinfo: STARTUPINFOW = unsafe { mem::zeroed() };
    sinfo.cb = mem::size_of::<STARTUPINFOW>() as u32;
    let mut pinfo: PROCESS_INFORMATION = unsafe { mem::zeroed() };
    let creation_flags = CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_UNICODE_ENVIRONMENT;

    let (envp, _data) = make_envp(daemon_env_vars, daemon_env_removals)?;

    let status = unsafe {
        CreateProcessW(
            os_str_to_wide_null_term(program).as_ptr(), // lpApplicationName
            cmd.as_mut_ptr(),                           // lpCommandLine
            ptr::null(),                                // lpProcessAttributes
            ptr::null(),                                // lpThreadAttributes
            FALSE,                                      // bInheritHandles
            creation_flags,                             // dwCreationFlags
            envp,                                       // lpEnvironment
            os_str_to_wide_null_term(cwd).as_ptr(),     // lpCurrentDirectory
            &sinfo,
            &mut pinfo,
        )
    };
    if status == 0 {
        return Err(buck2_error::Error::from(io::Error::last_os_error()));
    }
    unsafe { CloseHandle(pinfo.hThread) };
    unsafe { CloseHandle(pinfo.hProcess) };
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::path::PathBuf;

    use crate::daemon::daemon_windows::spawn_background_process_on_windows;

    #[test]
    fn test_smoke() {
        if !cfg!(windows) {
            return;
        }

        // We need absolute path to `cmd.exe`.
        let cmd_exe_path = PathBuf::from(env::var("COMSPEC").expect("COMSPEC variable not set"));

        // TODO(nga): check it actually spawns a process.
        spawn_background_process_on_windows(
            &env::current_dir().unwrap(),
            &cmd_exe_path,
            ["/c", "echo test"],
            [].as_slice(),
            [].as_slice(),
        )
        .unwrap();
    }
}
