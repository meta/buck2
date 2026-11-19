/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

fn has_certificate_expired_message(error: &dyn std::fmt::Display) -> bool {
    error.to_string().contains("CertificateExpired")
}

impl From<hyper_util::client::legacy::Error> for crate::Error {
    #[cold]
    #[track_caller]
    fn from(value: hyper_util::client::legacy::Error) -> Self {
        let cert_expired = has_certificate_expired_message(&value);
        let error = crate::conversion::from_any_with_tag(value, crate::ErrorTag::Hyper);
        if cert_expired {
            error.tag([crate::ErrorTag::CertExpired])
        } else {
            error
        }
    }
}

impl From<hyper::Error> for crate::Error {
    #[cold]
    #[track_caller]
    fn from(value: hyper::Error) -> Self {
        let cert_expired = has_certificate_expired_message(&value);
        let error = crate::conversion::from_any_with_tag(value, crate::ErrorTag::Hyper);
        if cert_expired {
            error.tag([crate::ErrorTag::CertExpired])
        } else {
            error
        }
    }
}
