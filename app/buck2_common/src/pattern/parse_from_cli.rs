/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::cells::CellAliasResolver;
use buck2_core::cells::CellResolver;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::pattern::pattern::ParsedPattern;
use buck2_core::pattern::pattern::ParsedPatternWithModifiers;
use buck2_core::pattern::pattern_type::PatternType;
use buck2_core::pattern::unparsed::UnparsedPatterns;
use dice::DiceComputations;

use crate::dice::cells::HasCellResolver;
use crate::pattern::resolve::ResolveTargetPatterns;
use crate::pattern::resolve::ResolvedPattern;
use crate::target_aliases::BuckConfigTargetAliasResolver;
use crate::target_aliases::HasTargetAliasResolver;

struct PatternParser<'d> {
    cell_resolver: &'d CellResolver,
    cell_alias_resolver: &'d CellAliasResolver,
    cwd: CellPath,
    target_alias_resolver: &'d BuckConfigTargetAliasResolver,
}

impl<'d> PatternParser<'d> {
    async fn new(
        ctx: &mut DiceComputations<'d>,
        cwd: &ProjectRelativePath,
    ) -> buck2_error::Result<Self> {
        let cell_resolver = ctx.get_cell_resolver().await?;

        let cwd = cell_resolver.get_cell_path(&cwd);
        let cell_name = cwd.cell();

        let target_alias_resolver = ctx.target_alias_resolver().await?;
        let cell_alias_resolver = ctx.get_cell_alias_resolver(cell_name).await?;

        Ok(Self {
            cell_resolver,
            cell_alias_resolver,
            cwd,
            target_alias_resolver,
        })
    }

    fn parse_pattern<T: PatternType>(
        &self,
        pattern: &str,
    ) -> buck2_error::Result<ParsedPattern<T>> {
        ParsedPattern::parse_relaxed(
            self.target_alias_resolver,
            self.cwd,
            pattern,
            &self.cell_resolver,
            &self.cell_alias_resolver,
        )
    }

    fn parse_pattern_with_modifiers<T: PatternType>(
        &self,
        pattern: &str,
    ) -> buck2_error::Result<ParsedPatternWithModifiers<T>> {
        ParsedPatternWithModifiers::parse_relaxed(
            self.target_alias_resolver,
            self.cwd,
            pattern,
            &self.cell_resolver,
            &self.cell_alias_resolver,
        )
    }
}

/// Parse target patterns out of command line arguments.
///
/// The format allowed here is more relaxed than in build files and elsewhere, so only use this
/// with strings passed by the user on the CLI.
/// See `ParsedPattern::parse_relaxed` for details.
pub async fn parse_patterns_from_cli_args<T: PatternType>(
    ctx: &mut DiceComputations<'_>,
    target_patterns: &[String],
    cwd: &ProjectRelativePath,
) -> buck2_error::Result<Vec<ParsedPattern<T>>> {
    let parser = PatternParser::new(ctx, cwd).await?;

    target_patterns
        .iter()
        .map(|value| parser.parse_pattern(value))
        .collect::<Result<Vec<_>, _>>()
}

pub async fn parse_patterns_with_modifiers_from_cli_args<T: PatternType>(
    ctx: &mut DiceComputations<'_>,
    target_patterns: &[String],
    cwd: &ProjectRelativePath,
) -> buck2_error::Result<Vec<ParsedPatternWithModifiers<T>>> {
    let parser = PatternParser::new(ctx, cwd).await?;

    target_patterns
        .iter()
        .map(|value| parser.parse_pattern_with_modifiers(value))
        .collect::<Result<Vec<_>, _>>()
}

pub async fn parse_patterns_from_cli_args_typed<T: PatternType>(
    ctx: &mut DiceComputations<'_>,
    patterns: &UnparsedPatterns<T>,
) -> buck2_error::Result<Vec<ParsedPattern<T>>> {
    parse_patterns_from_cli_args(ctx, patterns.patterns(), patterns.working_dir()).await
}

pub async fn parse_and_resolve_patterns_from_cli_args<T: PatternType>(
    ctx: &mut DiceComputations<'_>,
    target_patterns: &[String],
    cwd: &ProjectRelativePath,
) -> buck2_error::Result<ResolvedPattern<T>> {
    let patterns = parse_patterns_from_cli_args(ctx, target_patterns, cwd).await?;
    ResolveTargetPatterns::resolve(ctx, &patterns).await
}

pub async fn parse_and_resolve_patterns_with_modifiers_from_cli_args<T: PatternType>(
    ctx: &mut DiceComputations<'_>,
    target_patterns: &[String],
    cwd: &ProjectRelativePath,
) -> buck2_error::Result<ResolvedPattern<T>> {
    let patterns = parse_patterns_with_modifiers_from_cli_args(ctx, target_patterns, cwd).await?;
    ResolveTargetPatterns::resolve_with_modifiers(ctx, &patterns).await
}
