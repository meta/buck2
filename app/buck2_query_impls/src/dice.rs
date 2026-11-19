/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use async_trait::async_trait;
use buck2_build_api::configure_targets::load_compatible_patterns_with_modifiers;
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::dice::data::HasProjectRoot;
use buck2_common::file_ops::dice::DiceFileComputations;
use buck2_common::package_listing::dice::DicePackageListingResolver;
use buck2_common::package_listing::resolver::PackageListingResolver;
use buck2_common::pattern::resolve::ResolveTargetPatterns;
use buck2_common::pattern::resolve::ResolvedPattern;
use buck2_common::target_aliases::BuckConfigTargetAliasResolver;
use buck2_common::target_aliases::HasTargetAliasResolver;
use buck2_core::cells::CellAliasResolver;
use buck2_core::cells::CellResolver;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::cell_path_with_allowed_relative_dir::CellPathWithAllowedRelativeDir;
use buck2_core::cells::name::CellName;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_core::package::PackageLabel;
use buck2_core::pattern::pattern::InferTargetNames;
use buck2_core::pattern::pattern::ParsedPattern;
use buck2_core::pattern::pattern::ParsedPatternWithModifiers;
use buck2_core::pattern::pattern::TargetParsingRel;
use buck2_core::pattern::pattern_type::ProvidersPatternExtra;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_core::pattern::query_file_literal::parse_query_file_literal;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_hash::BuckMutMap;
use buck2_hash::buck_mut_insertion_set;
use buck2_node::load_patterns::MissingTargetBehavior;
use buck2_node::load_patterns::load_patterns;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::configured_frontend::ConfiguredTargetNodeCalculation;
use buck2_node::nodes::unconfigured::TargetNode;
use buck2_node::target_calculation::ConfiguredTargetCalculation;
use buck2_query::query::syntax::simple::eval::file_set::FileNode;
use buck2_query::query::syntax::simple::eval::file_set::FileSet;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use dice::DiceComputations;
use dice::LinearRecomputeDiceComputations;
use dupe::Dupe;

use crate::cquery::environment::CqueryDelegate;
use crate::uquery::environment::QueryLiterals;
use crate::uquery::environment::UqueryDelegate;

pub(crate) mod aquery;

pub(crate) struct LiteralParser {
    // file and target literals are resolved relative to the working dir.
    working_dir: CellPath,
    working_dir_abs: AbsNormPathBuf,
    project_root: ProjectRoot,
    cell_resolver: CellResolver,
    cell_alias_resolver: CellAliasResolver,
    target_alias_resolver: BuckConfigTargetAliasResolver,
}

impl LiteralParser {
    fn convert_parsed_pattern(
        &self,
        providers_pattern: ParsedPattern<ProvidersPatternExtra>,
    ) -> buck2_error::Result<ParsedPattern<TargetPatternExtra>> {
        let target_pattern = match providers_pattern {
            // FIXME(JakobDegen): Providers in a query target literal should be an error, at which
            // point this becomes a plain `ParsedPattern::parse_relative`; today they are dropped.
            ParsedPattern::Target(package, target_name, ProvidersPatternExtra { .. }) => {
                ParsedPattern::Target(package, target_name, TargetPatternExtra)
            }
            ParsedPattern::Package(package) => ParsedPattern::Package(package),
            ParsedPattern::Recursive(path) => ParsedPattern::Recursive(path),
        };

        Ok(target_pattern)
    }

    fn parse_target_pattern(
        &self,
        value: &str,
    ) -> buck2_error::Result<ParsedPattern<TargetPatternExtra>> {
        let providers_pattern = self.parse_providers_pattern(value)?;
        self.convert_parsed_pattern(providers_pattern)
    }

    fn parse_target_pattern_with_modifiers(
        &self,
        value: &str,
    ) -> buck2_error::Result<ParsedPatternWithModifiers<TargetPatternExtra>> {
        let ParsedPatternWithModifiers {
            parsed_pattern,
            modifiers,
        } = self.parse_providers_pattern_with_modifiers(value)?;

        let target_pattern = self.convert_parsed_pattern(parsed_pattern)?;

        Ok(ParsedPatternWithModifiers {
            parsed_pattern: target_pattern,
            modifiers,
        })
    }

    pub(crate) fn parse_providers_pattern(
        &self,
        value: &str,
    ) -> buck2_error::Result<ParsedPattern<ProvidersPatternExtra>> {
        ParsedPattern::parse_not_relaxed(
            value,
            TargetParsingRel::AllowRelative(
                &CellPathWithAllowedRelativeDir::backwards_relative_not_supported(self.working_dir),
                Some(&self.target_alias_resolver),
            ),
            &self.cell_resolver,
            &self.cell_alias_resolver,
            InferTargetNames::No,
        )
    }

    pub(crate) fn parse_providers_pattern_with_modifiers(
        &self,
        value: &str,
    ) -> buck2_error::Result<ParsedPatternWithModifiers<ProvidersPatternExtra>> {
        ParsedPatternWithModifiers::parse_not_relaxed(
            value,
            TargetParsingRel::AllowRelative(
                &CellPathWithAllowedRelativeDir::backwards_relative_not_supported(self.working_dir),
                Some(&self.target_alias_resolver),
            ),
            &self.cell_resolver,
            &self.cell_alias_resolver,
        )
    }

    fn parse_file_literal(&self, literal: &str) -> buck2_error::Result<CellPath> {
        parse_query_file_literal(
            literal,
            &self.cell_alias_resolver,
            &self.cell_resolver,
            &self.working_dir_abs,
            &self.project_root,
        )
    }
}

/// A Uquery delegate that resolves TargetNodes with the provided
/// InterpreterCalculation.
pub(crate) struct DiceQueryDelegate<'c, 'd> {
    ctx: LinearRecomputeDiceComputations<'c, 'd>,
    query_data: Arc<DiceQueryData>,
}

pub(crate) struct DiceQueryData {
    literal_parser: LiteralParser,
    global_cfg_options: GlobalCfgOptions,
    /// With uquery `--allow-partial-graph`, skip packages that fail to load
    /// while enumerating an open-ended (recursive) literal, instead of
    /// aborting. Explicitly named literals still fail on load errors.
    allow_partial_graph: bool,
}

impl DiceQueryData {
    pub(crate) fn new(
        global_cfg_options: GlobalCfgOptions,
        cell_resolver: CellResolver,
        cell_alias_resolver: CellAliasResolver,
        working_dir: &ProjectRelativePath,
        project_root: ProjectRoot,
        target_alias_resolver: BuckConfigTargetAliasResolver,
        allow_partial_graph: bool,
    ) -> Self {
        let cell_path = cell_resolver.get_cell_path(working_dir);

        let working_dir_abs = project_root.resolve(working_dir);

        Self {
            literal_parser: LiteralParser {
                working_dir_abs,
                working_dir: cell_path,
                project_root,
                cell_resolver,
                cell_alias_resolver,
                target_alias_resolver,
            },
            global_cfg_options,
            allow_partial_graph,
        }
    }

    pub(crate) fn literal_parser(&self) -> &LiteralParser {
        &self.literal_parser
    }

    pub(crate) fn global_cfg_options(&self) -> &GlobalCfgOptions {
        &self.global_cfg_options
    }
}

impl<'c, 'd> DiceQueryDelegate<'c, 'd> {
    pub(crate) fn new(
        ctx: LinearRecomputeDiceComputations<'c, 'd>,
        query_data: Arc<DiceQueryData>,
    ) -> Self {
        Self { ctx, query_data }
    }

    pub(crate) fn ctx(&self) -> DiceComputations<'_> {
        self.ctx.get()
    }

    pub(crate) fn query_data(&self) -> &Arc<DiceQueryData> {
        &self.query_data
    }
}

#[async_trait]
impl UqueryDelegate for DiceQueryDelegate<'_, '_> {
    // get the list of potential buildfile names for each cell
    async fn get_buildfile_names_by_cell(
        &self,
    ) -> buck2_error::Result<BuckMutMap<CellName, Arc<[FileNameBuf]>>> {
        let mut ctx = self.ctx.get();
        let resolver = ctx.get_cell_resolver().await?;
        let buildfiles = ctx
            .try_compute_join(resolver.cells(), async |ctx, (name, _)| {
                DiceFileComputations::buildfiles(ctx, name)
                    .await
                    .map(|x| (name, x.dupe()))
            })
            .await?;

        Ok(buildfiles.into_iter().collect())
    }

    async fn resolve_target_patterns(
        &self,
        patterns: &[&str],
    ) -> buck2_error::Result<ResolvedPattern<TargetPatternExtra>> {
        let parsed_patterns = patterns
            .iter()
            .map(|p| self.query_data.literal_parser.parse_target_pattern(p))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ResolveTargetPatterns::resolve(&mut self.ctx.get(), &parsed_patterns).await?)
    }

    // Returns all packages from immediate enclosing up to cell root that could potentially own the path.
    async fn get_enclosing_packages(
        &self,
        path: &CellPath,
    ) -> buck2_error::Result<Vec<PackageLabel>> {
        let cell_root = CellPath::empty(path.cell());
        Ok(DicePackageListingResolver(&mut self.ctx.get())
            .get_enclosing_packages(*path, cell_root)
            .await?
            .into_iter()
            .collect())
    }

    async fn eval_file_literal(&self, literal: &str) -> buck2_error::Result<FileSet> {
        let cell_path = self.query_data.literal_parser.parse_file_literal(literal)?;
        Ok(FileSet::new(buck_mut_insertion_set![FileNode(cell_path)]))
    }

    fn linear_dice_computations(&self) -> LinearRecomputeDiceComputations<'_, '_> {
        self.ctx
    }

    fn ctx(&self) -> DiceComputations<'_> {
        self.ctx.get()
    }
}

#[async_trait]
impl CqueryDelegate for DiceQueryDelegate<'_, '_> {
    fn uquery_delegate(&self) -> &dyn UqueryDelegate {
        self
    }

    async fn get_node_for_configured_target(
        &self,
        target: &ConfiguredTargetLabel,
    ) -> buck2_error::Result<ConfiguredTargetNode> {
        Ok(self
            .ctx
            .get()
            .get_configured_target_node(target)
            .await
            .require_compatible()?
            .dupe())
    }

    async fn get_node_for_default_configured_target(
        &self,
        target: &TargetLabel,
    ) -> buck2_error::Result<MaybeCompatible<ConfiguredTargetNode>> {
        let target = self.ctx.get().get_default_configured_target(target).await?;
        self.ctx
            .get()
            .get_configured_target_node(&target)
            .await
            .ok()
            .map(|n| n.map(|n| n.dupe()))
    }

    fn ctx(&self) -> DiceComputations<'_> {
        self.ctx.get()
    }
}

#[async_trait]
impl QueryLiterals<ConfiguredTargetNode> for DiceQueryData {
    async fn eval_literals(
        &self,
        literals: &[&str],
        ctx: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<TargetSet<ConfiguredTargetNode>> {
        let parsed_patterns = literals
            .iter()
            .map(|p| self.literal_parser.parse_target_pattern_with_modifiers(p))
            .collect::<Result<Vec<_>, _>>()?;

        let result = load_compatible_patterns_with_modifiers(
            ctx,
            parsed_patterns,
            &self.global_cfg_options,
            MissingTargetBehavior::Fail,
            false,
        )
        .await?;
        Ok(result.compatible_targets)
    }
}

#[async_trait]
impl QueryLiterals<TargetNode> for DiceQueryData {
    async fn eval_literals(
        &self,
        literals: &[&str],
        ctx: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<TargetSet<TargetNode>> {
        let parsed_patterns = literals
            .iter()
            .map(|p| self.literal_parser.parse_target_pattern(p))
            .collect::<Result<Vec<_>, _>>()?;

        // `--allow-partial-graph` tolerates load failures only in the open-ended
        // parts of a query that have to be enumerated (recursive `//foo/...`
        // patterns). Explicitly named target/package literals must still fully
        // resolve, so a broken or missing one always fails the query.
        let (recursive_patterns, explicit_patterns): (Vec<_>, Vec<_>) = parsed_patterns
            .into_iter()
            .partition(|p| matches!(p, ParsedPattern::Recursive(..)));

        let mut target_set = TargetSet::new();

        for (_package, result) in load_patterns(ctx, explicit_patterns, MissingTargetBehavior::Fail)
            .await?
            .into_iter()
        {
            target_set.extend(result?.into_values());
        }

        for (_package, result) in
            load_patterns(ctx, recursive_patterns, MissingTargetBehavior::Fail)
                .await?
                .into_iter()
        {
            match result {
                Ok(res) => target_set.extend(res.into_values()),
                Err(e) if self.allow_partial_graph => {
                    tracing::trace!(
                        "query allow-partial-graph: skipping a package that failed to load: {e:#}"
                    );
                }
                Err(e) => return Err(e),
            }
        }

        Ok(target_set)
    }
}

pub(crate) async fn get_dice_query_delegate<'a, 'c: 'a, 'd>(
    ctx: LinearRecomputeDiceComputations<'c, 'd>,
    working_dir: &'a ProjectRelativePath,
    global_cfg_options: GlobalCfgOptions,
    allow_partial_graph: bool,
) -> buck2_error::Result<DiceQueryDelegate<'c, 'd>> {
    let cell_resolver = ctx.get().get_cell_resolver().await?.dupe();
    let cell_alias_resolver = ctx
        .get()
        .get_cell_alias_resolver_for_dir(working_dir)
        .await?
        .dupe();
    let target_alias_resolver = ctx.get().target_alias_resolver().await?.dupe();
    let project_root = ctx.get().get_project_root().await?.dupe();
    Ok(DiceQueryDelegate::new(
        ctx,
        Arc::new(DiceQueryData::new(
            global_cfg_options,
            cell_resolver,
            cell_alias_resolver,
            working_dir,
            project_root,
            target_alias_resolver,
            allow_partial_graph,
        )),
    ))
}
