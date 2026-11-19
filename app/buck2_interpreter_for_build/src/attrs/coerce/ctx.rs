/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::RefCell;
use std::cell::RefMut;
use std::fmt;
use std::fmt::Debug;

use buck2_common::package_listing::listing::PackageListing;
use buck2_core::cells::CellAliasResolver;
use buck2_core::cells::CellResolver;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::cell_path_with_allowed_relative_dir::CellPathWithAllowedRelativeDir;
use buck2_core::cells::name::CellName;
use buck2_core::is_open_source;
use buck2_core::package::PackageLabel;
use buck2_core::package::package_relative_path::PackageRelativePath;
use buck2_core::package::package_relative_path::PackageRelativePathBuf;
use buck2_core::pattern::pattern::InferTargetNames;
use buck2_core::pattern::pattern::ParsedPattern;
use buck2_core::pattern::pattern::TargetParsingRel;
use buck2_core::pattern::pattern_type::PatternType;
use buck2_core::pattern::pattern_type::ProvidersPatternExtra;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_core::provider::label::ProvidersLabel;
use buck2_node::attrs::arena::AttrToken;
use buck2_node::attrs::arena::CoercedAttrArenaBuilder;
use buck2_node::attrs::coerced_path::CoercedDirectory;
use buck2_node::attrs::coerced_path::CoercedPath;
use buck2_node::attrs::coercion_context::AttrCoercionContext;
use buck2_node::attrs::coercion_context::CoerceMemoKey;
use buck2_node::attrs::coercion_context::CoerceMemoTable;
use buck2_node::modifiers::CfgModifiers;
use buck2_node::modifiers::ModifierLabelParser;
use buck2_node::query::query_functions::CONFIGURED_GRAPH_QUERY_FUNCTIONS;
use buck2_query::query::syntax::simple::eval::error::QueryError;
use buck2_query::query::syntax::simple::functions::QueryLiteralVisitor;
use buck2_query_parser::Expr;
use buck2_query_parser::spanned::Spanned;
use buck2_util::arc_str::ArcStr;
use buck2_util::arc_str::PackedStr;
use bumpalo::Bump;
use dupe::Dupe;
use hashbrown::HashTable;
use hashbrown::hash_table;
use starlark::values::Value;

use crate::attrs::coerce::arc_str_interner::ArcStrInterner;
use crate::attrs::coerce::str_hash::str_hash;

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum BuildAttrCoercionContextError {
    #[error("Expected a label, got the pattern `{0}`.")]
    RequiredLabel(String),
    #[error("Expected a package: `{0}` can only be specified in a build file.")]
    NotBuildFileContext(String),
    #[error("Expected file, but got a directory for path `{1}` in package `{0}`.")]
    SourceFileIsDirectory(PackageLabel, String),
    #[error("Source file `{1}` does not exist as a member of package `{0}`.")]
    SourceFileMissing(PackageLabel, String),
    #[error(
        "Directory `{1}` of package `{0}` may not cover any subpackages, but includes subpackage `{2}`."
    )]
    SourceDirectoryIncludesSubPackage(PackageLabel, String, PackageRelativePathBuf),
}

/// An incomplete attr coercion context. Will be replaced with a real one later.
pub struct BuildAttrCoercionContext {
    /// Used to coerce targets
    cell_resolver: CellResolver,
    cell_name: CellName,
    cell_alias_resolver: CellAliasResolver,
    /// Used to resolve relative targets. This is present when a build file
    /// is being evaluated, however it is absent if an extension file is being
    /// evaluated. The latter case occurs when default values for attributes
    /// are coerced when a UDR is declared.
    enclosing_package: Option<(PackageLabel, PackageListing)>,
    /// This defines the limited scope in which we allow parsing patterns beginning with `../`
    current_dir_with_allowed_relative_dirs: CellPathWithAllowedRelativeDir,
    /// Whether to infer a target name when a pattern does not provide one,
    /// making `//foo/bar` equivalent to `//foo/bar:bar`. Controlled by the
    /// `buck2.infer_target_names` buckconfig.
    infer_target_names: InferTargetNames,
    /// Allocator for `label_cache`.
    alloc: Bump,
    /// Label coercion cache. We use `RawTable` where because `HashMap` API
    /// requires either computing hash twice (for get, then for insert) or
    /// allocating a key to perform a query using `entry` API.
    /// Strings are owned by `alloc`, using bump allocator makes evaluation 0.5% faster.
    label_cache: RefCell<HashTable<(u64, *const str, ProvidersLabel)>>,
    /// Interns strings embedded as `ArcStr` fields inside composite values
    /// (macro `Arg` parts, `select_fail`/`select_incompatible` messages),
    /// which have no arena node of their own.
    str_interner: ArcStrInterner,
    /// The node constructors (`build_*_attr`) encode attribute values into
    /// this arena as coercion proceeds. Package contexts seal it into the
    /// package's shared storage; bzl contexts (rule defaults) seal it into
    /// the value's own storage.
    attr_arena_builder: RefCell<CoercedAttrArenaBuilder>,
}

impl Debug for BuildAttrCoercionContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BuildAttrCoercionContext")
            .finish_non_exhaustive()
    }
}

impl BuildAttrCoercionContext {
    fn new(
        cell_resolver: CellResolver,
        cell_name: CellName,
        cell_alias_resolver: CellAliasResolver,
        enclosing_package: Option<(PackageLabel, PackageListing)>,
        current_dir_with_allowed_relative_dirs: CellPathWithAllowedRelativeDir,
        infer_target_names: InferTargetNames,
    ) -> Self {
        Self {
            cell_resolver,
            cell_name,
            cell_alias_resolver,
            enclosing_package,
            current_dir_with_allowed_relative_dirs,
            infer_target_names,
            alloc: Bump::new(),
            label_cache: RefCell::new(HashTable::new()),
            str_interner: ArcStrInterner::new(),
            attr_arena_builder: RefCell::new(CoercedAttrArenaBuilder::default()),
        }
    }

    pub(crate) fn with_attr_arena_builder<R>(
        &self,
        f: impl FnOnce(&mut CoercedAttrArenaBuilder) -> R,
    ) -> R {
        f(&mut self.attr_arena_builder.borrow_mut())
    }

    pub(crate) fn with_attr_arena_ref<R>(
        &self,
        f: impl FnOnce(&CoercedAttrArenaBuilder) -> R,
    ) -> R {
        f(&self.attr_arena_builder.borrow())
    }

    pub(crate) fn take_attr_arena_builder(self) -> CoercedAttrArenaBuilder {
        self.attr_arena_builder.into_inner()
    }

    pub fn new_no_package(
        cell_resolver: CellResolver,
        cell_name: CellName,
        cell_alias_resolver: CellAliasResolver,
        infer_target_names: InferTargetNames,
    ) -> Self {
        Self::new(
            cell_resolver,
            cell_name,
            cell_alias_resolver,
            None,
            CellPathWithAllowedRelativeDir::backwards_relative_not_supported(CellPath::empty(
                cell_name,
            )),
            infer_target_names,
        )
    }

    pub fn new_with_package(
        cell_resolver: CellResolver,
        cell_alias_resolver: CellAliasResolver,
        enclosing_package: (PackageLabel, PackageListing),
        current_dir_with_allowed_relative_dirs: CellPathWithAllowedRelativeDir,
        infer_target_names: InferTargetNames,
    ) -> Self {
        Self::new(
            cell_resolver,
            enclosing_package.0.cell_name(),
            cell_alias_resolver,
            Some(enclosing_package),
            current_dir_with_allowed_relative_dirs,
            infer_target_names,
        )
    }

    pub fn parse_pattern<P: PatternType>(
        &self,
        value: &str,
    ) -> buck2_error::Result<ParsedPattern<P>> {
        let target_parsing_rel = match self.enclosing_package.as_ref().map(|x| x.0.as_cell_path()) {
            Some(package) => {
                if self
                    .current_dir_with_allowed_relative_dirs
                    .has_allowed_relative_dir()
                {
                    TargetParsingRel::AllowRelative(
                        &self.current_dir_with_allowed_relative_dirs,
                        None,
                    )
                } else {
                    TargetParsingRel::AllowLimitedRelative(package)
                }
            }
            None => TargetParsingRel::RequireAbsolute(self.cell_name),
        };
        ParsedPattern::parse_not_relaxed(
            value,
            target_parsing_rel,
            &self.cell_resolver,
            &self.cell_alias_resolver,
            self.infer_target_names,
        )
    }

    fn coerce_label_no_cache(&self, value: &str) -> buck2_error::Result<ProvidersLabel> {
        // TODO(nmj): Make this take an import path / package
        match self.parse_pattern::<ProvidersPatternExtra>(value)? {
            ParsedPattern::Target(package, target_name, providers) => {
                Ok(providers.into_providers_label(package, target_name.as_ref()))
            }
            _ => Err(BuildAttrCoercionContextError::RequiredLabel(value.to_owned()).into()),
        }
    }

    fn require_enclosing_package(
        &self,
        msg: &str,
    ) -> buck2_error::Result<&(PackageLabel, PackageListing)> {
        self.enclosing_package.as_ref().ok_or_else(|| {
            BuildAttrCoercionContextError::NotBuildFileContext(msg.to_owned()).into()
        })
    }
}

impl<'v> AttrCoercionContext<'v> for BuildAttrCoercionContext {
    fn attr_arena(&self) -> RefMut<'_, CoercedAttrArenaBuilder> {
        self.attr_arena_builder.borrow_mut()
    }

    fn coerce_providers_label(&self, value: &str) -> buck2_error::Result<ProvidersLabel> {
        let hash = str_hash(value);
        let mut label_cache = self.label_cache.borrow_mut();

        match label_cache.entry(
            hash,
            |(h, v, _)| *h == hash && value == unsafe { &**v },
            |(h, _, _)| *h,
        ) {
            hash_table::Entry::Occupied(e) => Ok(e.get().2.dupe()),
            hash_table::Entry::Vacant(e) => {
                let label = self.coerce_label_no_cache(value)?;

                e.insert((hash, self.alloc.alloc_str(value), label.dupe()));
                Ok(label)
            }
        }
    }

    fn coerce_cfg_modifiers(&self, value: Value<'v>) -> CfgModifiers {
        CfgModifiers::from_target(value, &ModifierLabelParser::new(&self.cell_resolver))
    }

    fn intern_str(&self, value: &str) -> ArcStr {
        self.str_interner.intern(value)
    }

    fn coerce_path(&self, value: &str, allow_directory: bool) -> buck2_error::Result<CoercedPath> {
        let path = <&PackageRelativePath>::try_from(value)?;
        let (package, listing) = self.require_enclosing_package(value)?;

        if let Some(path) = listing.get_file(path) {
            return Ok(CoercedPath::File(path));
        }

        if let Some(path) = listing.get_dir(path) {
            if !allow_directory {
                return Err(BuildAttrCoercionContextError::SourceFileIsDirectory(
                    package.dupe(),
                    value.to_owned(),
                )
                .into());
            }
            // FIXME(JakobDegen): A directory source that contains a subpackage should be an error
            // here; as it is, it silently covers only this package's files. Internally this is
            // tolerated for the same reason as a missing file; open source gets the error.
            if is_open_source()
                && let Some(subpackage) = listing.subpackages_within(&path).next()
            {
                return Err(
                    BuildAttrCoercionContextError::SourceDirectoryIncludesSubPackage(
                        package.dupe(),
                        value.to_owned(),
                        subpackage.to_owned(),
                    )
                    .into(),
                );
            }
            let files = listing.files_within(&path).collect();
            Ok(CoercedPath::Directory(Box::new(CoercedDirectory {
                dir: path,
                files,
            })))
        } else {
            // FIXME(JakobDegen): A source file that is not in the package listing should be an
            // error here, not only once something reads it. Internally it is tolerated because
            // of misconfigured sparse checkouts; open source has no such excuse.
            if is_open_source() {
                return Err(BuildAttrCoercionContextError::SourceFileMissing(
                    package.dupe(),
                    value.to_owned(),
                )
                .into());
            }
            Ok(CoercedPath::File(PackedStr::new(path)))
        }
    }

    fn coerce_target_pattern(
        &self,
        pattern: &str,
    ) -> buck2_error::Result<ParsedPattern<TargetPatternExtra>> {
        self.parse_pattern(pattern)
    }

    fn visit_query_function_literals<'q>(
        &self,
        visitor: &mut dyn QueryLiteralVisitor<'q>,
        expr: &Spanned<Expr<'q>>,
        query: &'q str,
    ) -> buck2_error::Result<()> {
        CONFIGURED_GRAPH_QUERY_FUNCTIONS
            .get()?
            .visit_literals(visitor, expr)
            .map_err(|e| QueryError::convert_error(e, query))?;
        Ok(())
    }
}

/// Coercion context for one buildfile evaluation: delegates to the package's
/// `BuildAttrCoercionContext` and memoizes per-value results in the module's
/// heap-scoped table, which is what proves the memo cannot outlive the
/// values it keys.
pub struct MemoizedCoercionCtx<'c, 'v> {
    pub base: &'c BuildAttrCoercionContext,
    pub memo: &'v CoerceMemoTable<'v>,
}

impl<'c, 'v> AttrCoercionContext<'v> for MemoizedCoercionCtx<'c, 'v> {
    fn attr_arena(&self) -> RefMut<'_, CoercedAttrArenaBuilder> {
        AttrCoercionContext::attr_arena(self.base)
    }

    fn coerce_providers_label(&self, value: &str) -> buck2_error::Result<ProvidersLabel> {
        AttrCoercionContext::coerce_providers_label(self.base, value)
    }

    fn coerce_memo_lookup(&self, key: CoerceMemoKey<'v>) -> Option<AttrToken> {
        self.memo.lookup(key)
    }

    fn coerce_memo_insert(&self, key: CoerceMemoKey<'v>, value: AttrToken) {
        self.memo.insert(key, value);
    }

    fn coerce_cfg_modifiers(&self, value: Value<'v>) -> CfgModifiers {
        AttrCoercionContext::coerce_cfg_modifiers(self.base, value)
    }

    fn intern_str(&self, value: &str) -> ArcStr {
        AttrCoercionContext::intern_str(self.base, value)
    }

    fn coerce_path(&self, value: &str, allow_directory: bool) -> buck2_error::Result<CoercedPath> {
        AttrCoercionContext::coerce_path(self.base, value, allow_directory)
    }

    fn coerce_target_pattern(
        &self,
        pattern: &str,
    ) -> buck2_error::Result<ParsedPattern<TargetPatternExtra>> {
        AttrCoercionContext::coerce_target_pattern(self.base, pattern)
    }

    fn visit_query_function_literals<'q>(
        &self,
        visitor: &mut dyn QueryLiteralVisitor<'q>,
        expr: &Spanned<Expr<'q>>,
        query: &'q str,
    ) -> buck2_error::Result<()> {
        AttrCoercionContext::visit_query_function_literals(self.base, visitor, expr, query)
    }
}
