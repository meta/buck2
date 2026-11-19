/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use buck2_certs::validate::validate_certs;
use buck2_error::ErrorTag;
use buck2_fs::paths::abs_path::AbsPathBuf;
use dupe::Dupe;
use edenfs::BinaryHash;
use edenfs::EdenErrorType;
use edenfs::FileAttributeDataOrErrorV2;
use edenfs::FileAttributeDataV2;
use edenfs::PathString;
use edenfs::SourceControlType;
use edenfs_clients::errors::ListMountsError;
use fbthrift::ApplicationException;
use fbthrift::ApplicationExceptionErrorCode;
use sorted_vector_map::SortedVectorMap;

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = IoEdenMountNotReady)]
#[error("Mount never became ready: `{mount}`")]
pub struct MountNeverBecameReady {
    pub mount: AbsPathBuf,
}

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = IoEdenDaemonRestarted)]
#[error(
    "EdenFS restarted (pid {old_pid} -> {new_pid}, start unixtime {} -> {}) while this buck2 \
     daemon was running. Restart buck2 with `buck2 kill`",
    format_start_time(old_start_time),
    format_start_time(new_start_time)
)]
pub struct EdenDaemonRestarted {
    pub old_pid: i32,
    pub old_start_time: Option<SystemTime>,
    pub new_pid: i32,
    pub new_start_time: Option<SystemTime>,
}

fn format_start_time(start_time: &Option<SystemTime>) -> String {
    match start_time.and_then(|t| t.duration_since(UNIX_EPOCH).ok()) {
        Some(since_epoch) => since_epoch.as_secs().to_string(),
        None => "unknown".to_owned(),
    }
}

#[derive(buck2_error::Error, Debug)]
pub enum IsMountReadyError {
    #[error("Mount does not exist in Eden: `{mount}`")]
    #[buck2(tag = IoEdenMountDoesNotExist)]
    MountDoesNotExist { mount: AbsPathBuf },
    #[error("Eden Request Failed: {0:?}")]
    #[buck2(tag = IoEdenRequestError, tag = IoEdenListMounts)]
    RequestError(ListMountsError),
}

#[derive(buck2_error::Error, Debug)]
pub enum ConnectAndRequestError<E> {
    #[error(transparent)]
    #[buck2(tag = IoEdenConnectionError)]
    ConnectionError(buck2_error::Error),
    #[error("Eden Request Failed: {0:?}")]
    #[buck2(tag = IoEdenRequestError)]
    RequestError(E),
}

fn is_app_exception_from_hanging(e: &ApplicationException) -> bool {
    // Timeout indicates that a single Thrift request took too long. Loadshedding indicates "Queue
    // Timeout" errors which are typically caused by heavy load on the Thrift server.
    e.type_ == ApplicationExceptionErrorCode::Timeout
        || e.type_ == ApplicationExceptionErrorCode::Loadshedding
}

pub trait ErrorFromHangingMount {
    fn is_caused_by_hanging_mount(&self) -> bool;
}

impl<E: ErrorFromHangingMount> ErrorFromHangingMount for ConnectAndRequestError<E> {
    fn is_caused_by_hanging_mount(&self) -> bool {
        match self {
            // NOTE: This is slightly incorrect, but the only user of this trait is the EdenFS
            // health check. The health check short circuits on ConnectionErrors, and therefore
            // this codepath will never be hit. This code can be fixed using structured logging to
            // return a new flavor of ConnectionError for hangs during client creation.
            Self::ConnectionError(_) => false,
            Self::RequestError(e) => e.is_caused_by_hanging_mount(),
        }
    }
}

macro_rules! impl_error_from_hanging_mount {
    ($err: ident) => {
        impl ErrorFromHangingMount for ::edenfs_clients::errors::$err {
            fn is_caused_by_hanging_mount(&self) -> bool {
                match self {
                    // For now, we assume all Thrift Errors are not caused by a hanging mount.
                    Self::ThriftError(..) => false,
                    Self::ApplicationException(e) if is_app_exception_from_hanging(e) => true,
                    _ => false,
                }
            }
        }
    };
}

impl_error_from_hanging_mount!(GetDaemonInfoError);

#[derive(Copy, Clone, Dupe, PartialEq, Eq)]
pub enum ErrorHandlingStrategy {
    Reconnect,
    Retry,
    Abort,
}

pub trait HasErrorHandlingStrategy {
    fn error_handling_strategy(&self) -> ErrorHandlingStrategy;
}

impl<E: HasErrorHandlingStrategy> HasErrorHandlingStrategy for ConnectAndRequestError<E> {
    fn error_handling_strategy(&self) -> ErrorHandlingStrategy {
        match self {
            Self::ConnectionError(..) => ErrorHandlingStrategy::Reconnect,
            Self::RequestError(e) => e.error_handling_strategy(),
        }
    }
}

impl HasErrorHandlingStrategy for IsMountReadyError {
    fn error_handling_strategy(&self) -> ErrorHandlingStrategy {
        match self {
            Self::MountDoesNotExist { .. } => ErrorHandlingStrategy::Abort,
            Self::RequestError(e) => e.error_handling_strategy(),
        }
    }
}

macro_rules! impl_has_error_handling_strategy {
    ($err: ident) => {
        impl HasErrorHandlingStrategy for ::edenfs_clients::errors::$err {
            fn error_handling_strategy(&self) -> ErrorHandlingStrategy {
                match self {
                    Self::ThriftError(..) => ErrorHandlingStrategy::Reconnect,
                    Self::ApplicationException(..) => ErrorHandlingStrategy::Retry,
                    Self::ex(..) => ErrorHandlingStrategy::Abort,
                }
            }
        }
    };
}

impl_has_error_handling_strategy!(GetAttributesFromFilesV2Error);
impl_has_error_handling_strategy!(GlobFilesError);
impl_has_error_handling_strategy!(ListMountsError);
impl_has_error_handling_strategy!(SynchronizeWorkingCopyError);
impl_has_error_handling_strategy!(SetPathObjectIdError);
impl_has_error_handling_strategy!(RemoveRecursivelyError);
impl_has_error_handling_strategy!(EnsureMaterializedError);
impl_has_error_handling_strategy!(ReaddirError);
impl_has_error_handling_strategy!(GetFileContentError);
impl_has_error_handling_strategy!(GetSHA1Error);
impl_has_error_handling_strategy!(GetCurrentJournalPositionError);
impl_has_error_handling_strategy!(ChangesSinceV2Error);
impl_has_error_handling_strategy!(GetDaemonInfoError);

fn eden_posix_error_tag(code: i32) -> ErrorTag {
    match code {
        libc::ENOENT => ErrorTag::IoEdenFileNotFound,
        libc::EACCES | libc::EPERM => ErrorTag::IoPermissionDenied,
        libc::ETIMEDOUT => ErrorTag::IoTimeout,
        libc::EBUSY => ErrorTag::IoExecutableFileBusy,
        libc::EPIPE => ErrorTag::IoBrokenPipe,
        libc::ENOSPC => ErrorTag::IoStorageFull,
        libc::ECONNABORTED => ErrorTag::IoConnectionAborted,
        libc::ENOTCONN => ErrorTag::IoNotConnected,
        libc::EBADMSG => ErrorTag::IoEdenDataCorruption,
        _ => ErrorTag::IoEdenUncategorized,
    }
}

fn eden_network_error_tag(code_opt: Option<i32>) -> ErrorTag {
    const CURLE_OPERATION_TIMEDOUT: i32 = curl_sys::CURLE_OPERATION_TIMEDOUT as i32;
    const CURLE_RECV_ERROR: i32 = curl_sys::CURLE_RECV_ERROR as i32;
    const CURLE_SSL_CERTPROBLEM: i32 = curl_sys::CURLE_SSL_CERTPROBLEM as i32;

    match code_opt {
        Some(code) => {
            match code {
                CURLE_OPERATION_TIMEDOUT => ErrorTag::IoEdenNetworkCurlTimedout, // 28
                CURLE_RECV_ERROR | CURLE_SSL_CERTPROBLEM => ErrorTag::IoEdenNetworkTls, // 56 and 58
                401 => ErrorTag::HttpUnauthorized, // http::StatusCode::UNAUTHORIZED
                403 => ErrorTag::HttpForbidden,    // http::StatusCode::FORBIDDEN
                503 => ErrorTag::HttpServiceUnavailable, // http::StatusCode::SERVICE_UNAVAILABLE
                _ => ErrorTag::IoEdenNetworkUncategorized,
            }
        }
        None => ErrorTag::IoEdenNetworkUncategorized,
    }
}

fn eden_service_error_tag(error: &edenfs::EdenError) -> ErrorTag {
    match error.errorType {
        EdenErrorType::WIN32_ERROR => ErrorTag::IoEdenWin32Error,
        EdenErrorType::HRESULT_ERROR => ErrorTag::IoEdenHresultError,
        EdenErrorType::ARGUMENT_ERROR => ErrorTag::IoEdenArgumentError,
        EdenErrorType::GENERIC_ERROR => ErrorTag::IoEdenGenericError,
        EdenErrorType::MOUNT_GENERATION_CHANGED => ErrorTag::IoEdenMountGenerationChanged,
        EdenErrorType::JOURNAL_TRUNCATED => ErrorTag::IoEdenJournalTruncated,
        EdenErrorType::CHECKOUT_IN_PROGRESS => ErrorTag::IoEdenCheckoutInProgress,
        EdenErrorType::OUT_OF_DATE_PARENT => ErrorTag::IoEdenOutOfDateParent,
        EdenErrorType::ATTRIBUTE_UNAVAILABLE => ErrorTag::IoEdenAttributeUnavailable,
        _ => ErrorTag::IoEdenUncategorized,
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = IoEden)]
pub enum EdenError {
    #[error("Eden POSIX error (code = {code}): {0}", error.message)]
    #[buck2(tag = eden_posix_error_tag(code))]
    PosixError { error: edenfs::EdenError, code: i32 },

    #[error("Eden network error (code = {code:?}): {0}", error.message)]
    #[buck2(tag = eden_network_error_tag(code))]
    NetworkError {
        error: edenfs::EdenError,
        code: Option<i32>,
    },

    #[error("Eden service error: {0}", error.message)]
    #[buck2(tag = eden_service_error_tag(&error))]
    ServiceError { error: edenfs::EdenError },

    #[error("Eden returned an unexpected field: {field}")]
    #[buck2(tag = IoEdenUnknownField)]
    UnknownField { field: i32 },
}

impl From<edenfs::EdenError> for EdenError {
    fn from(error: edenfs::EdenError) -> Self {
        if error.errorType == EdenErrorType::POSIX_ERROR {
            if let Some(error_code) = error.errorCode {
                return Self::PosixError {
                    error,
                    code: error_code,
                };
            }
        } else if error.errorType == EdenErrorType::NETWORK_ERROR {
            let code_opt = error.errorCode;
            return Self::NetworkError {
                error,
                code: code_opt,
            };
        } else if error.errorType == EdenErrorType::GENERIC_ERROR {
            // TODO(minglunli): Hacky solution to check if Eden errors are cert related
            if let Err(e) = futures::executor::block_on(validate_certs()) {
                let eden_err = edenfs::EdenError {
                    message: format!("{e}"),
                    ..error
                };

                return Self::ServiceError { error: eden_err };
            }
        }

        Self::ServiceError { error }
    }
}

pub trait EdenDataIntoResult {
    type Data;

    fn into_result(self) -> Result<Self::Data, EdenError>;
}

macro_rules! impl_eden_data_into_result {
    ($typ: ident, $data: ty, $ok_variant: ident) => {
        impl EdenDataIntoResult for ::edenfs::$typ {
            type Data = $data;

            fn into_result(self) -> Result<Self::Data, EdenError> {
                match self {
                    Self::$ok_variant(data) => Ok(data),
                    Self::error(e) => Err(e.into()),
                    Self::UnknownField(field) => Err(EdenError::UnknownField { field }),
                }
            }
        }
    };
}

impl_eden_data_into_result!(
    SourceControlTypeOrError,
    SourceControlType,
    sourceControlType
);

impl_eden_data_into_result!(
    FileAttributeDataOrErrorV2,
    FileAttributeDataV2,
    fileAttributeData
);

impl_eden_data_into_result!(SizeOrError, i64, size);

impl_eden_data_into_result!(Sha1OrError, BinaryHash, sha1);

impl_eden_data_into_result!(Blake3OrError, BinaryHash, blake3);

impl_eden_data_into_result!(
    DirListAttributeDataOrError,
    SortedVectorMap<PathString, FileAttributeDataOrErrorV2>,
    dirListAttributeData
);
