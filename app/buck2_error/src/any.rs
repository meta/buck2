/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Integrations of `buck2_error::Error` with `StdError`.

use std::error::Error as StdError;

use buck2_data::error::ErrorTag;

use crate::source_location::SourceLocation;

#[cold]
#[track_caller]
pub fn from_any_with_tag<T>(e: T, tag: ErrorTag) -> crate::Error
where
    T: Into<anyhow::Error>,
{
    let anyhow: anyhow::Error = e.into();
    let caller = std::panic::Location::caller();
    let source_location = crate::source_location::SourceLocation::new(caller.file(), caller.line());
    from_any_with_tag_and_source_location(anyhow.as_ref(), source_location, tag)
}

pub(crate) fn from_any_with_tag_and_source_location(
    value: &'_ (dyn StdError + 'static),
    source_location: SourceLocation,
    error_tag: ErrorTag,
) -> crate::Error {
    // Instead of just turning this into an error root, we'll extract the whole context stack and
    // convert it manually.
    let mut context_stack = Vec::new();
    let mut cur = value;
    // We allow all of these to appear more than once in the context chain, however we always use
    // the bottom-most value when actually generating the root
    let base = 'base: loop {
        // Compute the next element in the source chain
        if let Some(new_cur) = cur.source() {
            context_stack.push(cur);
            cur = new_cur;
            continue;
        }

        // `anyhow` only ever uses the standard `Display` formatting of error types, never the
        // alternate or debug formatting. We can match that behavior by just converting the error to
        // a string. That prevents us from having to deal with the type returned by `source` being
        // potentially non-`Send` or non-`Sync`.
        let description = format!("{cur}");
        let e = crate::Error::new(description, error_tag, source_location, None);
        break 'base e;
    };
    // We've converted the base error to a `buck2_error::Error`. Next, we need to add back any
    // context that is not included in the `base` error yet.
    let mut e = base;
    for context_value in context_stack.into_iter().rev() {
        e = e.context(format!("{context_value}"));
    }
    e
}

#[cfg(test)]
mod tests {
    use crate as buck2_error;

    #[derive(Debug, buck2_error_derive::Error)]
    #[buck2(input)]
    #[error("unused")]
    struct UserMetadataError;

    #[derive(Debug, buck2_error_derive::Error)]
    #[buck2(tier0)]
    #[error("unused")]
    struct InfraMetadataWrapperError(#[source] UserMetadataError);

    #[test]
    fn test_no_root_metadata_context() {
        let e = InfraMetadataWrapperError(UserMetadataError);
        let e: crate::Error = e.into();
        assert_eq!(e.get_tier(), Some(crate::Tier::Tier0));
    }
}
