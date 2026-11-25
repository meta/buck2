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
use starlark::values::list::StarlarkList;
use starlark::values::tuple::StarlarkTuple;

use crate::attrs::coerce::arc_str_interner::ArcStrInterner;
use crate::attrs::coerce::str_hash::str_hash;
use crate::attrs::coerce::str_hash::strs_hash;

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
    /// `labels.infer_target_names`.
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
    /// Target modifiers written as a list of strings, by those strings. Parsing a list is
    /// costly, and the targets of a package tend to share a few.
    modifiers_cache: RefCell<HashTable<(u64, Box<[Box<str>]>, CfgModifiers)>>,
    /// The node constructors (`build_*_attr`) encode attribute values into
    /// this arena as coercion proceeds. Package contexts seal it into the
    /// package's shared storage; bzl contexts (rule defaults) seal it into
    /// the value's own storage.
    attr_arena_builder: RefCell<CoercedAttrArenaBuilder>,
}

#[cfg(test)]
thread_local! {
    /// How many times this thread has parsed target modifiers.
    static PARSED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The strings of `items`, if they are nothing but strings.
fn strings_of<'a, 'v>(
    items: &'a [Value<'v>],
) -> Option<impl Iterator<Item = &'v str> + Clone + 'a> {
    items
        .iter()
        .all(|item| item.unpack_str().is_some())
        .then(|| items.iter().filter_map(|item| item.unpack_str()))
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
            modifiers_cache: RefCell::new(HashTable::new()),
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

impl BuildAttrCoercionContext {
    /// The modifiers of a list of `strings` whose hash is `hash`: what `parse` gives, the
    /// first time the list is seen.
    fn modifiers_of_strings<'a>(
        &self,
        hash: u64,
        strings: impl Iterator<Item = &'a str> + Clone,
        parse: impl FnOnce() -> CfgModifiers,
    ) -> CfgModifiers {
        match self.modifiers_cache.borrow_mut().entry(
            hash,
            |(h, key, _)| *h == hash && key.iter().map(|s| &**s).eq(strings.clone()),
            |(h, _, _)| *h,
        ) {
            hash_table::Entry::Occupied(e) => e.get().2,
            hash_table::Entry::Vacant(e) => {
                let modifiers = parse();
                e.insert((hash, strings.map(Box::from).collect(), modifiers));
                modifiers
            }
        }
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
        let parse = || {
            #[cfg(test)]
            PARSED.set(PARSED.get() + 1);
            CfgModifiers::from_target(value, &ModifierLabelParser::new(&self.cell_resolver))
        };
        let list = StarlarkList::from_value(value);
        let items = match &list {
            Some(list) => Some(list.content()),
            None => StarlarkTuple::from_value(value).map(|tuple| tuple.content()),
        };
        match items.and_then(strings_of) {
            Some(strings) => self.modifiers_of_strings(strs_hash(strings.clone()), strings, parse),
            None => parse(),
        }
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

#[cfg(test)]
mod tests {
    use buck2_node::attrs::coercion_context::AttrCoercionContext;
    use buck2_node::modifiers::CfgModifiers;
    use buck2_node::modifiers::ModifierLabelParser;
    use starlark::values::Heap;
    use starlark::values::Value;
    use starlark::values::list::AllocList;
    use starlark::values::tuple::AllocTuple;

    use crate::attrs::coerce::ctx::PARSED;
    use crate::attrs::coerce::testing::coercion_ctx;

    #[test]
    fn test_modifiers_of_a_list_seen_before_are_those_of_parsing_it() {
        Heap::temp(|heap| {
            let ctx = coercion_ctx();
            let labels = ModifierLabelParser::new(&ctx.cell_resolver);
            let list = |strings: &[&str]| heap.alloc(AllocList(strings.iter().copied())).to_value();
            let string = heap.alloc("cell1//os:linux").to_value();
            let values: Vec<Value> = vec![
                list(&[]),
                list(&["cell1//os:linux"]),
                list(&["cell1//os:linux", "cell1//cpu:arm64"]),
                list(&["cell1//cpu:arm64", "cell1//os:linux"]),
                // The characters of the third list, in one string.
                list(&["cell1//os:linuxcell1//cpu:arm64"]),
                heap.alloc(AllocTuple(["cell1//os:linux"])).to_value(),
                list(&["not a label"]),
                // Not lists of strings.
                heap.alloc(AllocList([string, Value::new_none()]))
                    .to_value(),
                string,
                Value::new_none(),
            ];
            // Twice, to also ask for every list after it was stored.
            for value in values.iter().chain(&values) {
                assert_eq!(
                    CfgModifiers::from_target(*value, &labels),
                    ctx.coerce_cfg_modifiers(*value),
                    "{value}"
                );
            }
            assert_ne!(
                ctx.coerce_cfg_modifiers(values[2]),
                ctx.coerce_cfg_modifiers(values[3])
            );
            assert_ne!(
                ctx.coerce_cfg_modifiers(values[2]),
                ctx.coerce_cfg_modifiers(values[4])
            );
            assert_ne!(
                ctx.coerce_cfg_modifiers(values[1]),
                ctx.coerce_cfg_modifiers(values[7])
            );
        });
    }

    #[test]
    fn test_equal_lists_of_strings_are_parsed_once() {
        Heap::temp(|heap| {
            let ctx = coercion_ctx();
            let list = |strings: &[&str]| heap.alloc(AllocList(strings.iter().copied())).to_value();
            // How many times coercing `value` parses it.
            let parses = |value: Value| {
                let before = PARSED.get();
                ctx.coerce_cfg_modifiers(value);
                PARSED.get() - before
            };

            assert_eq!(1, parses(list(&["cell1//os:linux"])));
            // An equal list, and a tuple of the same strings, find the first.
            assert_eq!(0, parses(list(&["cell1//os:linux"])));
            assert_eq!(
                0,
                parses(heap.alloc(AllocTuple(["cell1//os:linux"])).to_value())
            );
            assert_eq!(1, parses(list(&["cell1//cpu:arm64"])));
            // Anything but a list of strings is parsed every time.
            let string = heap.alloc("cell1//os:linux").to_value();
            let mixed = heap
                .alloc(AllocList([string, Value::new_none()]))
                .to_value();
            for value in [string, Value::new_none(), mixed] {
                assert_eq!(1, parses(value));
                assert_eq!(1, parses(value));
            }
        });
    }

    #[test]
    fn test_lists_with_one_hash_keep_their_own_modifiers() {
        Heap::temp(|heap| {
            let ctx = coercion_ctx();
            let labels = ModifierLabelParser::new(&ctx.cell_resolver);
            let parse = |strings: [&str; 1]| {
                CfgModifiers::from_target(heap.alloc(AllocList(strings)).to_value(), &labels)
            };
            let (linux, arm64) = (["cell1//os:linux"], ["cell1//cpu:arm64"]);

            let of_linux = ctx.modifiers_of_strings(0, linux.into_iter(), || parse(linux));
            let of_arm64 = ctx.modifiers_of_strings(0, arm64.into_iter(), || parse(arm64));
            assert_eq!(parse(linux), of_linux);
            assert_eq!(parse(arm64), of_arm64);
            assert_ne!(of_linux, of_arm64);

            // And neither is parsed again.
            let seen = |strings: [&str; 1]| {
                ctx.modifiers_of_strings(0, strings.into_iter(), || panic!("parsed twice"))
            };
            assert_eq!(of_linux, seen(linux));
            assert_eq!(of_arm64, seen(arm64));
        });
    }
}
