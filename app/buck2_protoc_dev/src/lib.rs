/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::env;
use std::ffi::OsString;
use std::io;
use std::path::Path;
#[cfg(not(buck_build))]
use std::path::PathBuf;

fn get_env(key: &str) -> Option<OsString> {
    println!("cargo:rerun-if-env-changed={key}");
    env::var_os(key)
}

#[cfg(not(buck_build))]
unsafe fn set_var(
    var: &str,
    override_var: &str,
    path: Result<PathBuf, protoc_bin_vendored::Error>,
) {
    let path = if let Some(override_var_value) = env::var_os(override_var) {
        eprintln!("INFO: Variable ${var} is overridden by ${override_var}");
        PathBuf::from(override_var_value)
    } else {
        match path {
            Err(e) => {
                panic!("{var} not available for platform {e:?}, set ${override_var} to override")
            }
            Ok(path) => {
                assert!(path.exists(), "Path does not exist: `{}`", path.display());
                path
            }
        }
    };

    let path = dunce::canonicalize(path).expect("Failed to canonicalize path");
    eprintln!("INFO: Variable ${var} set to {path:?}");
    unsafe { env::set_var(var, path) };
}

/// Set up $PROTOC to point to the in repo binary if available.
///
/// Note: repo root is expected to be a relative or absolute path to the root of the repository.
unsafe fn maybe_set_protoc() {
    #[cfg(not(buck_build))]
    {
        // `cargo build` of `buck2` does not require external `protoc` dependency
        // because it uses prebuilt bundled `protoc` binary from `protoc-bin-vendored` crate.
        // However, prebuilt `protoc` binaries do not work in NixOS builds, see
        // https://github.com/facebook/buck2/issues/65
        // So for NixOS builds path to `protoc` binary can be overridden with
        // `BUCK2_BUILD_PROTOC` environment variable.
        unsafe {
            set_var(
                "PROTOC",
                "BUCK2_BUILD_PROTOC",
                protoc_bin_vendored::protoc_bin_path(),
            );
        }
    }
}

/// Set $PROTOC_INCLUDE.
unsafe fn maybe_set_protoc_include() {
    #[cfg(not(buck_build))]
    {
        unsafe {
            set_var(
                "PROTOC_INCLUDE",
                "BUCK2_BUILD_PROTOC_INCLUDE",
                protoc_bin_vendored::include_path(),
            );
        }
    }
}

pub struct Builder {
    tonic: tonic_prost_build::Builder,
}

pub fn configure() -> Builder {
    let tonic = tonic_prost_build::configure();
    // We want to use optional everywhere
    let tonic = tonic.protoc_arg("--experimental_allow_proto3_optional");

    Builder { tonic }
}

impl Builder {
    pub fn type_attribute<P: AsRef<str>, A: AsRef<str>>(self, path: P, attribute: A) -> Self {
        Self {
            tonic: self.tonic.type_attribute(path, attribute),
        }
    }

    pub fn field_attribute<P: AsRef<str>, A: AsRef<str>>(self, path: P, attribute: A) -> Self {
        Self {
            tonic: self.tonic.field_attribute(path, attribute),
        }
    }

    pub fn extern_path(self, proto_path: impl AsRef<str>, rust_path: impl AsRef<str>) -> Self {
        Self {
            tonic: self.tonic.extern_path(proto_path, rust_path),
        }
    }

    pub fn boxed<P: AsRef<str>>(self, path: P) -> Self {
        Self {
            tonic: self.tonic.boxed(path),
        }
    }

    /// Sets up the PROTOC and PROTOC_INCLUDE environment variables.
    ///
    /// # Safety
    ///
    /// This function calls `std::env::set_var` which is unsafe due to potential data races
    /// when environment variables are read concurrently from other threads. The caller must
    /// ensure that no other threads are reading environment variables while this function
    /// is executing, or that such concurrent access is properly synchronized.
    pub unsafe fn setup_protoc(self) -> Self {
        // It would be great if there were on the config rather than an env variables...
        unsafe { maybe_set_protoc() };
        unsafe { maybe_set_protoc_include() };
        self
    }

    pub fn compile<P>(self, protos: &[P], includes: &[P]) -> io::Result<()>
    where
        P: AsRef<Path>,
    {
        let Self { mut tonic } = self;

        // Buck likes to set $OUT in a genrule, while Cargo likes to set $OUT_DIR.
        // If we have $OUT set only, move it into the config
        if get_env("OUT_DIR").is_none() {
            if let Some(out) = get_env("OUT") {
                tonic = tonic.out_dir(out);
            }
        }

        // Tell Cargo that if the given file changes, to rerun this build script.
        for proto_file in protos {
            println!("cargo:rerun-if-changed={}", proto_file.as_ref().display());
        }

        tonic.compile_protos(protos, includes)
    }
}
