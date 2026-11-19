/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_core::configuration::data::ConfigurationData;
use buck2_util::late_binding::LateBinding;
use dice::DiceComputations;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use pagable::Pagable;
use pagable::PagableTagged;
use pagable::pagable_typetag;

use crate::metadata::key::MetadataKeyRef;
use crate::modifiers::CfgModifiers;
use crate::nodes::unconfigured::TargetNodeRef;
use crate::rule_type::RuleType;
use crate::super_package::SuperPackage;

/// Trait for configuration constructor functions.
/// The output of invoking these functions is a PlatformInfo
#[pagable_typetag]
#[async_trait]
pub trait CfgConstructorImpl: PagableTagged + Send + Sync + Debug + Allocative {
    /// Evaluates the configuration constructor to resolve modifiers and produce a configuration data.
    ///
    /// # Arguments
    /// * `configuring_exec_dep` - When `true`, indicates this target is being configured as an
    ///   execution dependency (exec_dep). This flag is passed to the Starlark cfg_constructor
    ///   to enable execution specific modifier resolution.
    fn eval<'a>(
        &'a self,
        ctx: &'a mut DiceComputations,
        cfg: &'a ConfigurationData,
        package_cfg_modifiers: Option<&'a CfgModifiers>,
        target_cfg_modifiers: Option<&'a CfgModifiers>,
        cli_modifiers: &'a [String],
        rule_type: &'a RuleType,
        configuring_exec_dep: bool,
        cancellation: &'a CancellationContext,
    ) -> Pin<Box<dyn Future<Output = buck2_error::Result<ConfigurationData>> + Send + 'a>>;

    /// Returns the metadata key used to encode modifiers in PACKAGE values and metadata attribute
    fn key(&self) -> &MetadataKeyRef;
}

pub static CFG_CONSTRUCTOR_CALCULATION_IMPL: LateBinding<
    &'static dyn CfgConstructorCalculationImpl,
> = LateBinding::new("CFG_CONSTRUCTOR_CALCULATION_IMPL");

/// Which platform's configuration the cfg constructor is being invoked for. This determines the
/// highest-priority modifiers, which apply above both package-level and target-level modifiers.
#[derive(Clone, Dupe, Debug, Eq, PartialEq, Hash, Allocative, Pagable)]
pub enum CfgConstructorModifiers {
    /// A regular target's target platform is being configured: modifiers provided on the command
    /// line apply.
    TargetPlatform(Arc<Vec<String>>),
    /// An exec dep's execution platform is being configured: the platform's own constraint values
    /// apply, so that package-level and target-level modifiers on the exec dep cannot override
    /// them. They are a pure function of the configuration the constructor is invoked with, so
    /// they are derived from it at evaluation time rather than stored here.
    ExecPlatform,
}

impl CfgConstructorModifiers {
    pub fn configuring_exec_dep(&self) -> bool {
        match self {
            CfgConstructorModifiers::TargetPlatform(_) => false,
            CfgConstructorModifiers::ExecPlatform => true,
        }
    }

    /// Whether there are no highest-priority modifiers to apply. `cfg` must be the configuration
    /// the constructor is invoked with, from which `ExecPlatform` modifiers are derived.
    pub fn is_empty(&self, cfg: &ConfigurationData) -> buck2_error::Result<bool> {
        Ok(match self {
            CfgConstructorModifiers::TargetPlatform(cli_modifiers) => cli_modifiers.is_empty(),
            CfgConstructorModifiers::ExecPlatform => cfg.data()?.constraints.is_empty(),
        })
    }

    /// The highest-priority modifiers, rendered as modifier strings. `cfg` must be the
    /// configuration the constructor is invoked with; for `ExecPlatform` the strings are its
    /// constraint values.
    ///
    /// They are passed through the `cli_modifiers` argument of `CfgConstructorImpl::eval` because
    /// that position has the highest priority in modifier resolution. This is an implementation
    /// detail — they are NOT user-provided CLI modifiers.
    pub fn render(&self, cfg: &ConfigurationData) -> buck2_error::Result<Cow<'_, [String]>> {
        Ok(match self {
            CfgConstructorModifiers::TargetPlatform(cli_modifiers) => {
                Cow::Borrowed(cli_modifiers.as_slice())
            }
            CfgConstructorModifiers::ExecPlatform => Cow::Owned(
                cfg.data()?
                    .constraints
                    .values()
                    .map(|value| value.to_string())
                    .collect(),
            ),
        })
    }
}

#[async_trait]
pub trait CfgConstructorCalculationImpl: Send + Sync + 'static {
    /// Invokes starlark cfg constructors on provided configuration
    /// and returns the result.
    async fn eval_cfg_constructor(
        &self,
        ctx: &mut DiceComputations<'_>,
        target: TargetNodeRef<'_>,
        super_package: &SuperPackage,
        cfg: ConfigurationData,
        modifiers: CfgConstructorModifiers,
        rule_name: &RuleType,
    ) -> buck2_error::Result<ConfigurationData>;
}
