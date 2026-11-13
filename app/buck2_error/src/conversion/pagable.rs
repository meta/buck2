/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

impl From<pagable::DeferredReadError> for crate::Error {
    #[cold]
    #[track_caller]
    fn from(value: pagable::DeferredReadError) -> Self {
        crate::conversion::from_any_with_tag(value, crate::ErrorTag::DeferredFieldRead)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn deferred_read_error_is_tagged_and_keeps_its_context() {
        let error =
            pagable::DeferredReadError::from(anyhow::anyhow!("no row").context("restoring arc 7"));
        let error = crate::Error::from(error);
        assert!(error.has_tag(crate::ErrorTag::DeferredFieldRead));
        assert_eq!(format!("{error:#}"), "restoring arc 7: no row");
    }
}
