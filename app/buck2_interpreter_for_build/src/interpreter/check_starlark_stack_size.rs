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
use buck2_error::BuckErrorContext;
use buck2_error::starlark_error::from_starlark_with_options;
use buck2_interpreter::dice::starlark_provider::StarlarkEvalKind;
use buck2_interpreter::factory::BuckStarlarkModule;
use buck2_interpreter::factory::StarlarkEvaluatorProvider;
use buck2_interpreter::file_type::StarlarkFileType;
use dice::DiceComputations;
use dice::EqualityBehavior;
use dice::Key;
use dice::OkPagableValueSerialize;
use dice::ValueSerialize;
use dice_futures::cancellation::CancellationContext;
use dupe::ResultDupedErrExt;
use indoc::indoc;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark::environment::Globals;
use starlark::syntax::AstModule;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Tier0)]
enum CheckStarlarkStackSizeError {
    #[error("Error checking starlark stack size")]
    CheckStarlarkStackSizeError,
}

// In order to prevent non deterministic crashes
// we intentionally set off a starlark stack overflow, to make
// sure that starlark catches the overflow and reports an error
// before the native stack overflows
pub(crate) async fn check_starlark_stack_size(
    ctx: &mut DiceComputations<'_>,
) -> buck2_error::Result<()> {
    #[derive(
        Debug,
        derive_more::Display,
        Clone,
        Allocative,
        Eq,
        PartialEq,
        Hash,
        Pagable
    )]
    #[pagable_typetag(dice::DiceKeyDyn)]
    struct StarlarkStackSizeChecker;

    #[async_trait]
    impl Key for StarlarkStackSizeChecker {
        type Value = buck2_error::Result<()>;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            cancellation: &CancellationContext,
        ) -> Self::Value {
            let eval_kind = StarlarkEvalKind::Unknown("Check starlark stack size".into());
            let provider = StarlarkEvaluatorProvider::new(ctx, eval_kind).await?;

            BuckStarlarkModule::with_profiling(|env| {
                let (finished_eval, _) =
                    provider.with_evaluator(&env, cancellation.into(), move |eval, _| {
                        let content = indoc!(
                            r#"
                                def f():
                                    f()
                                f()
                        "#
                        );
                        let ast = AstModule::parse(
                            "x.star",
                            content.to_owned(),
                            &StarlarkFileType::Bzl.dialect(false),
                        )
                        .map_err(|e| {
                            from_starlark_with_options(
                                e,
                                buck2_error::starlark_error::NativeErrorHandling::Unknown,
                                false,
                            )
                        })
                        .internal_error("Failed to parse check module")?;
                        match eval.eval_module(ast, &Globals::standard()) {
                            Err(e) if e.to_string().contains("Starlark call stack overflow") => {
                                Ok(())
                            }
                            Err(p) => Err(from_starlark_with_options(
                                p,
                                buck2_error::starlark_error::NativeErrorHandling::Unknown,
                                false,
                            )),
                            Ok(_) => {
                                Err(CheckStarlarkStackSizeError::CheckStarlarkStackSizeError.into())
                            }
                        }
                    })?;
                Ok((finished_eval.finish()?.0, ()))
            })
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

    ctx.compute(&StarlarkStackSizeChecker)
        .await?
        .as_ref()
        .duped_err()
        .map(|_| ())
}
