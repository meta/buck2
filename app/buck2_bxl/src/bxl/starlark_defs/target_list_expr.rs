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
use std::iter;

use allocative::Allocative;
use buck2_build_api::configure_targets::get_maybe_compatible_targets;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::paths::CellRelativePath;
use buck2_core::configuration::compatibility::IncompatiblePlatformReason;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_core::pattern::pattern::ParsedPattern;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_error::BuckErrorContext;
use buck2_hash::BuckMutInsertionSet;
use buck2_interpreter::types::target_label::StarlarkConfiguredTargetLabel;
use buck2_interpreter::types::target_label::StarlarkTargetLabel;
use buck2_node::load_patterns::MissingTargetBehavior;
use buck2_node::load_patterns::load_patterns;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::configured_frontend::ConfiguredTargetNodeCalculation;
use buck2_node::nodes::frontend::TargetGraphCalculation;
use buck2_node::nodes::unconfigured::TargetNode;
use buck2_node::target_calculation::ConfiguredTargetCalculation;
use buck2_query::query::environment::QueryTarget;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use buck2_util::truncate::truncate;
use dice::DiceComputations;
use dupe::Dupe;
use dupe::IterDupedExt;
use either::Either;
use starlark::values::Heap;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::list::ListType;
use starlark::values::type_repr::StarlarkTypeRepr;

use crate::bxl::starlark_defs::context::BxlContext;
use crate::bxl::starlark_defs::context::BxlContextCoreData;
use crate::bxl::starlark_defs::context::ErrorPrinter;
use crate::bxl::starlark_defs::eval_extra::BxlEvalExtra;
use crate::bxl::starlark_defs::nodes::configured::StarlarkConfiguredTargetNode;
use crate::bxl::starlark_defs::nodes::unconfigured::StarlarkTargetNode;
use crate::bxl::starlark_defs::target_expr::TargetExpr;
use crate::bxl::starlark_defs::targetset::StarlarkTargetSet;

/// TargetExpr is just a simple type that can be used in starlark_module
/// functions for arguments that should be target sets. It will accept a
/// literal (like `//some:target`) or list of literals or a TargetSet Value (from one of the
/// BXL functions that return them).
pub(crate) enum TargetListExpr<'v, Node: QueryTarget> {
    One(TargetExpr<'v, Node>),
    Iterable(Vec<TargetExpr<'v, Node>>),
    TargetSet(Cow<'v, TargetSet<Node>>),
}

// Filters out incompatible targets and emits the error message
pub(crate) fn filter_incompatible<T: ErrorPrinter>(
    targets: impl IntoIterator<Item = MaybeCompatible<ConfiguredTargetNode>>,
    error_printer: &T,
) -> buck2_error::Result<TargetSet<ConfiguredTargetNode>> {
    let mut target_set = TargetSet::new();
    let mut incompatible_targets = BuckMutInsertionSet::new();

    for res in targets {
        match res {
            MaybeCompatible::Incompatible(reason) => {
                incompatible_targets.insert(reason.target.dupe());
            }
            MaybeCompatible::Compatible(target) => {
                target_set.insert(target);
            }
        }
    }

    if !incompatible_targets.is_empty() {
        error_printer.print_to_error_stream(
            IncompatiblePlatformReason::skipping_message_for_multiple(incompatible_targets.iter()),
        )?;
    }

    Ok(target_set)
}

fn collect_and_filter_targets<T: ErrorPrinter>(
    results: impl IntoIterator<Item = ResultMaybeCompatible<ConfiguredTargetNode>>,
    keep_going: bool,
    error_printer: &T,
) -> buck2_error::Result<TargetSet<ConfiguredTargetNode>> {
    let maybe_compatible: Vec<_> = if keep_going {
        results.into_iter().filter_map(|r| r.ok().ok()).collect()
    } else {
        results
            .into_iter()
            .map(|r| r.ok())
            .collect::<buck2_error::Result<_>>()?
    };
    filter_incompatible(maybe_compatible, error_printer)
}

#[derive(StarlarkTypeRepr, UnpackValue)]
pub(crate) enum TargetNodeOrTargetLabel<'v> {
    TargetNode(&'v StarlarkTargetNode),
    TargetLabel(&'v StarlarkTargetLabel),
}

impl<'v> TargetNodeOrTargetLabel<'v> {
    fn label(&self) -> &'v TargetLabel {
        match self {
            TargetNodeOrTargetLabel::TargetNode(node) => node.0.label(),
            TargetNodeOrTargetLabel::TargetLabel(label) => label.label(),
        }
    }
}

#[derive(StarlarkTypeRepr, UnpackValue)]
pub(crate) enum TargetNodeOrTargetLabelOrStr<'v> {
    TargetNode(&'v StarlarkTargetNode),
    TargetLabel(&'v StarlarkTargetLabel),
    Str(&'v str),
}

#[derive(StarlarkTypeRepr, UnpackValue)]
pub(crate) enum ConfiguredTargetNodeArg<'v> {
    ConfiguredTargetNode(&'v StarlarkConfiguredTargetNode),
    ConfiguredTargetLabel(&'v StarlarkConfiguredTargetLabel),
    Str(&'v str),
    Unconfigured(TargetNodeOrTargetLabel<'v>),
}

#[derive(StarlarkTypeRepr, UnpackValue)]
pub(crate) enum TargetSetOrTargetList<'v> {
    TargetSet(&'v StarlarkTargetSet<TargetNode>),
    TargetList(ValueOf<'v, ListType<ValueOf<'v, TargetNodeOrTargetLabelOrStr<'v>>>>),
}

#[derive(StarlarkTypeRepr, UnpackValue)]
pub(crate) enum ConfiguredTargetListArg<'v> {
    ConfiguredTargetSet(&'v StarlarkTargetSet<ConfiguredTargetNode>),
    TargetSet(&'v StarlarkTargetSet<TargetNode>),
    TargetList(ValueOf<'v, ListType<ConfiguredTargetNodeArg<'v>>>),
}

#[derive(StarlarkTypeRepr, UnpackValue)]
pub(crate) enum TargetListExprArg<'v> {
    Target(TargetNodeOrTargetLabelOrStr<'v>),
    List(TargetSetOrTargetList<'v>),
}

#[derive(StarlarkTypeRepr, UnpackValue)]
pub(crate) enum ConfiguredTargetListExprArg<'v> {
    Target(ConfiguredTargetNodeArg<'v>),
    List(ValueOf<'v, ConfiguredTargetListArg<'v>>),
}

impl<'v> TargetListExpr<'v, TargetNode> {
    pub(crate) fn iter(&self) -> impl ExactSizeIterator<Item = TargetExpr<'v, TargetNode>> {
        match &self {
            Self::One(one) => Either::Left(Either::Left(iter::once(one.clone()))),
            Self::Iterable(iterable) => Either::Left(Either::Right(iterable.iter().cloned())),
            Self::TargetSet(target_set) => {
                Either::Right(target_set.iter().map(|s| TargetExpr::Node(s.clone())))
            }
        }
    }

    /// Get a `TargetSet<TargetNode>` from the `TargetExpr`
    pub(crate) async fn get(
        self,
        ctx: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<Cow<'v, TargetSet<TargetNode>>> {
        let set = ctx
            .try_compute_join(self.iter(), async |ctx, node_or_ref| {
                node_or_ref.get_from_dice(ctx).await
            })
            .await?
            .into_iter()
            .collect();
        Ok(Cow::Owned(set))
    }

    /// Get a single `TargetNode`
    pub(crate) async fn get_one(
        &self,
        ctx: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<Option<TargetNode>> {
        Ok(match &self {
            Self::One(node_or_ref) => Some(node_or_ref.get_from_dice(ctx).await?),
            _ => None,
        })
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
pub(crate) enum TargetExprError {
    #[error(
        "Expected a single target like item, but was `{0}`. If you have passed in a `label`, make sure to call `configured_target()` to get the underlying configured target label."
    )]
    NotATarget(String),
}

impl<'v> TargetListExpr<'v, ConfiguredTargetNode> {
    fn iter(&self) -> Box<dyn ExactSizeIterator<Item = TargetExpr<'v, ConfiguredTargetNode>> + '_> {
        match &self {
            Self::One(one) => Box::new(iter::once(one.clone())),
            Self::Iterable(iterable) => Box::new(iterable.iter().cloned()),
            Self::TargetSet(target_set) => {
                Box::new(target_set.iter().map(|s| TargetExpr::Node(s.clone())))
            }
        }
    }

    /// Get a vector of maybe compatible `ConfiguredTargetNode`s from the `TargetExpr`.
    /// Any callers of this function will need to call `filter_incompatible()` on the result
    /// in order to get the `TargetSet<ConfiguredTargetNode>`.
    pub(crate) async fn get(
        self,
        dice: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<Vec<MaybeCompatible<ConfiguredTargetNode>>> {
        dice.compute_join(self.iter(), async |ctx, node_or_ref| {
            ctx.get_configured_target_node(node_or_ref.node_ref())
                .await
                .ok()
                .map(|n| n.map(|n| n.dupe()))
        })
        .await
        .into_iter()
        .collect()
    }

    /// Get a single maybe compatible `ConfiguredTargetNode`.
    pub(crate) async fn get_one(
        &self,
        dice: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<Option<MaybeCompatible<ConfiguredTargetNode>>> {
        Ok(match &self {
            Self::One(node_or_ref) => Some(
                dice.get_configured_target_node(node_or_ref.node_ref())
                    .await
                    .ok()?
                    .map(|n| n.dupe()),
            ),
            _ => None,
        })
    }

    pub(crate) fn as_provider_labels(&self) -> Vec<ConfiguredProvidersLabel> {
        self.iter()
            .map(|e| ConfiguredProvidersLabel::default_for(e.node_ref().dupe()))
            .collect()
    }

    pub(crate) async fn unpack_opt(
        arg: ConfiguredTargetListExprArg<'v>,
        global_cfg_options: &GlobalCfgOptions,
        ctx: &BxlContext<'v>,
        dice: &mut DiceComputations<'_>,
        keep_going: bool,
    ) -> buck2_error::Result<TargetListExpr<'v, ConfiguredTargetNode>> {
        match arg {
            ConfiguredTargetListExprArg::Target(arg) => {
                Self::unpack_literal(arg, global_cfg_options, ctx, dice, keep_going).await
            }
            ConfiguredTargetListExprArg::List(arg) => {
                Self::unpack_iterable(arg, global_cfg_options, ctx, dice, keep_going).await
            }
        }
    }

    /// FIXME(JakobDegen): The cquery callers of this should reject an unconfigured target that
    /// arrives without a target platform instead of configuring it with the default one;
    /// `configured_targets()` is the caller that legitimately accepts them.
    pub(crate) async fn unpack(
        // TODO(nga): this does not accept unconfigured targets, so should be narrower type here.
        arg: ConfiguredTargetListExprArg<'v>,
        global_cfg_options: &GlobalCfgOptions,
        ctx: &BxlContext<'v>,
        dice: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<TargetListExpr<'v, ConfiguredTargetNode>> {
        Self::unpack_opt(arg, global_cfg_options, ctx, dice, false).await
    }

    /// When `keep_going` is true, resolution errors cause the target to be
    /// skipped. For string literals, errors are reported via
    /// `print_to_error_stream` (see `unpack_string_literal`).
    async fn unpack_literal(
        arg: ConfiguredTargetNodeArg<'v>,
        global_cfg_options: &GlobalCfgOptions,
        ctx: &BxlContext<'_>,
        dice: &mut DiceComputations<'_>,
        keep_going: bool,
    ) -> buck2_error::Result<TargetListExpr<'v, ConfiguredTargetNode>> {
        let result = match arg {
            ConfiguredTargetNodeArg::ConfiguredTargetNode(configured_target) => {
                Ok(Self::One(TargetExpr::Node(configured_target.0.dupe())))
            }
            ConfiguredTargetNodeArg::ConfiguredTargetLabel(configured_target) => Ok(
                TargetListExpr::One(TargetExpr::Label(Cow::Borrowed(configured_target.label()))),
            ),
            ConfiguredTargetNodeArg::Str(s) => {
                Self::unpack_string_literal(s, global_cfg_options, ctx, dice, keep_going).await
            }
            ConfiguredTargetNodeArg::Unconfigured(label) => dice
                .get_configured_target(label.label(), global_cfg_options)
                .await
                .map(|configured| TargetListExpr::One(TargetExpr::Label(Cow::Owned(configured)))),
        };

        if !keep_going {
            return result;
        }

        // For keep_going: skip targets that fail to resolve, and verify
        // labels can be fetched as nodes.
        let expr = result.unwrap_or(Self::Iterable(Vec::new()));
        if let TargetListExpr::One(TargetExpr::Label(label)) = &expr {
            if dice.get_configured_target_node(label).await.ok().is_err() {
                return Ok(Self::Iterable(Vec::new()));
            }
        }
        Ok(expr)
    }

    // Unpack functionality for a string literal, with keep_going support
    async fn unpack_string_literal(
        val: &str,
        global_cfg_options: &GlobalCfgOptions,
        ctx: &BxlContext<'_>,
        dice: &mut DiceComputations<'_>,
        keep_going: bool,
    ) -> buck2_error::Result<TargetListExpr<'v, ConfiguredTargetNode>> {
        match ParsedPattern::<TargetPatternExtra>::parse_relaxed(
            ctx.target_alias_resolver(),
            // TODO(nga): Parse relaxed relative to cell root is incorrect.
            CellPath::new(ctx.cell_name(), CellRelativePath::empty()),
            val,
            ctx.cell_resolver(),
            ctx.cell_alias_resolver(),
        )? {
            ParsedPattern::Target(pkg, name, TargetPatternExtra) => {
                let result = match dice
                    .get_configured_target(
                        &TargetLabel::new(pkg, name.as_ref()),
                        global_cfg_options,
                    )
                    .await
                {
                    Ok(label) => {
                        // check if we can get a maybe compatible configured target node successfully here to make
                        // sure keep_going works. We will try to get the node later, but due to how complex this
                        // code is, it's much easier to just call it once here as a sanity check.
                        match dice.get_configured_target_node(&label).await.ok() {
                            Ok(_) => Ok(TargetListExpr::One(TargetExpr::Label(Cow::Owned(label)))),
                            Err(e) => Err(e),
                        }
                    }
                    Err(e) => Err(e),
                };

                result.or_else(|e| {
                    if keep_going {
                        Ok(TargetListExpr::Iterable(Vec::new()))
                    } else {
                        Err(e)
                    }
                })
            }
            pattern => {
                let loaded_patterns =
                    load_patterns(dice, vec![pattern], MissingTargetBehavior::Fail).await?;

                // TODO: Both package errors (_package_errors) and target errors (from the iterator)
                // are currently discarded when even when keep_going = true. BXL should report these errors like
                // ctargets does.
                let (maybe_compatible_iter, _package_errors) = get_maybe_compatible_targets(
                    dice,
                    loaded_patterns.iter_loaded_targets_by_package(),
                    global_cfg_options,
                    keep_going,
                )
                .await?;

                let result = collect_and_filter_targets(maybe_compatible_iter, keep_going, ctx)?;
                Ok(TargetListExpr::TargetSet(Cow::Owned(result)))
            }
        }
    }

    async fn unpack_iterable(
        value: ValueOf<'v, ConfiguredTargetListArg<'v>>,
        global_cfg_options: &GlobalCfgOptions,
        ctx: &BxlContext<'_>,
        dice: &mut DiceComputations<'_>,
        keep_going: bool,
    ) -> buck2_error::Result<TargetListExpr<'v, ConfiguredTargetNode>> {
        match value.try_unpack()? {
            ConfiguredTargetListArg::ConfiguredTargetSet(s) => {
                Ok(Self::TargetSet(Cow::Borrowed(s)))
            }
            ConfiguredTargetListArg::TargetSet(s) => {
                let results: Vec<ResultMaybeCompatible<ConfiguredTargetNode>> = dice
                    .compute_join(s.0.iter(), async |dice, node| {
                        let label = dice
                            .get_configured_target(node.label(), global_cfg_options)
                            .await?;
                        dice.get_configured_target_node(&label)
                            .await
                            .map(|n| n.dupe())
                    })
                    .await;

                let target_set = collect_and_filter_targets(results, keep_going, ctx)?;
                Ok(TargetListExpr::TargetSet(Cow::Owned(target_set)))
            }
            ConfiguredTargetListArg::TargetList(unpack) => {
                let mut resolved = vec![];

                // FIXME(JakobDegen): These iterations don't run in parallel, which is pretty sad.
                // Unfortunately, that's also not easy to fix because for some reason this code
                // prints to console
                for item in unpack.try_items() {
                    let item = item?;
                    let unpacked =
                        match Self::unpack_literal(item, global_cfg_options, ctx, dice, keep_going)
                            .await
                        {
                            Ok(expr) => expr,
                            Err(_) if keep_going => continue,
                            Err(e) => return Err(e),
                        };

                    match unpacked {
                        TargetListExpr::One(node) => resolved.push(node),
                        TargetListExpr::TargetSet(set) => match set {
                            Cow::Borrowed(s) => itertools::Either::Left(s.iter().duped()),
                            Cow::Owned(s) => itertools::Either::Right(s.into_iter()),
                        }
                        .for_each(|t| resolved.push(TargetExpr::Node(t))),
                        TargetListExpr::Iterable(items) => {
                            if keep_going {
                                resolved.extend(items);
                            } else {
                                let error: buck2_error::Error =
                                    TargetExprError::NotATarget(value.to_value().to_repr()).into();
                                return Err(error.context(format!(
                                    "Error resolving list `{}`",
                                    truncate(&value.to_value().to_repr(), 150)
                                )));
                            }
                        }
                    }
                }

                Ok(Self::Iterable(resolved))
            }
        }
    }
}

impl<'v> TargetListExpr<'v, TargetNode> {
    pub(crate) async fn unpack(
        value: TargetListExprArg<'v>,
        ctx: &BxlContext<'_>,
        dice: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<TargetListExpr<'v, TargetNode>> {
        match value {
            TargetListExprArg::Target(x) => Self::unpack_literal(x, ctx, dice).await,
            TargetListExprArg::List(x) => Self::unpack_iterable(x, ctx, dice).await,
        }
    }

    async fn unpack_literal(
        value: TargetNodeOrTargetLabelOrStr<'v>,
        ctx: &BxlContext<'_>,
        dice: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<TargetListExpr<'v, TargetNode>> {
        match value {
            TargetNodeOrTargetLabelOrStr::TargetNode(target) => {
                Ok(TargetListExpr::One(TargetExpr::Node(target.0.dupe())))
            }
            TargetNodeOrTargetLabelOrStr::TargetLabel(target) => Ok(TargetListExpr::One(
                TargetExpr::Label(Cow::Borrowed(target.label())),
            )),
            TargetNodeOrTargetLabelOrStr::Str(s) => {
                match ParsedPattern::<TargetPatternExtra>::parse_relaxed(
                    ctx.target_alias_resolver(),
                    // TODO(nga): Parse relaxed relative to cell root is incorrect.
                    CellPath::new(ctx.cell_name(), CellRelativePath::empty()),
                    s,
                    ctx.cell_resolver(),
                    ctx.cell_alias_resolver(),
                )? {
                    ParsedPattern::Target(pkg, name, TargetPatternExtra) => {
                        Ok(TargetListExpr::One(TargetExpr::Label(Cow::Owned(
                            TargetLabel::new(pkg, name.as_ref()),
                        ))))
                    }
                    pattern => {
                        let loaded_patterns =
                            load_patterns(dice, vec![pattern], MissingTargetBehavior::Fail).await?;
                        let mut target_set = TargetSet::new();
                        for (_package, results) in loaded_patterns.into_iter() {
                            target_set.extend(results?.into_values());
                        }
                        Ok(Self::TargetSet(Cow::Owned(target_set)))
                    }
                }
            }
        }
    }

    async fn unpack_iterable(
        value: TargetSetOrTargetList<'v>,
        ctx: &BxlContext<'_>,
        dice: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<TargetListExpr<'v, TargetNode>> {
        match value {
            TargetSetOrTargetList::TargetSet(s) => Ok(Self::TargetSet(Cow::Borrowed(s))),
            TargetSetOrTargetList::TargetList(items) => {
                let mut resolved = vec![];

                for item in items.try_items() {
                    let item = item?;
                    let unpacked = Self::unpack_literal(item.unpack(), ctx, dice).await?;

                    match unpacked {
                        TargetListExpr::One(node) => resolved.push(node),
                        TargetListExpr::TargetSet(set) => match set {
                            Cow::Borrowed(s) => itertools::Either::Left(s.iter().duped()),
                            Cow::Owned(s) => itertools::Either::Right(s.into_iter()),
                        }
                        .for_each(|t| resolved.push(TargetExpr::Node(t))),
                        TargetListExpr::Iterable(_) => {
                            return Err(TargetExprError::NotATarget(item.to_value().to_repr()))
                                .buck_error_context("list in a list");
                        }
                    }
                }
                Ok(Self::Iterable(resolved))
            }
        }
    }
}

#[derive(Debug, Allocative)]
pub(crate) enum SingleOrCompatibleConfiguredTargets {
    Single(ConfiguredTargetNode),
    Compatibles(Vec<MaybeCompatible<ConfiguredTargetNode>>),
}

impl SingleOrCompatibleConfiguredTargets {
    pub(crate) fn into_value<'v>(
        self,
        heap: Heap<'v>,
        bxl_eval_extra: &BxlEvalExtra,
    ) -> buck2_error::Result<Value<'v>> {
        match self {
            SingleOrCompatibleConfiguredTargets::Single(node) => {
                Ok(heap.alloc(StarlarkConfiguredTargetNode(node)))
            }
            SingleOrCompatibleConfiguredTargets::Compatibles(compatibles) => {
                let target_set = filter_incompatible(compatibles, bxl_eval_extra)?;
                Ok(heap.alloc(StarlarkTargetSet(target_set)))
            }
        }
    }
}

async fn unpack_string_literal(
    val: &str,
    global_cfg_options: &GlobalCfgOptions,
    ctx: &BxlContextCoreData,
    dice: &mut DiceComputations<'_>,
) -> buck2_error::Result<SingleOrCompatibleConfiguredTargets> {
    match ParsedPattern::<TargetPatternExtra>::parse_relaxed(
        ctx.target_alias_resolver(),
        // TODO(nga): Parse relaxed relative to cell root is incorrect.
        CellPath::new(ctx.cell_name(), CellRelativePath::empty()),
        val,
        ctx.cell_resolver(),
        ctx.cell_alias_resolver(),
    )? {
        ParsedPattern::Target(pkg, name, TargetPatternExtra) => {
            let label = dice
                .get_configured_target(&TargetLabel::new(pkg, name.as_ref()), global_cfg_options)
                .await?;
            let compatible_node = dice.get_configured_target_node(&label).await.ok()?;
            compatible_node
                .require_compatible()
                .map(|n| SingleOrCompatibleConfiguredTargets::Single(n.dupe()))
        }
        pattern => {
            let loaded_patterns =
                load_patterns(dice, vec![pattern], MissingTargetBehavior::Fail).await?;

            let (maybe_compatible_iter, _package_errors) = get_maybe_compatible_targets(
                dice,
                loaded_patterns.iter_loaded_targets_by_package(),
                global_cfg_options,
                true,
            )
            .await?;

            let maybe_compatible = maybe_compatible_iter
                .map(|r| r.ok())
                .collect::<buck2_error::Result<_>>()?;
            Ok(SingleOrCompatibleConfiguredTargets::Compatibles(
                maybe_compatible,
            ))
        }
    }
}

#[derive(Debug, Clone, Allocative)]
pub(crate) enum OwnedTargetNodeOrTargetLabel {
    TargetNode(StarlarkTargetNode),
    TargetLabel(StarlarkTargetLabel),
}

impl OwnedTargetNodeOrTargetLabel {
    pub(crate) fn from_ref(reference: TargetNodeOrTargetLabel<'_>) -> Self {
        match reference {
            TargetNodeOrTargetLabel::TargetNode(node) => {
                OwnedTargetNodeOrTargetLabel::TargetNode(node.dupe())
            }
            TargetNodeOrTargetLabel::TargetLabel(label) => {
                OwnedTargetNodeOrTargetLabel::TargetLabel(label.dupe())
            }
        }
    }

    fn label(&self) -> &TargetLabel {
        match self {
            OwnedTargetNodeOrTargetLabel::TargetNode(node) => node.0.label(),
            OwnedTargetNodeOrTargetLabel::TargetLabel(label) => label.label(),
        }
    }

    pub(crate) async fn to_configured_target_node(
        &self,
        global_cfg_options: &GlobalCfgOptions,
        dice: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<ConfiguredTargetNode> {
        let configured_label = dice
            .get_configured_target(self.label(), global_cfg_options)
            .await?;
        dice.get_configured_target_node(&configured_label)
            .await
            .require_compatible()
            .map(|n| n.dupe())
    }

    pub(crate) async fn to_unconfigured_target_node(
        &self,
        dice: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<TargetNode> {
        match self {
            OwnedTargetNodeOrTargetLabel::TargetNode(node) => Ok(node.0.dupe()),
            OwnedTargetNodeOrTargetLabel::TargetLabel(label) => Ok(dice
                .get_target_node(label.label())
                .await
                .map(|node| node.dupe())?),
        }
    }
}

#[derive(Debug, Clone, Allocative)]
pub(crate) enum OwnedConfiguredTargetNodeArg {
    ConfiguredTargetNode(StarlarkConfiguredTargetNode),
    ConfiguredTargetLabel(StarlarkConfiguredTargetLabel),
    String(String),
    Unconfigured(OwnedTargetNodeOrTargetLabel),
}

impl OwnedConfiguredTargetNodeArg {
    pub(crate) fn from_ref(reference: ConfiguredTargetNodeArg<'_>) -> Self {
        match reference {
            ConfiguredTargetNodeArg::ConfiguredTargetNode(node) => {
                OwnedConfiguredTargetNodeArg::ConfiguredTargetNode(node.dupe())
            }
            ConfiguredTargetNodeArg::ConfiguredTargetLabel(label) => {
                OwnedConfiguredTargetNodeArg::ConfiguredTargetLabel(label.dupe())
            }
            ConfiguredTargetNodeArg::Str(str) => {
                OwnedConfiguredTargetNodeArg::String(str.to_owned())
            }
            ConfiguredTargetNodeArg::Unconfigured(unconfigured) => {
                OwnedConfiguredTargetNodeArg::Unconfigured(OwnedTargetNodeOrTargetLabel::from_ref(
                    unconfigured,
                ))
            }
        }
    }

    pub(crate) async fn to_configured_target_node(
        &self,
        global_cfg_options: &GlobalCfgOptions,
        ctx: &BxlContextCoreData,
        dice: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<SingleOrCompatibleConfiguredTargets> {
        match self {
            OwnedConfiguredTargetNodeArg::ConfiguredTargetNode(node) => {
                Ok(SingleOrCompatibleConfiguredTargets::Single(node.0.dupe()))
            }
            OwnedConfiguredTargetNodeArg::ConfiguredTargetLabel(label) => {
                let compatible = dice.get_configured_target_node(label.label()).await.ok()?;
                compatible
                    .require_compatible()
                    .map(|n| SingleOrCompatibleConfiguredTargets::Single(n.dupe()))
            }
            OwnedConfiguredTargetNodeArg::String(str) => {
                unpack_string_literal(str, global_cfg_options, ctx, dice).await
            }
            OwnedConfiguredTargetNodeArg::Unconfigured(unconfigured) => unconfigured
                .to_configured_target_node(global_cfg_options, dice)
                .await
                .map(SingleOrCompatibleConfiguredTargets::Single),
        }
    }
}

#[derive(Debug, Clone, Allocative)]
pub(crate) enum OwnedTargetNodeArg {
    Unconfigured(OwnedTargetNodeOrTargetLabel),
    String(String),
}

impl OwnedTargetNodeArg {
    pub(crate) fn from_ref(expr: &TargetNodeOrTargetLabelOrStr<'_>) -> Self {
        match *expr {
            TargetNodeOrTargetLabelOrStr::TargetNode(node) => OwnedTargetNodeArg::Unconfigured(
                OwnedTargetNodeOrTargetLabel::TargetNode(node.dupe()),
            ),
            TargetNodeOrTargetLabelOrStr::TargetLabel(label) => OwnedTargetNodeArg::Unconfigured(
                OwnedTargetNodeOrTargetLabel::TargetLabel(label.dupe()),
            ),
            TargetNodeOrTargetLabelOrStr::Str(str) => OwnedTargetNodeArg::String(str.to_owned()),
        }
    }

    pub(crate) async fn to_unconfigured_target_node(
        &self,
        ctx: &BxlContextCoreData,
        dice: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<Either<StarlarkTargetNode, StarlarkTargetSet<TargetNode>>> {
        match self {
            OwnedTargetNodeArg::Unconfigured(unconfigured) => unconfigured
                .to_unconfigured_target_node(dice)
                .await
                .map(|node| Either::Left(StarlarkTargetNode(node))),
            OwnedTargetNodeArg::String(str) => {
                match ParsedPattern::<TargetPatternExtra>::parse_relaxed(
                    ctx.target_alias_resolver(),
                    CellPath::new(ctx.cell_name(), CellRelativePath::empty()),
                    str,
                    ctx.cell_resolver(),
                    ctx.cell_alias_resolver(),
                )? {
                    ParsedPattern::Target(pkg, name, TargetPatternExtra) => {
                        let label = TargetLabel::new(pkg, name.as_ref());
                        Ok(dice
                            .get_target_node(&label)
                            .await
                            .map(|node| Either::Left(StarlarkTargetNode(node)))?)
                    }
                    pattern => {
                        let loaded_patterns =
                            load_patterns(dice, vec![pattern], MissingTargetBehavior::Fail).await?;
                        let mut target_set = TargetSet::new();
                        for (_package, results) in loaded_patterns.into_iter() {
                            target_set.extend(results?.into_values());
                        }
                        Ok(Either::Right(StarlarkTargetSet(target_set)))
                    }
                }
            }
        }
    }

    pub(crate) async fn to_unconfigured_target_set(
        &self,
        ctx: &BxlContextCoreData,
        dice: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<StarlarkTargetSet<TargetNode>> {
        match self.to_unconfigured_target_node(ctx, dice).await? {
            Either::Left(node) => Ok(StarlarkTargetSet(TargetSet::from_iter(vec![node.0]))),
            Either::Right(target_set) => Ok(target_set),
        }
    }
}

#[derive(Debug, Clone, Allocative)]
pub(crate) enum OwnedTargetListExprArg {
    Target(OwnedTargetNodeArg),
    TargetSet(TargetSet<TargetNode>),
    TargetList(Vec<OwnedTargetNodeArg>),
}

impl OwnedTargetListExprArg {
    pub(crate) fn from_ref(expr: &TargetListExprArg<'_>) -> buck2_error::Result<Self> {
        Ok(match expr {
            TargetListExprArg::Target(target) => {
                OwnedTargetListExprArg::Target(OwnedTargetNodeArg::from_ref(target))
            }
            TargetListExprArg::List(list) => match list {
                TargetSetOrTargetList::TargetSet(set) => {
                    OwnedTargetListExprArg::TargetSet(set.0.clone())
                }
                TargetSetOrTargetList::TargetList(list) => {
                    let owned_targets = list
                        .try_items()
                        .map(|item| buck2_error::Ok(OwnedTargetNodeArg::from_ref(&item?.unpack())))
                        .collect::<buck2_error::Result<_>>()?;
                    OwnedTargetListExprArg::TargetList(owned_targets)
                }
            },
        })
    }

    pub(crate) async fn to_unconfigured_target_set(
        &self,
        ctx: &BxlContextCoreData,
        dice: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<StarlarkTargetSet<TargetNode>> {
        match self {
            OwnedTargetListExprArg::Target(target) => {
                target.to_unconfigured_target_set(ctx, dice).await
            }
            OwnedTargetListExprArg::TargetSet(target_set) => {
                Ok(StarlarkTargetSet(target_set.clone()))
            }
            OwnedTargetListExprArg::TargetList(targets) => {
                let mut result = TargetSet::new();
                for target in targets {
                    let target_set = target.to_unconfigured_target_set(ctx, dice).await?;
                    result.extend(&target_set.0);
                }
                Ok(StarlarkTargetSet(result))
            }
        }
    }
}
