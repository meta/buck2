/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#[cfg(target_os = "macos")]
use buck2_error::conversion::from_any_with_tag;
use buck2_settings::MacosQosClass;

/// The `libc::qos_class_t` for a class the daemon can be spawned with. Higher
/// classes (`user_interactive`, `user_initiated`) and the sentinel values
/// (`default`, `unspecified`) are rejected by `posix_spawnattr_set_qos_class_np`
/// with `EINVAL`, which is why the setting only offers these two.
#[cfg(target_os = "macos")]
fn to_qos_class_t(qos_class: MacosQosClass) -> buck2_error::Result<libc::qos_class_t> {
    match qos_class {
        MacosQosClass::Utility => Ok(libc::qos_class_t::QOS_CLASS_UTILITY),
        MacosQosClass::Background => Ok(libc::qos_class_t::QOS_CLASS_BACKGROUND),
        MacosQosClass::SkipLowering => Err(buck2_error::internal_error!(
            "SkipLowering should not reach do_lower_priority"
        )),
    }
}

pub(crate) fn daemon_lower_priority(
    skip_macos_qos_flag: bool,
    macos_qos_class: Option<MacosQosClass>,
) -> buck2_error::Result<()> {
    if skip_macos_qos_flag {
        // Either:
        // * we already lowered priority or
        // * we are running in-process daemon.
        return Ok(());
    }

    let qos_class = macos_qos_class.unwrap_or(MacosQosClass::Utility);

    match qos_class {
        MacosQosClass::SkipLowering => {}
        MacosQosClass::Utility | MacosQosClass::Background => {
            #[cfg(target_os = "macos")]
            {
                do_lower_priority(qos_class)?;
            }
        }
    }
    Ok(())
}

/// On macOS we lower priority by restarting the daemon with QoS class = utility.
///
/// When a program is launched from command line, at least from iTerm2,
/// macOS seems to think it is interactive, so it gives it high priority.
/// buck2 daemon spawns processes like compilers which do not really need high priority.
/// When compilers run with high priority, they starve other processes.
/// Practically it results in very large ping and VPN disconnects on my machine.
///
/// Ideally instead of restarting the daemon we should just start the daemon with proper QoS class,
/// but rust `Command` API does not provide access to `posix_spawnattr_t`,
/// and `Command` provides convenient API to capture stdout/stderr.
///
/// This function never return `Ok`.
#[cfg(target_os = "macos")]
fn do_lower_priority(qos_class: MacosQosClass) -> buck2_error::Result<()> {
    use std::env;
    use std::ffi::CString;
    use std::io;
    use std::iter;
    use std::mem::MaybeUninit;
    use std::os::unix::ffi::OsStrExt;
    use std::ptr;

    use buck2_error::BuckErrorContext;

    unsafe extern "C" {
        // https://github.com/rust-lang/libc/pull/3128
        pub fn posix_spawnattr_set_qos_class_np(
            attr: *mut libc::posix_spawnattr_t,
            qos_class: libc::qos_class_t,
        ) -> libc::c_int;
        // `environ` macro expands to this function call.
        // https://github.com/rust-lang/rust/blob/07c993eba8b76eae497e98433ae075b00f01be10/library/std/src/sys/unix/os.rs#L493
        fn _NSGetEnviron() -> *mut *mut *mut libc::c_char;
    }

    let exe = env::current_exe()?;
    let exe = CString::new(exe.into_os_string().as_bytes())
        .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))?;

    struct Spawnattr(libc::posix_spawnattr_t);

    impl Drop for Spawnattr {
        fn drop(&mut self) {
            unsafe {
                let r = libc::posix_spawnattr_destroy(&mut self.0);
                assert_eq!(r, 0);
            }
        }
    }

    impl Spawnattr {
        fn new() -> buck2_error::Result<Spawnattr> {
            unsafe {
                let mut spawnattr = MaybeUninit::zeroed();
                let r = libc::posix_spawnattr_init(spawnattr.as_mut_ptr());
                if r != 0 {
                    return Err(io::Error::from_raw_os_error(r))
                        .buck_error_context("posix_spawnattr_init");
                }
                Ok(Spawnattr(spawnattr.assume_init()))
            }
        }
    }

    let mut spawnattr = Spawnattr::new()?;

    let mut argv: Vec<CString> = env::args()
        .map(|s| CString::new(s).map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0)))
        .chain(iter::once(Ok(CString::new("--skip-macos-qos").map_err(
            |e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0),
        )?)))
        .collect::<buck2_error::Result<Vec<_>>>()?;
    let argv: Vec<*mut libc::c_char> = argv
        .iter_mut()
        .map(|s| s.as_ptr() as *mut libc::c_char)
        .chain(iter::once(ptr::null_mut()))
        .collect();

    unsafe {
        // Apple-specific flag: `posix_spawn` behaves like `exec` instead of `fork+exec`.
        let r = libc::posix_spawnattr_setflags(
            &mut spawnattr.0,
            libc::POSIX_SPAWN_SETEXEC as libc::c_short,
        );
        if r != 0 {
            return Err(io::Error::from_raw_os_error(r))
                .buck_error_context("posix_spawnattr_setflags");
        }

        let r = posix_spawnattr_set_qos_class_np(&mut spawnattr.0, to_qos_class_t(qos_class)?);

        if r != 0 {
            return Err(io::Error::from_raw_os_error(r))
                .buck_error_context("posix_spawnattr_set_qos_class_np");
        }

        let environ = *_NSGetEnviron();
        let mut pid = MaybeUninit::zeroed();

        let r = libc::posix_spawnp(
            pid.as_mut_ptr(),
            exe.as_ptr(),
            ptr::null(),
            &spawnattr.0,
            argv.as_ptr(),
            environ,
        );
        if r != 0 {
            return Err(io::Error::from_raw_os_error(r)).buck_error_context("posix_spawnp");
        }
    }

    #[derive(Debug, buck2_error::Error)]
    #[error("`posix_spawnp` with `POSIX_SPAWN_SETEXEC` flag should not return on success.")]
    #[buck2(tag = Tier0)]
    struct Unreachable;

    Err(Unreachable.into())
}
