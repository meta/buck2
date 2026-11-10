/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Display;

use allocative::Allocative;
use buck2_core::cells::name::CellName;
use buck2_core::cells::paths::CellRelativePath;
use buck2_core::package::PackageLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_core::target::name::TargetNameRef;
use buck2_data::CommandExecutionDetails;
use buck2_data::action_key;
use buck2_error::BuckErrorOptionContext;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_interpreter::types::target_label::StarlarkTargetLabel;
use derivative::Derivative;
use derive_more::Display;
use display_container::fmt_container;
use dupe::Dupe;
use serde::Serialize;
use serde::Serializer;
use serde::ser::SerializeMap;
use starlark::any::ProvidesStaticType;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_complex_value;
use starlark::starlark_module;
use starlark::starlark_simple_value;
use starlark::values::Freeze;
use starlark::values::Heap;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::none::NoneOr;
use starlark::values::starlark_value;
use starlark::values::string::StarlarkStr;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum BxlResultError {
    #[error("called `bxl.Result.unwrap()` on an `Err` value: {0}")]
    UnwrapOnError(buck2_error::Error),
    #[error("called `bxl.Result.unwrap_err()` on an `Ok` value: {0}")]
    UnwrapErrOnOk(String),
}

/// Error value object returned by fallible BXL operation.
#[derive(
    Debug,
    ProvidesStaticType,
    Derivative,
    Display,
    Allocative,
    Trace,
    starlark::StarlarkPagable
)]
#[display("bxl.Error({})", StarlarkStr::repr(&format!("{err:?}")))]
pub(crate) struct StarlarkError {
    #[starlark_pagable(pagable)]
    err: buck2_error::Error,
}

impl Serialize for StarlarkError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("result", "error")?;
        map.serialize_entry("value", &format!("{:?}", self.err))?;
        map.end()
    }
}

impl StarlarkError {
    pub(crate) fn new(err: buck2_error::Error) -> Self {
        Self { err }
    }
}

starlark_simple_value!(StarlarkError);

starlark::methods_static!(BXL_ERROR_METHODS = error_methods);

#[starlark_value(type = "bxl.Error")]
impl<'v> StarlarkValue<'v> for StarlarkError {
    fn get_methods() -> Option<&'static Methods> {
        Some(BXL_ERROR_METHODS.methods())
    }
}

/// The error type for bxl
#[starlark_module]
fn error_methods(builder: &mut MethodsBuilder) {
    /// The error message
    #[starlark(attribute)]
    fn message<'v>(this: &'v StarlarkError) -> starlark::Result<String> {
        Ok(format!("{:?}", this.err))
    }

    /// The root error message, without contextual messages or late formatting.
    #[starlark(attribute)]
    fn root_message(this: &StarlarkError) -> starlark::Result<String> {
        Ok(this.err.root_message().to_owned())
    }

    /// Action failure details, or `None` when this error was not caused by an action.
    /// Returns a [`bxl.ActionError`](../ActionError) with recorded action diagnostics.
    #[starlark(attribute)]
    fn action_error<'v>(
        this: &StarlarkError,
        heap: Heap<'v>,
    ) -> starlark::Result<NoneOr<ValueOf<'v, StarlarkActionError>>> {
        Ok(if this.err.action_error().is_some() {
            NoneOr::Other(ValueOf::alloc(
                heap,
                StarlarkActionError {
                    err: this.err.dupe(),
                },
            ))
        } else {
            NoneOr::None
        })
    }
}

/// Recorded diagnostics for an action failure.
#[derive(
    Debug,
    ProvidesStaticType,
    Display,
    Allocative,
    Trace,
    starlark::StarlarkPagable
)]
#[display("bxl.ActionError")]
pub(crate) struct StarlarkActionError {
    #[starlark_pagable(pagable)]
    err: buck2_error::Error,
}

impl StarlarkActionError {
    fn target_label(&self) -> Option<&buck2_data::TargetLabel> {
        let action = self.err.action_error()?;
        match action.key.as_ref().and_then(|key| key.owner.as_ref()) {
            Some(
                action_key::Owner::TargetLabel(target)
                | action_key::Owner::TestTargetLabel(target)
                | action_key::Owner::LocalResourceSetup(target),
            ) => target.label.as_ref(),
            _ => None,
        }
    }

    fn target(&self) -> buck2_error::Result<Option<StarlarkTargetLabel>> {
        self.target_label()
            .map(|label| {
                let (cell, package) = label
                    .package
                    .split_once("//")
                    .internal_error("Invalid recorded action target package")?;
                let package = PackageLabel::new(
                    CellName::unchecked_new(cell)?,
                    CellRelativePath::new(ForwardRelativePath::new(package)?),
                )?;
                Ok(StarlarkTargetLabel::new(TargetLabel::new(
                    package,
                    TargetNameRef::new(&label.name)?,
                )))
            })
            .transpose()
    }

    fn command(&self) -> Option<&CommandExecutionDetails> {
        self.err
            .action_error()?
            .last_command
            .as_ref()?
            .details
            .as_ref()
    }
}

impl Serialize for StarlarkActionError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let command = self.command();
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry(
            "target",
            &self
                .target_label()
                .map(|label| format!("{}:{}", label.package, label.name)),
        )?;
        map.serialize_entry(
            "stdout",
            &command.map(|command| command.cmd_stdout.as_str()),
        )?;
        map.serialize_entry(
            "stderr",
            &command.map(|command| command.cmd_stderr.as_str()),
        )?;
        map.end()
    }
}

starlark_simple_value!(StarlarkActionError);

starlark::methods_static!(BXL_ACTION_ERROR_METHODS = action_error_methods);

#[starlark_value(type = "bxl.ActionError")]
impl<'v> StarlarkValue<'v> for StarlarkActionError {
    fn get_methods() -> Option<&'static Methods> {
        Some(BXL_ACTION_ERROR_METHODS.methods())
    }
}

#[starlark_module]
fn action_error_methods(builder: &mut MethodsBuilder) {
    /// The unconfigured target label, or `None` for BXL, anonymous, or unavailable owners.
    #[starlark(attribute)]
    fn target<'v>(
        this: &StarlarkActionError,
        heap: Heap<'v>,
    ) -> starlark::Result<NoneOr<ValueOf<'v, StarlarkTargetLabel>>> {
        Ok(NoneOr::from_option(
            this.target()?.map(|label| ValueOf::alloc(heap, label)),
        ))
    }

    /// Command stdout, or `None` when command details are unavailable. Empty streams are `""`.
    #[starlark(attribute)]
    fn stdout<'v>(this: &'v StarlarkActionError) -> starlark::Result<NoneOr<&'v str>> {
        Ok(NoneOr::from_option(
            this.command().map(|command| command.cmd_stdout.as_str()),
        ))
    }

    /// Command stderr, or `None` when command details are unavailable. Empty streams are `""`.
    #[starlark(attribute)]
    fn stderr<'v>(this: &'v StarlarkActionError) -> starlark::Result<NoneOr<&'v str>> {
        Ok(NoneOr::from_option(
            this.command().map(|command| command.cmd_stderr.as_str()),
        ))
    }
}

#[derive(
    Debug,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    starlark::StarlarkPagable
)]
pub(crate) enum StarlarkResult<'v> {
    Ok(Value<'v>),
    Err(
        #[freeze(identity)]
        #[starlark_pagable(pagable)]
        buck2_error::Error,
    ),
}

impl<'v> Serialize for StarlarkResult<'v> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(2))?;
        match self {
            StarlarkResult::Ok(val) => {
                map.serialize_entry("result", "ok")?;
                map.serialize_entry("value", val)?;
            }
            StarlarkResult::Err(err) => {
                map.serialize_entry("result", "error")?;
                map.serialize_entry("value", &format!("{:?}", err))?;
            }
        }
        map.end()
    }
}

starlark_complex_value!(pub(crate) StarlarkResult);

impl<'v> Display for StarlarkResult<'v> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StarlarkResult::Ok(val) => fmt_container(f, "Result(Ok = ", ")", [val]),
            StarlarkResult::Err(err) => fmt_container(
                f,
                "Result(Err = ",
                ")",
                // TODO(nero): implement multiline when multiline is requested
                [StarlarkStr::repr(&format!("{err:?}"))],
            ),
        }
    }
}

#[starlark_value(type = "bxl.Result")]
impl<'v> StarlarkValue<'v> for StarlarkResult<'v> {
    fn get_methods() -> Option<&'static Methods>
    where
        Self: Sized,
    {
        Some(BXL_RESULT_METHODS.methods())
    }
}

starlark::methods_static!(BXL_RESULT_METHODS = result_methods);

#[starlark_module]
fn result_methods(builder: &mut MethodsBuilder) {
    /// Returns true if the result is an `Ok` value, false if it is an Error
    fn is_ok<'v>(this: &'v StarlarkResult<'v>) -> starlark::Result<bool> {
        Ok(this.is_ok())
    }

    /// Unwrap the result, returning the inner value if the result is `Ok`.
    /// If the result is an `Error`, it will fail
    fn unwrap<'v>(this: &'v StarlarkResult<'v>) -> starlark::Result<Value<'v>> {
        Ok(this.unwrap()?)
    }

    /// If the result is an `Ok`, return the inner value, otherwise return the default
    fn unwrap_or<'v>(
        this: &'v StarlarkResult<'v>,
        #[starlark(require = pos)] default: Value<'v>,
    ) -> starlark::Result<Value<'v>> {
        Ok(this.unwrap_or(default))
    }

    /// Unwrap the error, returning the inner error if the result is `Err`.
    /// If the result is an `Ok`, it will fail
    fn unwrap_err<'v>(
        this: &'v StarlarkResult<'v>,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueOf<'v, StarlarkError>> {
        Ok(ValueOf::alloc(heap, this.unwrap_err()?))
    }
}

impl<'v> StarlarkResult<'v> {
    pub(crate) fn from_result(res: buck2_error::Result<Value<'v>>) -> Self {
        match res {
            Ok(val) => Self::Ok(val),
            Err(err) => Self::Err(err),
        }
    }

    fn is_ok(&self) -> bool {
        match self {
            StarlarkResult::Ok(_) => true,
            StarlarkResult::Err(_) => false,
        }
    }

    fn unwrap(&self) -> buck2_error::Result<Value<'v>> {
        match self {
            StarlarkResult::Ok(val) => Ok(*val),
            StarlarkResult::Err(err) => Err(BxlResultError::UnwrapOnError(err.dupe()).into()),
        }
    }

    fn unwrap_or(&self, default: Value<'v>) -> Value<'v> {
        match self {
            StarlarkResult::Ok(val) => *val,
            StarlarkResult::Err(_) => default,
        }
    }

    fn unwrap_err(&self) -> buck2_error::Result<StarlarkError> {
        match self {
            StarlarkResult::Ok(val) => {
                let display_str = format!("{val}");
                Err(BxlResultError::UnwrapErrOnOk(display_str).into())
            }
            StarlarkResult::Err(err) => Ok(StarlarkError { err: err.dupe() }),
        }
    }
}

starlark::__starlark_pagable_only! {
    #[cfg(test)]
    mod tests {
        use buck2_core::target::label::label::TargetLabel as CoreTargetLabel;
        use buck2_data::ActionError;
        use buck2_data::ActionKey;
        use buck2_data::CommandExecution;
        use buck2_data::ConfiguredTargetLabel;
        use buck2_data::TargetLabel;
        use buck2_error::ErrorTag;
        use buck2_error::source_location::SourceLocation;
        use pagable::PagableDeserialize;
        use pagable::PagableSerialize;
        use serde_json::json;
        use starlark::values::FrozenHeapName;
        use starlark::values::OwnedFrozen;

        use super::*;

        fn round_trip(
            owned: OwnedFrozen<Value<'static>>,
        ) -> pagable::Result<OwnedFrozen<Value<'static>>> {
            let mut serializer = pagable::testing::TestingSerializer::new();
            owned.pagable_serialize(&mut serializer)?;
            let bytes = serializer.finish();
            let mut deserializer = pagable::testing::TestingDeserializer::new(&bytes);
            OwnedFrozen::<Value<'static>>::pagable_deserialize(&mut deserializer)
        }

        fn test_error() -> buck2_error::Error {
            buck2_error::buck2_error!(ErrorTag::Analysis, "analysis of `cell//pkg:target` failed")
                .context("while resolving a lazy operation")
                .string_tag("bxl_result_test")
                .tag([ErrorTag::Bxl])
        }

        #[test]
        fn action_error_round_trips() -> pagable::Result<()> {
            let owner = action_key::Owner::TargetLabel(ConfiguredTargetLabel {
                label: Some(TargetLabel {
                    package: "cell//pkg".to_owned(),
                    name: "target".to_owned(),
                }),
                ..Default::default()
            });
            for (owner, command, target, stdout, stderr) in [
                (
                    Some(owner.clone()),
                    Some(CommandExecutionDetails {
                        cmd_stdout: "action stdout".to_owned(),
                        cmd_stderr: "action stderr".to_owned(),
                        ..Default::default()
                    }),
                    Some("cell//pkg:target"),
                    Some("action stdout"),
                    Some("action stderr"),
                ),
                (Some(owner.clone()), Some(CommandExecutionDetails::default()), Some("cell//pkg:target"), Some(""), Some("")),
                (Some(owner), None, Some("cell//pkg:target"), None, None),
                (None, None, None, None, None),
            ] {
                let err = buck2_error::Error::new(
                    "action failed".to_owned(),
                    ErrorTag::Input,
                    SourceLocation::new(file!(), line!()),
                    Some(ActionError {
                        key: Some(ActionKey { owner, ..Default::default() }),
                        last_command: command.map(|details| CommandExecution {
                            details: Some(details),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                )
                .context("while resolving a lazy operation");
                let expected = json!({"target": target, "stdout": stdout, "stderr": stderr});
                let action = StarlarkActionError { err };
                let expected_target = target.map(CoreTargetLabel::testing_parse);
                assert_eq!(
                    action
                        .target()
                        .expect("typed action target")
                        .map(|target| target.label().dupe()),
                    expected_target,
                );
                assert_eq!(serde_json::to_value(&action).expect("serialize action diagnostics"), expected);
                let owned: OwnedFrozen<Value<'static>> = OwnedFrozen::build(
                    FrozenHeapName::user("action_error_round_trips"),
                    |heap| heap.alloc_representation(action).to_value(),
                );
                let restored = round_trip(owned)?;
                restored.by_ref(|value| {
                    let action = value.downcast_ref::<StarlarkActionError>().expect("a bxl.ActionError");
                    assert_eq!(
                        action
                            .target()
                            .expect("restored typed action target")
                            .map(|target| target.label().dupe()),
                        expected_target,
                    );
                    assert_eq!(serde_json::to_value(action).expect("serialize restored diagnostics"), expected);
                });
            }
            Ok(())
        }

        /// A rendering cut before the backtrace `anyhow` appends under
        /// `RUST_BACKTRACE`, which is captured where the error is formatted and
        /// so never compares equal between two renderings. The cut is the same
        /// on both sides of a comparison, so what follows it does not matter,
        /// including inside a `repr` that escapes the newlines.
        fn without_backtrace(rendered: &str) -> &str {
            rendered
                .split_once("Stack backtrace:")
                .map_or(rendered, |(message, _backtrace)| message)
        }

        /// What a paged-in error keeps; see `buck2_error::paging`.
        fn assert_same_error(restored: &buck2_error::Error, original: &buck2_error::Error) {
            assert_eq!(
                without_backtrace(&format!("{restored:?}")),
                without_backtrace(&format!("{original:?}"))
            );
            assert_eq!(restored.tags(), original.tags());
            assert_eq!(restored.string_tags(), original.string_tags());
            assert_eq!(restored.source_location(), original.source_location());
            assert_eq!(restored.category_key(), original.category_key());
        }

        #[test]
        fn result_err_round_trips() -> pagable::Result<()> {
            let err = test_error();
            let owned: OwnedFrozen<Value<'static>> =
                OwnedFrozen::build(FrozenHeapName::user("result_err_round_trips"), |heap| {
                    heap.alloc_representation(StarlarkResult::Err(err.dupe()))
                        .to_value()
                });
            let expected_display = owned.by_ref(|v| v.to_string());

            let restored = round_trip(owned)?;

            restored.by_ref(|v| {
                let result = StarlarkResult::from_value(*v).expect("a bxl.Result");
                assert!(!result.is_ok());
                assert_eq!(
                    without_backtrace(&v.to_string()),
                    without_backtrace(&expected_display)
                );
                let restored_err = result.unwrap_err().expect("an Err result");
                assert_same_error(&restored_err.err, &err);
            });
            Ok(())
        }

        #[test]
        fn result_ok_round_trips() -> pagable::Result<()> {
            let owned: OwnedFrozen<Value<'static>> =
                OwnedFrozen::build(FrozenHeapName::user("result_ok_round_trips"), |heap| {
                    heap.alloc_representation(StarlarkResult::Ok(heap.alloc("payload")))
                        .to_value()
                });

            let restored = round_trip(owned)?;

            restored.by_ref(|v| {
                let result = StarlarkResult::from_value(*v).expect("a bxl.Result");
                assert!(result.is_ok());
                let payload = result.unwrap().expect("an Ok result");
                assert_eq!(payload.unpack_str(), Some("payload"));
            });
            Ok(())
        }

        #[test]
        fn error_round_trips() -> pagable::Result<()> {
            let err = test_error();
            let owned: OwnedFrozen<Value<'static>> =
                OwnedFrozen::build(FrozenHeapName::user("error_round_trips"), |heap| {
                    heap.alloc_representation(StarlarkError::new(err.dupe()))
                        .to_value()
                });
            let expected_display = owned.by_ref(|v| v.to_string());

            let restored = round_trip(owned)?;

            restored.by_ref(|v| {
                let restored_err = v.downcast_ref::<StarlarkError>().expect("a bxl.Error");
                assert_eq!(
                    without_backtrace(&v.to_string()),
                    without_backtrace(&expected_display)
                );
                assert_same_error(&restored_err.err, &err);
            });
            Ok(())
        }
    }
}
