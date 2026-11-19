/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_common::legacy_configs::configs::LegacyBuckConfig;
use buck2_common::package_listing::listing::PackageListing;
use buck2_common::package_listing::listing::testing::PackageListingExt;
use buck2_core::bzl::ImportPath;
use buck2_core::cells::CellResolver;
use buck2_core::cells::alias::NonEmptyCellAlias;
use buck2_core::cells::build_file_cell::BuildFileCell;
use buck2_core::cells::cell_path_with_allowed_relative_dir::CellPathWithAllowedRelativeDir;
use buck2_core::cells::cell_root_path::CellRootPathBuf;
use buck2_core::cells::name::CellName;
use buck2_core::package::PackageLabel;
use buck2_core::pattern::pattern::InferTargetNames;
use buck2_hash::IntentionallyStdHashMap;
use buck2_interpreter::extra::InterpreterHostArchitecture;
use buck2_interpreter::extra::InterpreterHostPlatform;
use buck2_interpreter::file_type::StarlarkFileType;
use buck2_node::attrs::arena::OwnedCoercedAttr;
use buck2_node::attrs::attr_type::AttrType;
use buck2_node::attrs::coercion_context::AttrCoercionContext;
use buck2_node::attrs::configurable::AttrIsConfigurable;
use dupe::Dupe;
use starlark::environment::Globals;
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::syntax::AstModule;
use starlark::values::Value;

use crate::attrs::coerce::attr_type::AttrTypeExt;
use crate::attrs::coerce::ctx::BuildAttrCoercionContext;
use crate::interpreter::buckconfig::LegacyConfigsViewForStarlark;
use crate::interpreter::build_context::BuildContext;
use crate::interpreter::build_context::PerFileTypeContext;
use crate::interpreter::bzl_eval_ctx::BzlEvalCtx;
use crate::interpreter::cell_info::InterpreterCellInfo;
use crate::interpreter::functions::host_info::HostInfo;

pub fn coercion_ctx() -> BuildAttrCoercionContext {
    coercion_ctx_listing(PackageListing::testing_empty())
}

pub fn coercion_ctx_listing(package_listing: PackageListing) -> BuildAttrCoercionContext {
    build_coercion_ctx(package_listing)
}

/// Coercion for tests that assert on the encoded value: coerces into this
/// context's arena and copies the result into its own single-value storage.
pub trait CoerceSealed {
    fn coerce_sealed<'v>(
        &self,
        attr: &AttrType,
        value: Value<'v>,
    ) -> buck2_error::Result<OwnedCoercedAttr>;
}

impl CoerceSealed for BuildAttrCoercionContext {
    fn coerce_sealed<'v>(
        &self,
        attr: &AttrType,
        value: Value<'v>,
    ) -> buck2_error::Result<OwnedCoercedAttr> {
        let token = attr.coerce(AttrIsConfigurable::Yes, self, value)?;
        let arena = self.attr_arena();
        OwnedCoercedAttr::new_with(|b| b.encode_view(arena.view(token.word())))
    }
}

pub(crate) fn build_coercion_ctx(package_listing: PackageListing) -> BuildAttrCoercionContext {
    let package = PackageLabel::testing();
    let aliases = IntentionallyStdHashMap::from([(
        NonEmptyCellAlias::new("cell1".to_owned()).unwrap(),
        CellName::testing_new("cell1"),
    )]);

    let cell_resolver = CellResolver::testing_with_names_and_paths_with_alias(
        &[
            (package.cell_name(), CellRootPathBuf::testing_new("")),
            (
                CellName::testing_new("cell1"),
                CellRootPathBuf::testing_new("cell1"),
            ),
        ],
        aliases,
    );
    let cell_alias_resolver = cell_resolver.root_cell_cell_alias_resolver().dupe();

    BuildAttrCoercionContext::new_with_package(
        cell_resolver,
        cell_alias_resolver,
        (package, package_listing),
        CellPathWithAllowedRelativeDir::backwards_relative_not_supported(
            package.as_cell_path().to_owned(),
        ),
        InferTargetNames::No,
    )
}

fn cell_resolver() -> CellResolver {
    CellResolver::testing_with_name_and_path(
        CellName::testing_new("root"),
        CellRootPathBuf::testing_new(""),
    )
}

pub fn to_value<'v>(env: &Module<'v>, globals: &Globals, content: &str) -> Value<'v> {
    let import_path = ImportPath::testing_new("root//:defs.bzl");
    let ast = AstModule::parse(
        &import_path.to_string(),
        content.to_owned(),
        &StarlarkFileType::Bzl.dialect(false),
    )
    .unwrap_or_else(|err| panic!("Failed parsing `{content}`. Error: `{err}`"));
    let cell_info = InterpreterCellInfo::new(
        BuildFileCell::new(CellName::testing_new("root")),
        cell_resolver(),
        cell_resolver().root_cell_cell_alias_resolver().dupe(),
    )
    .unwrap();

    let mut buckconfigs =
        LegacyConfigsViewForStarlark::new(LegacyBuckConfig::empty(), LegacyBuckConfig::empty());
    let host_platform = InterpreterHostPlatform::Linux;
    let host_architecture = InterpreterHostArchitecture::X86_64;
    let host_info = HostInfo::new(host_platform, host_architecture, None);
    let build_ctx = BuildContext::new(
        &cell_info,
        &mut buckconfigs,
        &host_info,
        PerFileTypeContext::Bzl(BzlEvalCtx {
            bzl_path: import_path,
        }),
        false,
        InferTargetNames::No,
    );

    let mut eval = Evaluator::new(env);
    eval.extra = Some(&build_ctx);
    eval.eval_module(ast, globals)
        .unwrap_or_else(|err| panic!("Failed interpreting `{content}`. Error: `{err}`"))
}
