/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![cfg(test)]

use buck2_data::error::ErrorTag;

use crate as buck2_error;

#[derive(buck2_error_derive::Error, Debug)]
#[error("foo")]
#[buck2(input)]
pub struct Error1;

#[test]
fn test_derive_error1() {
    let e: crate::Error = Error1.into();
    assert_eq!(e.get_tier(), Some(crate::Tier::Input));
}

#[derive(buck2_error_derive::Error, Debug)]
#[error("foo")]
#[buck2(tier0)]
#[allow(unused)]
struct Error2((), ());

#[test]
fn test_derive_error2() {
    let e: crate::Error = Error2((), ()).into();
    assert_eq!(e.get_tier(), Some(crate::Tier::Tier0));
}

#[derive(buck2_error_derive::Error, Debug)]
pub enum Error3 {
    #[error("foo")]
    #[buck2(input)]
    VariantA,
    #[error("bar")]
    #[buck2(tier0)]
    VariantB,
    #[error("baz")]
    #[buck2(tag = Environment)]
    VariantC,
}

#[test]
fn test_derive_error3() {
    let e: crate::Error = Error3::VariantA.into();
    assert_eq!(e.get_tier(), Some(crate::Tier::Input));

    let e: crate::Error = Error3::VariantB.into();
    assert_eq!(e.get_tier(), Some(crate::Tier::Tier0));

    let e: crate::Error = Error3::VariantC.into();
    assert_eq!(e.get_tier(), Some(crate::Tier::Environment));
}

#[derive(buck2_error_derive::Error, Debug)]
#[error("Generic error")]
#[buck2(tag = Environment)]
pub struct GenericError<G>(G);

#[test]
fn test_generic_error() {
    let _e: crate::Error = GenericError(42).into();
}

/// Test that no unused fields warning is emitted.
#[derive(buck2_error_derive::Error, Debug)]
#[error("Unused")]
#[buck2(tag = Environment)]
pub struct WithField {
    x: u8,
}

#[test]
fn test_with_field() {
    let _e: crate::Error = WithField { x: 42 }.into();
}

#[derive(buck2_error_derive::Error, Debug)]
#[error("Unused")]
#[buck2(tag = Environment)]
struct NoAttrsStruct;

#[derive(buck2_error_derive::Error, Debug)]
#[error("Unused")]
#[buck2(tag = TestOnly)]
enum NoAttrsEnum {
    Variant,
}

#[test]
fn test_source_location_no_attrs() {
    let e: crate::Error = NoAttrsStruct.into();
    assert!(
        e.source_location()
            .to_string()
            .starts_with("buck2_error/src/derive_tests.rs::NoAttrsStruct::")
    );
    let e: crate::Error = NoAttrsEnum::Variant.into();
    assert!(
        e.source_location()
            .to_string()
            .starts_with("buck2_error/src/derive_tests.rs::NoAttrsEnum::Variant::")
    );
}

#[derive(buck2_error_derive::Error, Debug)]
#[error("Unused")]
#[buck2(input)]
enum EnumWithTypeOption {
    Variant,
}

#[test]
fn test_enum_with_type_option() {
    let e: crate::Error = EnumWithTypeOption::Variant.into();
    assert_eq!(e.get_tier(), Some(crate::Tier::Input));
    assert!(
        e.source_location()
            .to_string()
            .starts_with("buck2_error/src/derive_tests.rs::EnumWithTypeOption::Variant::")
    );
}

#[derive(buck2_error_derive::Error, Debug)]
#[error("Unused")]
#[buck2(input)]
struct ErrorWithSpelledOutCategory;

#[test]
fn test_error_with_spelled_out_category() {
    let e: crate::Error = ErrorWithSpelledOutCategory.into();
    assert_eq!(e.get_tier(), Some(crate::Tier::Input));
}

#[test]
fn test_source_metadata_are_included() {
    #[derive(buck2_error_derive::Error, Debug)]
    #[error("WatchmanError")]
    #[buck2(tag = WatchmanTimeout)]
    struct WatchmanError;

    #[derive(buck2_error_derive::Error, Debug)]
    #[error("Unused")]
    #[buck2(tag = WatchmanRequestError)]
    enum MaybeWatchmanError {
        Some(#[source] WatchmanError),
        None,
    }

    let e: crate::Error = MaybeWatchmanError::None.into();
    assert!(e.has_tag(crate::ErrorTag::WatchmanRequestError));

    let e: crate::Error = MaybeWatchmanError::Some(WatchmanError).into();
    assert!(e.has_tag(crate::ErrorTag::WatchmanTimeout));
    assert!(e.has_tag(crate::ErrorTag::WatchmanRequestError));

    assert!(format!("{e:?}").contains("Unused"));
    assert!(format!("{e:?}").contains("WatchmanError"));
}

#[test]
fn test_error_tags() {
    fn f() -> crate::ErrorTag {
        crate::ErrorTag::StarlarkFail
    }

    #[derive(buck2_error_derive::Error, Debug)]
    #[error("Unused")]
    #[buck2(tag = WatchmanTimeout)]
    enum TaggedError {
        #[buck2(tag = f())]
        A,
        #[buck2(tag = WatchmanTimeout)]
        B,
    }

    let a: crate::Error = TaggedError::A.into();
    assert_eq!(
        &a.tags(),
        &[
            crate::ErrorTag::StarlarkFail,
            crate::ErrorTag::WatchmanTimeout
        ]
    );
    let b: crate::Error = TaggedError::B.into();
    assert_eq!(&b.tags(), &[crate::ErrorTag::WatchmanTimeout]);
}

#[test]
fn test_error_tags_vec_fn() {
    fn calc_tags(extra_tag: bool) -> Vec<ErrorTag> {
        if extra_tag {
            vec![ErrorTag::StarlarkFail]
        } else {
            vec![]
        }
    }

    #[derive(buck2_error_derive::Error, Debug)]
    #[error("Unused")]
    #[buck2(tag = WatchmanTimeout, tags = calc_tags(*extra_tag))]
    struct TaggedError {
        extra_tag: bool,
    }

    let a: crate::Error = TaggedError { extra_tag: true }.into();
    assert_eq!(
        &a.tags(),
        &[ErrorTag::StarlarkFail, ErrorTag::WatchmanTimeout]
    );
    let b: crate::Error = TaggedError { extra_tag: false }.into();
    assert_eq!(&b.tags(), &[ErrorTag::WatchmanTimeout]);
}

#[test]
fn test_correct_transparent() {
    #[derive(buck2_error_derive::Error, Debug)]
    #[error("Unused")]
    #[buck2(tier0)]
    struct E;

    #[derive(buck2_error_derive::Error, Debug)]
    #[error(transparent)]
    #[buck2(tag = Input)]
    struct T(E);

    let t: crate::Error = T(E).into();
    assert_eq!(t.get_tier(), Some(crate::Tier::Tier0));
}

#[test]
fn test_error_message_with_provided_field() {
    #[derive(buck2_error_derive::Error, Debug)]
    #[error("Some message {0} + {1}")]
    #[buck2(tag = Environment)]
    struct SomeError(String, String);

    let t: crate::Error = SomeError("test123".to_owned(), "test222".to_owned()).into();
    assert!(format!("{t:?}").contains("Some message test123"));
}

#[test]
fn test_recovery_through_transparent_buck2_error() {
    #[derive(buck2_error_derive::Error, Debug)]
    #[error("base_display")]
    #[buck2(tag = Environment)]
    struct BaseError;

    #[derive(buck2_error_derive::Error, Debug)]
    #[error(transparent)]
    #[buck2(tag = TestOnly)]
    enum PartiallyStructured {
        #[error(transparent)]
        Other(buck2_error::Error),
    }

    let base: crate::Error = crate::Error::from(BaseError).tag([crate::ErrorTag::StarlarkFail]);
    let wrapped_direct: crate::Error = PartiallyStructured::Other(base.clone()).into();

    assert!(format!("{wrapped_direct:?}").contains("base_display"));
    assert_eq!(
        &wrapped_direct.tags()[..],
        &[
            crate::ErrorTag::Environment,
            crate::ErrorTag::StarlarkFail,
            crate::ErrorTag::TestOnly
        ]
    );
}
