/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;

use crate::invocation_paths::InvocationPaths;
use crate::invocation_paths::TenantPaths;

/// Stable identity used to address one tenant within a shared daemon.
///
/// The invocation working directory is deliberately absent: commands from different directories
/// in the same project and isolation must resolve to the same tenant.
#[derive(Allocative, Clone, Debug, Eq, Hash, PartialEq)]
pub struct TenantKey {
    project_root: AbsNormPathBuf,
    isolation: FileNameBuf,
}

impl TenantKey {
    /// Derives a tenant key while excluding the invocation cwd.
    pub fn from_invocation_paths(paths: &InvocationPaths) -> Self {
        Self::from_tenant_paths(&paths.tenant_paths())
    }

    /// Derives a tenant key from stable tenant paths.
    pub fn from_tenant_paths(paths: &TenantPaths) -> Self {
        Self {
            project_root: paths.project_root().root().to_buf(),
            isolation: paths.isolation().to_owned(),
        }
    }

    /// Returns the project root component of the tenant address.
    pub fn project_root(&self) -> &AbsNormPath {
        &self.project_root
    }

    /// Returns the isolation component of the tenant address.
    pub fn isolation(&self) -> &FileName {
        &self.isolation
    }

    /// Short identifier of this tenant address for telemetry. Stable across daemon restarts and
    /// machines with the same checkout path.
    pub fn telemetry_id(&self) -> String {
        let mut hasher = blake3::Hasher::new_derive_key("buck2.tenant.id.v1");
        hasher.update(self.project_root.as_os_str().as_encoded_bytes());
        hasher.update(&[0]);
        hasher.update(self.isolation.as_str().as_bytes());
        short_hex(hasher.finalize())
    }
}

/// Identifier of one tenant generation within one daemon process, for telemetry.
///
/// Plays the role `daemon_uuid` played while a daemon served exactly one tenant: it changes when
/// the tenant's build state is recreated and differs between tenants of a shared daemon. In an
/// unshared daemon it maps one-to-one onto the daemon id.
pub fn tenant_instance_id(daemon_id: &str, tenant_id: &str, generation: u64) -> String {
    let mut hasher = blake3::Hasher::new_derive_key("buck2.tenant.instance.v1");
    hasher.update(daemon_id.as_bytes());
    hasher.update(&[0]);
    hasher.update(tenant_id.as_bytes());
    hasher.update(&[0]);
    hasher.update(&generation.to_le_bytes());
    short_hex(hasher.finalize())
}

fn short_hex(hash: blake3::Hash) -> String {
    hash.to_hex()[..16].to_owned()
}

/// Description used to validate or construct a tenant.
///
/// It initially contains only the stable address. Tenant lifecycle work will add construction
/// inputs here as their ownership is separated from daemon-wide configuration.
#[derive(Allocative, Clone, Debug, Eq, PartialEq)]
pub struct TenantSpec {
    key: TenantKey,
}

impl TenantSpec {
    /// Creates the initial tenant specification while excluding invocation-only state.
    pub fn from_invocation_paths(paths: &InvocationPaths) -> Self {
        Self::from_tenant_paths(&paths.tenant_paths())
    }

    /// Creates the initial tenant specification from stable tenant paths.
    pub fn from_tenant_paths(paths: &TenantPaths) -> Self {
        Self {
            key: TenantKey::from_tenant_paths(paths),
        }
    }

    /// Returns the stable address used to find this tenant.
    pub fn key(&self) -> &TenantKey {
        &self.key
    }
}

#[cfg(test)]
mod tests {
    use buck2_core::fs::project::ProjectRoot;
    use buck2_core::fs::project_rel_path::ProjectRelativePath;
    use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
    use buck2_fs::paths::file_name::FileNameBuf;

    use super::*;
    use crate::daemon_routing::DaemonRouting;
    use crate::invocation_roots::InvocationRoots;

    fn paths(cwd: &str, isolation: &str) -> InvocationPaths {
        let project_root = if cfg!(windows) {
            "C:\\project"
        } else {
            "/project"
        };

        InvocationPaths::new(
            InvocationRoots {
                project_root: ProjectRoot::new_unchecked(
                    AbsNormPathBuf::try_from(project_root.to_owned())
                        .expect("test project root should be absolute and normalized"),
                ),
                cwd: ProjectRelativePath::new(cwd)
                    .expect("test cwd should be project-relative")
                    .to_buf(),
            },
            FileNameBuf::try_from(isolation.to_owned())
                .expect("test isolation should be a file name"),
            Some(DaemonRouting::Unshared),
        )
    }

    #[test]
    fn tenant_key_ignores_cwd() {
        let root = TenantSpec::from_invocation_paths(&paths("", "v2"));
        let subdir = TenantSpec::from_invocation_paths(&paths("subdir", "v2"));

        assert_eq!(root.key(), subdir.key());
    }

    #[test]
    fn tenant_key_includes_isolation() {
        let first = TenantSpec::from_invocation_paths(&paths("", "first"));
        let second = TenantSpec::from_invocation_paths(&paths("", "second"));

        assert_ne!(first.key(), second.key());
    }

    #[test]
    fn telemetry_id_is_stable_and_distinguishes_isolation() {
        let first = TenantKey::from_invocation_paths(&paths("", "first"));
        let second = TenantKey::from_invocation_paths(&paths("", "second"));

        assert_eq!(first.telemetry_id(), first.telemetry_id());
        assert_eq!(first.telemetry_id().len(), 16);
        assert_ne!(first.telemetry_id(), second.telemetry_id());
    }

    #[test]
    fn tenant_instance_id_changes_with_generation_and_daemon() {
        let tenant = TenantKey::from_invocation_paths(&paths("", "v2")).telemetry_id();

        assert_eq!(
            tenant_instance_id("daemon", &tenant, 1),
            tenant_instance_id("daemon", &tenant, 1)
        );
        assert_ne!(
            tenant_instance_id("daemon", &tenant, 1),
            tenant_instance_id("daemon", &tenant, 2)
        );
        assert_ne!(
            tenant_instance_id("daemon", &tenant, 1),
            tenant_instance_id("other", &tenant, 1)
        );
    }
}
