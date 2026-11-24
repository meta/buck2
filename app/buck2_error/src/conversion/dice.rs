/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use dice_error::DiceErrorImpl;
use dice_error::storage::PagableStorageBackendParseError;

use crate::ErrorTag;

impl From<dice_error::DiceError> for crate::Error {
    #[cold]
    #[track_caller]
    fn from(value: dice_error::DiceError) -> Self {
        let error_tag = match *value.0 {
            DiceErrorImpl::DuplicateChange(_) => ErrorTag::DiceDuplicatedChange,
            DiceErrorImpl::ChangedToInvalid(_) => ErrorTag::DiceChangedToInvalid,
            DiceErrorImpl::InjectedKeyGotInvalidation(_) => {
                ErrorTag::DiceInjectedKeyGotInvalidation
            }
            DiceErrorImpl::PageInFailed { .. } => ErrorTag::DicePageInFailed,
            DiceErrorImpl::DuplicateActivationData => ErrorTag::DiceDuplicateActivationData,
            DiceErrorImpl::Cycle(_) => ErrorTag::DiceCycle,
        };

        crate::conversion::from_any_with_tag(value, error_tag)
    }
}
impl From<PagableStorageBackendParseError> for crate::Error {
    #[cold]
    #[track_caller]
    fn from(value: PagableStorageBackendParseError) -> Self {
        crate::conversion::from_any_with_tag(value, ErrorTag::Input)
    }
}

#[cfg(test)]
mod tests {
    use crate::ErrorTag;

    #[test]
    fn page_in_failure_is_categorized_by_its_tag() {
        let error = dice_error::DiceError::page_in_failed(
            "SomeKey".to_owned(),
            anyhow::anyhow!("no row").into(),
        );
        let error = crate::Error::from(error);
        assert!(error.has_tag(ErrorTag::DicePageInFailed));
        assert_eq!(error.category_key(), "DICE_PAGE_IN_FAILED");
    }
}
