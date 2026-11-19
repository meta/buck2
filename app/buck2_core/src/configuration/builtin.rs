/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use allocative::Allocative;
use dupe::Dupe;
use pagable::Pagable;
use strong_hash::StrongHash;

#[derive(
    Debug, Copy, Clone, Dupe, Eq, PartialEq, Hash, Ord, PartialOrd, Allocative, StrongHash, Pagable
)]
pub enum BuiltinPlatform {
    /// The unbound platform is used when we don't yet have a platform bound. This is to support initialization
    /// and is used when analyzing a platform target itself (since we clearly can't have a platform yet bound
    /// at that point).
    /// That leads to a slight oddity that the platform's deps are processed with an empty platform. And so,
    /// the ConfigurationInfo it receives from a constraint that it itself sets, will indicate that the constraint
    /// doesn't match.
    Unbound,

    /// In some cases, a build does not have a default platform. In this case, we will use the "unspecified"
    /// platform. Any attempt to access the ConfigurationData for an "unspecified" platform will fail. This means
    /// that any use of constraints or selects or any other "configuration" feature will fail.
    ///
    /// This is generally only the case for users that haven't yet adopted configurations.
    Unspecified,
    UnspecifiedExec,

    /// The unbound_exec platform is used when we don't yet have an execution platform bound. This is used so that
    /// we can get the exec deps of a "configured" attr, which we need to resolve the execution platform.
    UnboundExec,
}

impl Display for BuiltinPlatform {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}

impl BuiltinPlatform {
    pub(crate) fn label(&self) -> &str {
        match self {
            BuiltinPlatform::Unbound => "<unbound>",
            BuiltinPlatform::UnboundExec => "<unbound_exec>",
            BuiltinPlatform::Unspecified => "<unspecified>",
            BuiltinPlatform::UnspecifiedExec => "<unspecified_exec>",
        }
    }

    pub(crate) fn from_label(label: &str) -> Option<BuiltinPlatform> {
        match label {
            "<unbound>" => Some(BuiltinPlatform::Unbound),
            "<unbound_exec>" => Some(BuiltinPlatform::UnboundExec),
            "<unspecified>" => Some(BuiltinPlatform::Unspecified),
            "<unspecified_exec>" => Some(BuiltinPlatform::UnspecifiedExec),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::configuration::builtin::BuiltinPlatform;

    #[test]
    fn test_from_label() {
        for platform in [
            BuiltinPlatform::Unbound,
            BuiltinPlatform::UnboundExec,
            BuiltinPlatform::Unspecified,
            BuiltinPlatform::UnspecifiedExec,
        ] {
            assert_eq!(
                Some(platform),
                BuiltinPlatform::from_label(platform.label())
            );
        }
    }
}
