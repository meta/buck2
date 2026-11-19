/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::fmt::Write;
use std::hash::Hash;
use std::sync::Arc;

use allocative::Allocative;
use buck2_error::BuckErrorContext;
use buck2_error::ContextValue;
use buck2_error::TypedContext;
use buck2_util::arc_str::ArcStr;
use dupe::Dupe;
use itertools::Either;
use pagable::Pagable;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;
use pagable::ValueSerialize;
use serde::Deserialize;
use serde::Serialize;
use strong_hash::StrongHash;

use crate::provider::label::ProvidersLabel;
use crate::target::configured_target_label::ConfiguredTargetLabel;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = CompatibilityError)]
enum CompatibilityErrors {
    /// Target is immediately incompatible with the configuration.
    #[error("{0:#}")]
    #[buck2(tag = TargetIncompatible)]
    TargetIncompatible(IncompatiblePlatformReason),
    /// Target is compatible but a transitive dependency is not.
    #[error("{0:#}")]
    #[buck2(tag = DepOnlyIncompatible)]
    DepOnlyIncompatible(IncompatiblePlatformReason),
    // We just need this so that the soft error doesn't print a wall of text to stderr
    // TODO(scottcao): Delete this once we are done with migration and made this a hard error
    // everywhere
    #[error(
        "{0:#} does not pass compatibility check (will be error in future) because its transitive dep {1:#}"
    )]
    #[buck2(tag = DepOnlyIncompatible)]
    DepOnlyIncompatibleSoftError(ConfiguredTargetLabel, IncompatiblePlatformReason),
}

/// MaybeCompatible is used to gracefully deal with things that are incompatible
/// with the target platform. The main place this comes up is that targets provided on the
/// cli may be incompatible with the default or requested platform, and we want to skip
/// building those rather than have it be an error.
#[derive(Clone, Dupe, Debug, Eq, PartialEq, Hash, Allocative, Pagable)]
pub enum MaybeCompatible<T> {
    Incompatible(Arc<IncompatiblePlatformReason>),
    Compatible(T),
}

impl<T> MaybeCompatible<T> {
    /// Converts to a result. Incompatible values get converted to an error.
    ///
    /// This is just a convenience for treating incompatibility as an error.
    pub fn require_compatible(self) -> buck2_error::Result<T> {
        match self {
            MaybeCompatible::Incompatible(reason) => Err(reason.to_err()),
            MaybeCompatible::Compatible(result) => Ok(result),
        }
    }

    /// Unwraps the compatible value, panicking with the given message if incompatible.
    pub fn expect_compatible(self, message: &'static str) -> T {
        match self {
            MaybeCompatible::Incompatible(reason) => panic!("{}: {}", message, reason),
            MaybeCompatible::Compatible(result) => result,
        }
    }

    pub fn is_compatible(&self) -> bool {
        matches!(self, Self::Compatible(..))
    }

    pub fn map<U>(self, func: impl FnOnce(T) -> U) -> MaybeCompatible<U> {
        match self {
            MaybeCompatible::Incompatible(e) => MaybeCompatible::Incompatible(e),
            MaybeCompatible::Compatible(t) => MaybeCompatible::Compatible(func(t)),
        }
    }

    pub fn try_map<E, U>(
        self,
        func: impl FnOnce(T) -> Result<U, E>,
    ) -> Result<MaybeCompatible<U>, E> {
        match self {
            MaybeCompatible::Incompatible(e) => Ok(MaybeCompatible::Incompatible(e)),
            MaybeCompatible::Compatible(t) => Ok(MaybeCompatible::Compatible(func(t)?)),
        }
    }

    /// Convert to `ResultMaybeCompatible`, lifting both variants.
    pub fn to_result_maybe_compatible(self) -> ResultMaybeCompatible<T> {
        match self {
            MaybeCompatible::Compatible(t) => ResultMaybeCompatible::Compatible(t),
            MaybeCompatible::Incompatible(e) => ResultMaybeCompatible::Incompatible(e),
        }
    }
}

/// This replaces the common pattern of `Result<MaybeCompatible<T>>` with an enum that
/// will propagate either the Err or Incompatible case with `?`. This just makes code
/// a bit simpler in the common case where we just want to propagate up the incompatibility.
///
/// Use `ok()` to convert back to `Result<MaybeCompatible<T>>` when the caller needs to
/// handle incompatibility explicitly, and `require_compatible()` to convert to
/// `Result<T>` when incompatibility should be treated as an error.
#[derive(Clone, Dupe, Debug, Allocative)]
pub enum ResultMaybeCompatible<T> {
    Compatible(T),
    Incompatible(Arc<IncompatiblePlatformReason>),
    Err(buck2_error::Error),
}

const COMPATIBLE: u8 = 0;
const INCOMPATIBLE: u8 = 1;

impl<T: PagableSerialize> PagableSerialize for ResultMaybeCompatible<T> {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        match self {
            Self::Compatible(v) => {
                COMPATIBLE.serialize(serializer.serde())?;
                v.pagable_serialize(serializer)?;
            }
            Self::Incompatible(reason) => {
                INCOMPATIBLE.serialize(serializer.serde())?;
                reason.pagable_serialize(serializer)?;
            }
            Self::Err(e) => {
                return Err(pagable::anyhow!(
                    "Cannot serialize `ResultMaybeCompatible::Err` {}",
                    e
                ));
            }
        }
        Ok(())
    }
}

impl<'de, T: PagableDeserialize<'de>> PagableDeserialize<'de> for ResultMaybeCompatible<T> {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        let discriminant = u8::deserialize(deserializer.serde())?;
        match discriminant {
            COMPATIBLE => Ok(Self::Compatible(T::pagable_deserialize(deserializer)?)),
            INCOMPATIBLE => Ok(Self::Incompatible(PagableDeserialize::pagable_deserialize(
                deserializer,
            )?)),
            _ => Err(pagable::anyhow!(
                "Invalid `ResultMaybeCompatible` discriminant: {}",
                discriminant
            )),
        }
    }
}

/// [`ValueSerialize`] for [`ResultMaybeCompatible`]: pages out the
/// `Compatible`/`Incompatible` variants and keeps `Err` values in memory.
pub struct ResultMaybeCompatibleValueSerialize<T>(std::marker::PhantomData<T>);

impl<T> ResultMaybeCompatibleValueSerialize<T> {
    pub fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<T: Pagable + Allocative + Dupe + Send + Sync + 'static> ValueSerialize
    for ResultMaybeCompatibleValueSerialize<T>
{
    type Value = ResultMaybeCompatible<T>;

    fn pagable_serialize_value(
        &self,
        v: &Self::Value,
        ser: &mut dyn PagableSerializer,
    ) -> Option<pagable::Result<()>> {
        match v {
            ResultMaybeCompatible::Err(_) => None,
            v => Some(v.pagable_serialize(ser)),
        }
    }

    fn pagable_deserialize_value<'de, D: PagableDeserializer<'de> + ?Sized>(
        &self,
        deser: &mut D,
    ) -> pagable::Result<Self::Value> {
        PagableDeserialize::pagable_deserialize(deser)
    }
}

impl<T> From<buck2_error::Error> for ResultMaybeCompatible<T> {
    fn from(value: buck2_error::Error) -> Self {
        Self::Err(value)
    }
}

impl<T> ResultMaybeCompatible<T> {
    pub fn is_compatible(&self) -> bool {
        matches!(self, Self::Compatible(..))
    }

    /// Borrows the compatible value, leaving the `Incompatible`/`Err` variants shared.
    pub fn as_ref(&self) -> ResultMaybeCompatible<&T> {
        match self {
            Self::Compatible(t) => ResultMaybeCompatible::Compatible(t),
            Self::Incompatible(reason) => ResultMaybeCompatible::Incompatible(reason.dupe()),
            Self::Err(e) => ResultMaybeCompatible::Err(e.clone()),
        }
    }

    pub fn map<U>(self, func: impl FnOnce(T) -> U) -> ResultMaybeCompatible<U> {
        match self {
            Self::Compatible(t) => ResultMaybeCompatible::Compatible(func(t)),
            Self::Incompatible(e) => ResultMaybeCompatible::Incompatible(e),
            Self::Err(e) => ResultMaybeCompatible::Err(e),
        }
    }

    pub fn map_err(
        self,
        func: impl FnOnce(buck2_error::Error) -> buck2_error::Error,
    ) -> ResultMaybeCompatible<T> {
        match self {
            Self::Compatible(t) => ResultMaybeCompatible::Compatible(t),
            Self::Incompatible(e) => ResultMaybeCompatible::Incompatible(e),
            Self::Err(e) => ResultMaybeCompatible::Err(func(e)),
        }
    }

    pub fn try_map<U>(
        self,
        func: impl FnOnce(T) -> Result<U, buck2_error::Error>,
    ) -> ResultMaybeCompatible<U> {
        match self {
            Self::Compatible(t) => ResultMaybeCompatible::Compatible(func(t)?),
            Self::Incompatible(e) => ResultMaybeCompatible::Incompatible(e),
            Self::Err(e) => ResultMaybeCompatible::Err(e),
        }
    }

    /// Unwraps the compatible value, panicking if incompatible or error.
    pub fn expect_compatible(self, message: &'static str) -> T {
        match self {
            ResultMaybeCompatible::Incompatible(reason) => panic!("{message}: {reason}"),
            ResultMaybeCompatible::Compatible(result) => result,
            ResultMaybeCompatible::Err(error) => panic!("{message}: {error:?}"),
        }
    }

    #[track_caller]
    pub fn internal_error(self, message: &str) -> ResultMaybeCompatible<T> {
        self.with_internal_error(|| message.to_owned())
    }

    #[track_caller]
    pub fn with_internal_error<F>(self, f: F) -> ResultMaybeCompatible<T>
    where
        F: FnOnce() -> String,
    {
        match self {
            Self::Compatible(v) => ResultMaybeCompatible::Compatible(v),
            Self::Incompatible(v) => ResultMaybeCompatible::Incompatible(v),
            Self::Err(e) => {
                buck2_error::Result::<!>::Err(e.into()).with_internal_error(f)?;
            }
        }
    }

    #[track_caller]
    pub fn buck_error_context<C: Into<ContextValue>>(self, context: C) -> ResultMaybeCompatible<T> {
        match self {
            Self::Err(e) => ResultMaybeCompatible::Err(e.context(context)),
            Self::Compatible(v) => ResultMaybeCompatible::Compatible(v),
            Self::Incompatible(v) => ResultMaybeCompatible::Incompatible(v),
        }
    }

    #[track_caller]
    pub fn tag(self, tag: buck2_error::ErrorTag) -> ResultMaybeCompatible<T> {
        match self {
            Self::Err(e) => ResultMaybeCompatible::Err(e.tag(vec![tag])),
            Self::Compatible(v) => ResultMaybeCompatible::Compatible(v),
            Self::Incompatible(v) => ResultMaybeCompatible::Incompatible(v),
        }
    }

    #[track_caller]
    pub fn with_buck_error_context<C, F>(self, f: F) -> ResultMaybeCompatible<T>
    where
        C: Into<ContextValue>,
        F: FnOnce() -> C,
    {
        match self {
            Self::Err(e) => ResultMaybeCompatible::Err(e.context(f())),
            Self::Compatible(v) => ResultMaybeCompatible::Compatible(v),
            Self::Incompatible(v) => ResultMaybeCompatible::Incompatible(v),
        }
    }

    #[track_caller]
    pub fn compute_context<
        TC: TypedContext,
        C1: Into<ContextValue>,
        C2: Into<ContextValue>,
        F: FnOnce(Arc<TC>) -> C1,
        F2: FnOnce() -> C2,
    >(
        self,
        map_context: F,
        new_context: F2,
    ) -> ResultMaybeCompatible<T> {
        match self {
            Self::Err(e) => Self::Err(e.compute_context(map_context, new_context)),
            v => v,
        }
    }

    /// Convert incompatibility to an error. Returns `Ok(t)` for `Compatible(t)`,
    /// and `Err(...)` for both `Incompatible` and `Err`.
    pub fn require_compatible(self) -> Result<T, buck2_error::Error> {
        match self {
            ResultMaybeCompatible::Compatible(t) => Ok(t),
            ResultMaybeCompatible::Incompatible(reason) => Err(reason.to_err()),
            ResultMaybeCompatible::Err(e) => Err(e),
        }
    }

    /// Convert to `Result<MaybeCompatible<T>>`, separating the error case from
    /// the compatible/incompatible cases.
    pub fn ok(self) -> Result<MaybeCompatible<T>, buck2_error::Error> {
        match self {
            ResultMaybeCompatible::Compatible(t) => Ok(MaybeCompatible::Compatible(t)),
            ResultMaybeCompatible::Incompatible(reason) => {
                Ok(MaybeCompatible::Incompatible(reason))
            }
            ResultMaybeCompatible::Err(e) => Err(e),
        }
    }
}

impl<T> std::ops::FromResidual<ResultMaybeCompatible<std::convert::Infallible>>
    for ResultMaybeCompatible<T>
{
    fn from_residual(residual: ResultMaybeCompatible<std::convert::Infallible>) -> Self {
        match residual {
            ResultMaybeCompatible::Compatible(_) => unreachable!(),
            ResultMaybeCompatible::Incompatible(i) => ResultMaybeCompatible::Incompatible(i),
            ResultMaybeCompatible::Err(e) => ResultMaybeCompatible::Err(e.into()),
        }
    }
}

impl<T, E: Into<buck2_error::Error>> std::ops::FromResidual<Result<std::convert::Infallible, E>>
    for ResultMaybeCompatible<T>
{
    fn from_residual(residual: Result<std::convert::Infallible, E>) -> Self {
        match residual {
            Ok(_) => unreachable!(),
            Err(e) => ResultMaybeCompatible::Err(e.into()),
        }
    }
}

impl<T> std::ops::Residual<T> for ResultMaybeCompatible<std::convert::Infallible> {
    type TryType = ResultMaybeCompatible<T>;
}

impl<T> std::ops::Try for ResultMaybeCompatible<T> {
    type Output = T;

    type Residual = ResultMaybeCompatible<std::convert::Infallible>;

    fn from_output(output: Self::Output) -> Self {
        Self::Compatible(output)
    }

    fn branch(self) -> std::ops::ControlFlow<Self::Residual, Self::Output> {
        match self {
            Self::Compatible(t) => std::ops::ControlFlow::Continue(t),
            Self::Incompatible(i) => {
                std::ops::ControlFlow::Break(ResultMaybeCompatible::Incompatible(i))
            }
            Self::Err(e) => std::ops::ControlFlow::Break(ResultMaybeCompatible::Err(e)),
        }
    }
}

impl<A, V: FromIterator<A>> FromIterator<ResultMaybeCompatible<A>> for ResultMaybeCompatible<V> {
    fn from_iter<T: IntoIterator<Item = ResultMaybeCompatible<A>>>(iter: T) -> Self {
        match iter
            .into_iter()
            .map(|v| match v {
                ResultMaybeCompatible::Compatible(v) => Ok(v),
                ResultMaybeCompatible::Incompatible(v) => Err(Either::Left(v)),
                ResultMaybeCompatible::Err(v) => Err(Either::Right(v)),
            })
            .collect::<Result<V, _>>()
        {
            Ok(v) => ResultMaybeCompatible::Compatible(v),
            Err(Either::Left(v)) => ResultMaybeCompatible::Incompatible(v),
            Err(Either::Right(v)) => ResultMaybeCompatible::Err(v),
        }
    }
}

#[derive(
    Debug, Eq, PartialEq, Hash, StrongHash, Clone, Dupe, Allocative, Pagable
)]
pub enum IncompatiblePlatformReasonCause {
    /// Target is incompatible because of unsatisfied config setting.
    UnsatisfiedConfig(ProvidersLabel),
    /// Target is incompatible because dependency is incompatible.
    Dependency(Arc<IncompatiblePlatformReason>),
    /// Target is incompatible because a select resolved to select_incompatible().
    SelectIncompatible(ArcStr),
}

#[derive(
    Debug, Eq, PartialEq, Hash, StrongHash, Clone, Dupe, Allocative, Pagable
)]
pub struct IncompatiblePlatformReason {
    pub target: ConfiguredTargetLabel,
    pub cause: IncompatiblePlatformReasonCause,
}

impl IncompatiblePlatformReason {
    pub fn to_err(&self) -> buck2_error::Error {
        match self.cause {
            IncompatiblePlatformReasonCause::UnsatisfiedConfig(_)
            | IncompatiblePlatformReasonCause::SelectIncompatible(_) => {
                CompatibilityErrors::TargetIncompatible(self.dupe()).into()
            }
            IncompatiblePlatformReasonCause::Dependency(_) => {
                CompatibilityErrors::DepOnlyIncompatible(self.dupe()).into()
            }
        }
    }

    pub fn to_soft_err(&self) -> buck2_error::Error {
        match &self.cause {
            IncompatiblePlatformReasonCause::UnsatisfiedConfig(_)
            | IncompatiblePlatformReasonCause::SelectIncompatible(_) => self.to_err(),
            IncompatiblePlatformReasonCause::Dependency(reason) => {
                let root_cause = reason.get_root_cause();
                CompatibilityErrors::DepOnlyIncompatibleSoftError(
                    self.target.dupe(),
                    root_cause.dupe(),
                )
                .into()
            }
        }
    }

    fn get_root_cause(&self) -> &IncompatiblePlatformReason {
        // Recurse until we find the root UnsatisfiedConfig/SelectIncompatible error that caused incompatibility errors
        match &self.cause {
            IncompatiblePlatformReasonCause::UnsatisfiedConfig(_)
            | IncompatiblePlatformReasonCause::SelectIncompatible(_) => self,
            IncompatiblePlatformReasonCause::Dependency(reason) => reason.get_root_cause(),
        }
    }

    pub fn skipping_message(&self, target: &ConfiguredTargetLabel) -> String {
        format!("Skipping target incompatible node `{target}`")
    }

    pub fn skipping_message_for_multiple<'t>(
        incompatible_targets: impl IntoIterator<Item = &'t ConfiguredTargetLabel>,
    ) -> String {
        let mut incompatible_targets = incompatible_targets.into_iter().collect::<Vec<_>>();
        incompatible_targets.sort();
        let mut message = String::new();

        writeln!(
            message,
            "Skipped {} incompatible targets:",
            incompatible_targets.len()
        )
        .unwrap();
        if incompatible_targets.len() < 10 {
            for target in incompatible_targets.iter() {
                writeln!(message, "  {target}").unwrap();
            }
        } else {
            for target in incompatible_targets.iter().take(3) {
                writeln!(message, "  {target}").unwrap();
            }
            writeln!(message, "  ...").unwrap();
            for target in incompatible_targets.iter().rev().take(3).rev() {
                writeln!(message, "  {target}").unwrap();
            }
        }
        message
    }
}

impl Display for IncompatiblePlatformReason {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.cause {
            IncompatiblePlatformReasonCause::UnsatisfiedConfig(unsatisfied_config) => write!(
                f,
                // WARN: CI uses this message to filter targets
                // If you change this message, please also update https://fburl.com/code/nvdg28nv
                "{}\n    is incompatible with {} ({} unsatisfied), check the target's compatibility attributes",
                self.target.unconfigured(),
                self.target.cfg(),
                unsatisfied_config,
            ),
            IncompatiblePlatformReasonCause::SelectIncompatible(message) => write!(
                f,
                "{}\n    is incompatible: select resolved to select_incompatible(): {}",
                self.target.unconfigured(),
                message,
            ),
            IncompatiblePlatformReasonCause::Dependency(previous) => {
                if f.alternate() {
                    write!(f, "{}\n-> {:#}", self.target, previous)
                } else {
                    write!(f, "{} -> {}", self.target, previous)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::configuration::compatibility::IncompatiblePlatformReason;
    use crate::configuration::data::ConfigurationData;
    use crate::target::label::label::TargetLabel;

    #[test]
    fn test_skipping_message_for_multiple() {
        let set = vec![
            TargetLabel::testing_parse("glass//foo:bar")
                .configure(ConfigurationData::testing_new()),
        ];
        assert_eq!(
            format!(
                "Skipped 1 incompatible targets:\n  glass//foo:bar ({})\n",
                ConfigurationData::testing_new()
            ),
            IncompatiblePlatformReason::skipping_message_for_multiple(&set),
        );

        let mut set = Vec::new();
        for i in 0..20 {
            // Test it doesn't crash
            IncompatiblePlatformReason::skipping_message_for_multiple(&set);

            set.push(
                TargetLabel::testing_parse(&format!("plate//foo:bar{i}"))
                    .configure(ConfigurationData::testing_new()),
            );
        }
        assert_eq!(
            format!(
                r"Skipped 20 incompatible targets:
  plate//foo:bar0 ({c})
  plate//foo:bar1 ({c})
  plate//foo:bar10 ({c})
  ...
  plate//foo:bar7 ({c})
  plate//foo:bar8 ({c})
  plate//foo:bar9 ({c})
",
                c = ConfigurationData::testing_new()
            ),
            IncompatiblePlatformReason::skipping_message_for_multiple(&set)
        );
    }
}
