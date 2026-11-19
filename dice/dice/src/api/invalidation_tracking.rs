/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Invalidation sources: which reported change a value depends on.
//!
//! A *source* is a change reported to dice: a value set for an injected key with `changed_to`,
//! or a key dirtied with `changed`. A value depends on every source in force at its version
//! among the injected and dirtied keys its computation transitively read, whether or not the
//! change made a difference to any value in between. Its [`InvalidationSources`] record the
//! version of the most recent of those.
//!
//! Sources are tracked at two priorities. Each key type chooses with
//! [`Key::invalidation_source_priority`](crate::Key::invalidation_source_priority) which it is
//! as a source: the normal-priority entry considers sources of normal and of high priority, the
//! high-priority entry only sources of high priority, and keys of `Ignored` priority are never
//! sources. Every value takes part in propagating both.
//!
//! During a computation, [`DiceComputations::get_invalidation_sources`] reports the sources of
//! the dependencies requested so far.
//!
//! [`DiceComputations::get_invalidation_sources`]: crate::DiceComputations::get_invalidation_sources

use allocative::Allocative;
use dupe::Dupe;

use crate::api::key::InvalidationSourcePriority;
use crate::versions::VersionNumber;

/// The most recent change among the sources a value depends on, per priority: `None` where it
/// depends on no source of that priority.
#[derive(Allocative, Clone, Copy, Dupe, Debug, PartialEq, Eq)]
pub struct InvalidationSources {
    normal: Option<VersionNumber>,
    high: Option<VersionNumber>,
}

impl InvalidationSources {
    /// Depends on no source.
    pub(crate) const NONE: Self = Self {
        normal: None,
        high: None,
    };

    /// Depends on one source, changed at `at`.
    pub(crate) fn source(priority: InvalidationSourcePriority, at: VersionNumber) -> Self {
        match priority {
            InvalidationSourcePriority::Ignored => Self::NONE,
            InvalidationSourcePriority::Normal => Self {
                normal: Some(at),
                high: None,
            },
            InvalidationSourcePriority::High => Self {
                normal: Some(at),
                high: Some(at),
            },
        }
    }

    /// Depends on the sources of both. The versions in `self` and `other` must lie on one chain,
    /// which they do when both are in force at one version.
    pub(crate) fn merge(self, other: Self) -> Self {
        fn later(a: Option<VersionNumber>, b: Option<VersionNumber>) -> Option<VersionNumber> {
            match (a, b) {
                (Some(a), Some(b)) => Some(if a.cmp_on_chain(b).is_lt() { b } else { a }),
                (a, b) => a.or(b),
            }
        }
        Self {
            normal: later(self.normal, other.normal),
            high: later(self.high, other.high),
        }
    }

    pub(crate) fn update(&mut self, other: Self) {
        *self = self.merge(other);
    }

    /// The most recent change among the sources of normal or high priority.
    pub fn normal_priority(&self) -> Option<VersionNumber> {
        self.normal
    }

    /// The most recent change among the sources of high priority.
    pub fn high_priority(&self) -> Option<VersionNumber> {
        self.high
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_keeps_the_latest_source_per_priority() {
        let v = VersionNumber::testing_new;
        let normal = |at| InvalidationSources::source(InvalidationSourcePriority::Normal, at);
        let high = |at| InvalidationSources::source(InvalidationSourcePriority::High, at);

        let mut sources = InvalidationSources::NONE;
        sources.update(normal(v(2)));
        assert_eq!(sources, normal(v(2)));
        sources.update(high(v(3)));
        assert_eq!(sources, high(v(3)));
        sources.update(normal(v(4)));
        assert_eq!(sources, normal(v(4)).merge(high(v(3))));
        assert_eq!(sources.normal_priority(), Some(v(4)));
        assert_eq!(sources.high_priority(), Some(v(3)));
        sources.update(InvalidationSources::source(
            InvalidationSourcePriority::Ignored,
            v(5),
        ));
        assert_eq!(sources.normal_priority(), Some(v(4)));
    }
}
