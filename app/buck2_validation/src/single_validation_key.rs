/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use async_trait::async_trait;
use buck2_artifact::actions::key::ActionKey;
use buck2_build_api::actions::artifact::get_artifact_fs::GetArtifactFs;
use buck2_build_api::actions::calculation::ActionCalculation;
use buck2_build_api::materialize::invocation_re_use_case;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::ErrorTag;
use buck2_execute::materialize::materializer::HasMaterializer;
use buck2_execute::materialize::materializer::MaterializationPurpose;
use buck2_execute::materialize::materializer::MaterializeRequest;
use buck2_fs::async_fs_util;
use buck2_fs::error::IoResultExt;
use derive_more::Display;
use dice::CancellationContext;
use dice::DiceComputations;
use dice::EqualityBehavior;
use dice::Key;
use dice::OkPagableValueSerialize;
use dice::ValueSerialize;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;

use crate::cached_validation_result::CachedValidationResult;
use crate::validator_api::parse_validation_result;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Tier0)]
enum ParseValidationResultError {
    #[error("Validation result should produce exactly one artifact")]
    WrongNumberOfArtifacts,
}

/// DICE key that corresponds to a single validation, represented by a ValidationSpec object.
/// Computation is the whole process of:
/// 1) Building the validation result artifact.
/// 2) Materializing it.
/// 3) Reading and parsing it to produce result which could be cached in DICE.
#[derive(Clone, Display, Dupe, Debug, Eq, PartialEq, Hash, Allocative, Pagable)]
#[repr(transparent)]
#[pagable_typetag(dice::DiceKeyDyn)]
pub(crate) struct SingleValidationKey(pub ActionKey);

#[async_trait]
impl Key for SingleValidationKey {
    type Value = buck2_error::Result<CachedValidationResult>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellation: &CancellationContext,
    ) -> Self::Value {
        let build_result = ActionCalculation::build_action(ctx, &self.0).await?;
        let (gen_path, artifact_value) = {
            if build_result.iter().count() != 1 {
                return Err(buck2_error::Error::from(
                    ParseValidationResultError::WrongNumberOfArtifacts,
                ));
            }
            let (gen_path, artifact_value) = build_result
                .iter()
                .next()
                .internal_error("Just checked single element")?;
            (gen_path.dupe(), artifact_value)
        };

        let fs = ctx.get_artifact_fs().await?;
        let project_relative_path = fs.buck_out_path_resolver().resolve_gen(
            &gen_path,
            if gen_path.is_content_based_path() {
                Some(artifact_value.content_based_path_hash())
            } else {
                None
            }
            .as_ref(),
        )?;

        let validation_result_path = fs.fs().resolve(&project_relative_path);

        // Make sure validation result is materialized before we parse it
        let re_use_case = invocation_re_use_case(ctx);
        let response = ctx
            .per_transaction_data()
            .get_materializer()
            .materialize(MaterializeRequest {
                artifacts: vec![(project_relative_path, artifact_value.dupe())],
                outputs: Vec::new(),
                purpose: MaterializationPurpose::IntermediateOnly,
                re_use_case,
            })
            .await?;
        for result in response.results {
            result?;
        }
        // Held while the result is read.
        let _lease = response.lease;

        let content = async_fs_util::read_to_string(&validation_result_path)
            .await
            .categorize_tagged(ErrorTag::ValidationResultRead)
            .with_buck_error_context(|| {
                format!(
                    "Reading validation result for `{}` located at: `{}`",
                    self.0.owner(),
                    validation_result_path
                )
            })?;

        // The error is reported under the target that was requested, which may be far from
        // the validation's owner: name the owner and the file.
        let r = parse_validation_result(&content).with_buck_error_context(|| {
            format!(
                "Parsing validation result for `{}` located at: `{}`",
                self.0.owner(),
                validation_result_path
            )
        })?;
        Ok(CachedValidationResult::new(
            r,
            self.0.owner().dupe(),
            validation_result_path,
        ))
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| match (x, y) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        })
    }

    fn validity(x: &Self::Value) -> bool {
        x.is_ok()
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        OkPagableValueSerialize::<Self::Value>::new()
    }
}
