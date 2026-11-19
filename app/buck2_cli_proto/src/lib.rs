/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![feature(min_specialization)]
#![allow(clippy::large_enum_variant)]

use buck2_error::internal_error;

use crate::BuckDaemonProtoError::MissingClientContext;

pub mod new_generic;
pub mod protobuf_util;
pub mod test_executor;

tonic::include_proto!("buck.daemon");

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Tier0)]
enum BuckDaemonProtoError {
    #[error("daemon request was missing client context")]
    MissingClientContext,
}

#[track_caller]
fn wrong_request_type(request_type: &'static str) -> buck2_error::Error {
    internal_error!("wrong gRPC request message type, expecting {request_type}")
}

impl ConfigOverride {
    /// Not `fbcode//config.key=value`
    pub fn flag_no_cell(s: &str) -> Self {
        Self {
            cell: None,
            config_override: s.to_owned(),
            config_type: crate::config_override::ConfigType::Value.into(),
        }
    }
}

pub trait HasClientContext {
    fn client_context(&self) -> buck2_error::Result<&ClientContext>;
}

impl HasClientContext for StreamingRequest {
    fn client_context(&self) -> buck2_error::Result<&ClientContext> {
        match self.request.as_ref() {
            Some(streaming_request::Request::Context(ctx)) => Ok(ctx),
            _ => Err(MissingClientContext.into()),
        }
    }
}

impl HasBuildOptions for StreamingRequest {
    fn build_options(&self) -> Option<&CommonBuildOptions> {
        None
    }
}

impl TryFrom<StreamingRequest> for LspRequest {
    type Error = buck2_error::Error;

    fn try_from(value: StreamingRequest) -> Result<Self, Self::Error> {
        match value.request {
            Some(streaming_request::Request::Lsp(req)) => Ok(req),
            _ => Err(wrong_request_type("LspRequest")),
        }
    }
}

impl From<LspRequest> for StreamingRequest {
    fn from(request: LspRequest) -> Self {
        Self {
            request: Some(streaming_request::Request::Lsp(request)),
        }
    }
}

impl TryFrom<StreamingRequest> for SubscriptionRequestWrapper {
    type Error = buck2_error::Error;

    fn try_from(value: StreamingRequest) -> Result<Self, Self::Error> {
        match value.request {
            Some(streaming_request::Request::Subscription(req)) => Ok(req),
            _ => Err(wrong_request_type("SubscriptionRequestWrapper")),
        }
    }
}

impl From<SubscriptionRequestWrapper> for StreamingRequest {
    fn from(request: SubscriptionRequestWrapper) -> Self {
        Self {
            request: Some(streaming_request::Request::Subscription(request)),
        }
    }
}

impl TryFrom<StreamingRequest> for DapRequest {
    type Error = buck2_error::Error;

    fn try_from(value: StreamingRequest) -> Result<Self, Self::Error> {
        match value.request {
            Some(streaming_request::Request::Dap(req)) => Ok(req),
            _ => Err(wrong_request_type("DapRequest")),
        }
    }
}

impl From<DapRequest> for StreamingRequest {
    fn from(request: DapRequest) -> Self {
        Self {
            request: Some(streaming_request::Request::Dap(request)),
        }
    }
}

/// Trait for requests that have CommonBuildOptions.
pub trait HasBuildOptions {
    fn build_options(&self) -> Option<&CommonBuildOptions>;
}

macro_rules! result_convert {
    ( $name:ident ) => {
        impl From<$name> for command_result::Result {
            fn from(v: $name) -> Self {
                Self::$name(v)
            }
        }

        impl TryFrom<command_result::Result> for $name {
            type Error = command_result::Result;
            fn try_from(r: command_result::Result) -> Result<Self, Self::Error> {
                if let command_result::Result::$name(res) = r {
                    Ok(res)
                } else {
                    Err(r)
                }
            }
        }
    };
}

macro_rules! partial_result_convert {
    ( $name:ident ) => {
        impl From<$name> for partial_result::PartialResult {
            fn from(v: $name) -> Self {
                Self::$name(v)
            }
        }

        impl TryFrom<partial_result::PartialResult> for $name {
            type Error = partial_result::PartialResult;

            fn try_from(r: partial_result::PartialResult) -> Result<Self, Self::Error> {
                #![allow(irrefutable_let_patterns)]

                if let partial_result::PartialResult::$name(res) = r {
                    Ok(res)
                } else {
                    Err(r)
                }
            }
        }
    };
}

/// Helper macro for defining a request and implementing common traits for said request. See the bottom of this file
/// for usage examples.
macro_rules! define_request {
    // define_request! expects an invocation of the form `define_request!(MyRequest, has(foo, bar, baz))`. The bulk of
    // this macro comes from parsing the contents of the `has` token tree. This is done by parsing the comma-delimited
    // list of identifiers tail-recursively while passing down parsing state as parameters to rules prefixed by `@has`.
    //
    // There are three state parameters in each of the non-public rules:
    //  1) @has, which marks the rule as non-public. The public rules pass @has as the first tokens when recursively
    //     invoking this rule.
    //  2) $name:ident, the name of the request.
    //  3) $has_buildopts:ident, a `true` or `false` token indicating whether or not we've emitted an impl of
    //     HasBuildOptions yet.
    //
    // If we reach the end of the token stream without emitting a HasBuildOptions impl, we'll emit a trivial one that
    // returns None.

    ( @has $name:ident $has_buildopts:ident context $($tail:ident)* ) => {
        impl HasClientContext for $name {
            fn client_context(&self) -> buck2_error::Result<&ClientContext> {
                // A request that has a client context field should always set the context.
                match &self.context {
                    Some(v) => Ok(v),
                    None => Err(BuckDaemonProtoError::MissingClientContext.into()),
                }
            }
        }

        define_request!(@has $name $has_buildopts $($tail)*);
    };

    ( @has $name:ident $has_buildopts:ident build_options $($tail:ident)* ) => {
        impl HasBuildOptions for $name {
            fn build_options(&self) -> Option<&CommonBuildOptions> {
                self.build_opts.as_ref()
            }
        }

        define_request!(@has $name true $($tail)*);
    };

    ( @has $name:ident false) => {
        impl HasBuildOptions for $name {
            fn build_options(&self) -> Option<&CommonBuildOptions> {
                None
            }
        }
    };

    ( @has $name:ident true) => {};

    // ------ Public API begins here.

    ( $name:ident, has ($($tail:ident),*)) => {
        define_request!(@has $name false $($tail)*);
    };

    ( $name:ident ) => {};
}

mod serialize_timestamp {
    use serde::Deserialize;
    use serde::Deserializer;
    use serde::Serialize;
    use serde::Serializer;

    pub fn serialize<S>(
        value: &Option<::prost_types::Timestamp>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let d = value.as_ref().map(|v| (v.seconds, v.nanos));
        d.serialize(serializer)
    }

    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<Option<::prost_types::Timestamp>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let d = Option::<(i64, i32)>::deserialize(deserializer)?;
        let d = d.map(|(seconds, nanos)| ::prost_types::Timestamp { seconds, nanos });
        Ok(d)
    }
}

result_convert!(AqueryResponse);
result_convert!(KillResponse);
result_convert!(PingResponse);
result_convert!(StatusResponse);
result_convert!(BuildResponse);
result_convert!(BxlResponse);
result_convert!(TestResponse);
result_convert!(UqueryResponse);
result_convert!(CqueryResponse);
result_convert!(TargetsResponse);
result_convert!(TargetsShowOutputsResponse);
result_convert!(ConfiguredTargetsResponse);
result_convert!(GenericResponse);
result_convert!(HydrationResponse);
result_convert!(ProfileResponse);
result_convert!(InstallResponse);
result_convert!(CleanStaleResponse);
result_convert!(LspResponse);
result_convert!(DapResponse);
result_convert!(AllocativeResponse);
result_convert!(SubscriptionCommandResponse);
result_convert!(TraceIoResponse);
result_convert!(NewGenericResponseMessage);

partial_result_convert!(StdoutBytes);
partial_result_convert!(LspMessage);
partial_result_convert!(SubscriptionResponseWrapper);
partial_result_convert!(DapMessage);

define_request!(KillRequest);
define_request!(StatusRequest);
define_request!(PingRequest);
define_request!(HydrationRequest, has(context));
define_request!(UnstableDiceDumpRequest, has(context));
define_request!(FlushDepFilesRequest, has(context));

define_request!(BuildRequest, has(context, build_options));
define_request!(BxlRequest, has(context, build_options));
define_request!(TargetsRequest, has(context));
define_request!(ConfiguredTargetsRequest, has(context));
define_request!(AqueryRequest, has(context));
define_request!(CqueryRequest, has(context));
define_request!(UqueryRequest, has(context));
define_request!(TestRequest, has(context, build_options));
define_request!(GenericRequest, has(context));
define_request!(ProfileRequest, has(context));
define_request!(AllocativeRequest, has(context));
define_request!(CleanStaleRequest, has(context));
define_request!(FileStatusRequest, has(context));
define_request!(TraceIoRequest, has(context));
define_request!(NewGenericRequestMessage, has(context));

define_request!(InstallRequest, has(context, build_options));
