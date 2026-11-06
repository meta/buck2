/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use async_trait::async_trait;
use buck2_build_api::analysis::calculation::RuleAnalysisCalculation;
use buck2_build_api::transition::TRANSITION_ATTRS_PROVIDER;
use buck2_build_api::transition::TransitionAttrProvider;
use buck2_core::configuration::transition::id::TransitionId;
use buck2_core::provider::label::ProvidersLabel;
use buck2_interpreter::load_module::InterpreterCalculation;
use dice::DiceComputations;
use dice::EqualityBehavior;
use dice::Key;
use dice::OkPagableValueSerialize;
use dice::ValueSerialize;
use dupe::ResultDupedErrExt;
use either::Either;
use futures::FutureExt;
use futures::future::BoxFuture;
use pagable::Pagable;
use pagable::pagable_typetag;
use ref_cast::RefCast;

use crate::transition::provider::OwnedTransitionInfo;
use crate::transition::provider::TransitionInfo;
use crate::transition::starlark::FrozenTransition;
use crate::transition::starlark::OwnedTransition;

pub(crate) enum TransitionData {
    MagicObject(OwnedTransition),
    Target(OwnedTransitionInfo),
}

impl TransitionData {
    /// The transition's `refs`, as `(name, target)` pairs.
    ///
    /// The iterator holds heap-branded values and so is not `Send`; collect it before crossing
    /// an await.
    pub(crate) fn refs(&self) -> impl Iterator<Item = (&str, &ProvidersLabel)> {
        match self {
            TransitionData::MagicObject(v) => Either::Left(
                v.as_ref()
                    .value()
                    .as_ref()
                    .refs
                    .iter()
                    .map(|(name, target)| (name.as_str(), target)),
            ),
            TransitionData::Target(_) => Either::Right([].into_iter()),
        }
        .into_iter()
    }

    pub(crate) fn attr_names(&self) -> Option<impl Iterator<Item = buck2_error::Result<&str>>> {
        match self {
            TransitionData::MagicObject(v) => Some(Either::Left(
                v.as_ref()
                    .value()
                    .as_ref()
                    .attrs_names
                    .as_ref()?
                    .iter()
                    .map(|s| Ok(s.as_str())),
            )),
            TransitionData::Target(v) => Some(Either::Right(
                v.as_ref().value().as_ref().get_attrs_names()?,
            )),
        }
    }

    pub(crate) fn is_split(&self) -> bool {
        match self {
            TransitionData::MagicObject(v) => v.as_ref().value().as_ref().split,
            TransitionData::Target(_) => false,
        }
    }
}

/// Fetch transition object (function plus context) by id.
#[async_trait]
pub(crate) trait FetchTransition {
    /// Fetch transition object by id.
    async fn fetch_transition(&mut self, id: &TransitionId) -> buck2_error::Result<TransitionData>;
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum FetchTransitionError {
    #[error("Transition object not found by id {:?}", _0)]
    NotFound(TransitionId),
    #[error("Expected `{0}` to be a transition target, but it had no `TransitionInfo` provider.")]
    MissingTransitionInfo(ProvidersLabel),
}

#[async_trait]
impl FetchTransition for DiceComputations<'_> {
    async fn fetch_transition(&mut self, id: &TransitionId) -> buck2_error::Result<TransitionData> {
        match id {
            TransitionId::MagicObject { path, name } => {
                let module = self.get_loaded_module_from_import_path(path).await?;
                let transition = module
                    .env()
                    // This is a hashmap lookup, so we are not caching the result in DICE.
                    .get_any_visibility(name)
                    .map_err(|_| {
                        buck2_error::Error::from(FetchTransitionError::NotFound(id.clone()))
                    })?
                    .0;

                Ok(TransitionData::MagicObject(
                    transition.downcast_starlark::<FrozenTransition<'static>>()?,
                ))
            }
            TransitionId::Target(label) => {
                let transition_info = self
                    .get_configuration_analysis_result(label)
                    .await?
                    .builtin_provider_value::<TransitionInfo>()?
                    .ok_or_else(|| FetchTransitionError::MissingTransitionInfo(label.clone()))?;
                Ok(TransitionData::Target(transition_info))
            }
        }
    }
}

/// Computes the attributes required by a transition.
///
/// This basically only exists so that we have a place to hand out the transition's attribute names
/// as owned `String`s, as we cannot directly return the `FrozenStarlarkStr`s that are actually
/// stored to crates that avoid depending on starlark.
#[derive(
    Debug,
    Eq,
    PartialEq,
    Hash,
    Clone,
    derive_more::Display,
    allocative::Allocative,
    ref_cast::RefCast,
    Pagable
)]
#[display("{}", _0)]
#[repr(transparent)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct TransitionAttrsKey(TransitionId);

#[async_trait]
impl Key for TransitionAttrsKey {
    type Value = buck2_error::Result<Option<Box<[String]>>>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellation: &dice::CancellationContext,
    ) -> Self::Value {
        ctx.fetch_transition(&self.0)
            .await?
            .attr_names()
            .map(|names| names.map(|s| Ok(s?.to_owned())).collect())
            .transpose()
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| {
            if let (Ok(x), Ok(y)) = (x, y) {
                x == y
            } else {
                false
            }
        })
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        OkPagableValueSerialize::<Self::Value>::new()
    }
}

struct TransitionGetAttrs;

impl TransitionAttrProvider for TransitionGetAttrs {
    fn transition_attrs<'a, 'd>(
        &self,
        ctx: &'a mut DiceComputations<'d>,
        transition_id: &'a TransitionId,
    ) -> BoxFuture<'a, buck2_error::Result<Option<&'d [String]>>>
    where
        'd: 'a,
    {
        async move {
            let k = TransitionAttrsKey::ref_cast(transition_id);
            Ok(ctx.compute(k).await?.as_ref().duped_err()?.as_deref())
        }
        .boxed()
    }
}

pub(crate) fn init_transition_attr_provider() {
    TRANSITION_ATTRS_PROVIDER.init(&TransitionGetAttrs);
}
