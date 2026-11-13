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

use allocative::Allocative;
use async_trait::async_trait;
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::legacy_configs::dice::HasLegacyConfigs;
use buck2_common::legacy_configs::view::LegacyBuckConfigView;
use buck2_core::cells::CellResolver;
use buck2_interpreter::dice::starlark_types::GetStarlarkTypes;
use dice::DiceComputations;
use dice::EqualityBehavior;
use dice::Key;
use dice::OkPagableValueSerialize;
use dice::ValueSerialize;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use dupe::ResultDupedErrExt;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark::environment::GlobalFrozenHeapName;
use starlark::environment::Globals;

use crate::interpreter::configuror::BuildInterpreterConfiguror;
use crate::interpreter::context::HasInterpreterContext;
use crate::interpreter::globals::base_globals;
use crate::interpreter::load_data::LOAD_AS_ALLOWLIST;
use crate::interpreter::load_data::LoadAsAllowlist;

pagable::static_str!(GLOBAL_ENV_HEAP_NAME = concat!(module_path!(), "::global_env"));

/// Information shared across interpreters. Contains no cell-specific
/// information.
#[derive(Allocative, pagable::Pagable)]
pub struct GlobalInterpreterState {
    pub cell_resolver: CellResolver,

    /// The GlobalEnvironment contains all the globally available symbols
    /// (primarily starlark stdlib and Buck-provided functions).
    pub global_env: Globals,

    /// Interpreter Configurer
    pub configuror: Arc<BuildInterpreterConfiguror>,

    /// Check types in Starlark (or just parse and ignore).
    pub disable_starlark_types: bool,

    /// Static typechecking for bzl and bxl files.
    pub unstable_typecheck: bool,

    /// Files allowed to use an `?as=` import format override.
    pub load_as_allowlist: LoadAsAllowlist,
}

impl GlobalInterpreterState {
    pub fn new(
        cell_resolver: CellResolver,
        interpreter_configuror: Arc<BuildInterpreterConfiguror>,
        disable_starlark_types: bool,
        unstable_typecheck: bool,
        load_as_allowlist: LoadAsAllowlist,
    ) -> buck2_error::Result<Self> {
        let global_env = base_globals()
            .with(|g| {
                if let Some(additional_globals) = interpreter_configuror.additional_globals() {
                    additional_globals.0.apply(g);
                }
            })
            .build_named(GlobalFrozenHeapName {
                name: GLOBAL_ENV_HEAP_NAME,
            });

        Ok(Self {
            cell_resolver,
            global_env,
            configuror: interpreter_configuror,
            disable_starlark_types,
            unstable_typecheck,
            load_as_allowlist,
        })
    }

    pub fn configuror(&self) -> &Arc<BuildInterpreterConfiguror> {
        &self.configuror
    }

    pub fn globals(&self) -> &Globals {
        &self.global_env
    }
}

#[async_trait]
pub trait HasGlobalInterpreterState<'d> {
    async fn get_global_interpreter_state(
        &mut self,
    ) -> buck2_error::Result<&'d Arc<GlobalInterpreterState>>;
}

#[async_trait]
impl<'d> HasGlobalInterpreterState<'d> for DiceComputations<'d> {
    async fn get_global_interpreter_state(
        &mut self,
    ) -> buck2_error::Result<&'d Arc<GlobalInterpreterState>> {
        #[derive(
            Clone,
            derive_more::Display,
            Dupe,
            Debug,
            Eq,
            Hash,
            PartialEq,
            Allocative,
            Pagable
        )]
        #[display("{:?}", self)]
        #[pagable_typetag(dice::DiceKeyDyn)]
        struct GisKey();

        #[async_trait]
        impl Key for GisKey {
            type Value = buck2_error::Result<Arc<GlobalInterpreterState>>;
            async fn compute(
                &self,
                ctx: &mut DiceComputations,
                _cancellation: &CancellationContext,
            ) -> Self::Value {
                let interpreter_configuror = ctx.get_interpreter_configuror().await?.dupe();
                let cell_resolver = ctx.get_cell_resolver().await?.dupe();
                let disable_starlark_types = ctx.get_disable_starlark_types().await?;
                let unstable_typecheck = ctx.get_unstable_typecheck().await?;

                let root_config = ctx.get_legacy_root_config_on_dice().await?;
                let load_as_allowlist = LoadAsAllowlist::new(
                    root_config
                        .view(ctx)
                        .parse_list::<String>(LOAD_AS_ALLOWLIST)?
                        .unwrap_or_default(),
                )?;

                Ok(Arc::new(GlobalInterpreterState::new(
                    cell_resolver,
                    interpreter_configuror,
                    disable_starlark_types,
                    unstable_typecheck,
                    load_as_allowlist,
                )?))
            }

            fn equality_behavior() -> EqualityBehavior<Self::Value> {
                EqualityBehavior::AlwaysUnequal
            }

            fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
                OkPagableValueSerialize::<Self::Value>::new()
            }
        }

        self.compute(&GisKey()).await?.as_ref().duped_err()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(values: &[&str]) -> LoadAsAllowlist {
        LoadAsAllowlist::new(values.iter().map(|s| (*s).to_owned()).collect()).unwrap()
    }

    #[test]
    fn test_empty_allows_nothing() {
        let allowed = parse(&[]);
        assert!(!allowed.is_allowed(Some("uv.lock")));
        assert!(!allowed.is_allowed(Some("foo.toml")));
        assert!(!allowed.is_allowed(None));
    }

    #[test]
    fn test_extension_glob() {
        let allowed = parse(&["*.lock", "*.cfg"]);
        assert!(allowed.is_allowed(Some("uv.lock")));
        assert!(allowed.is_allowed(Some("app.cfg")));
        // `*.lock` also matches a compound extension.
        assert!(allowed.is_allowed(Some("foo.pkg.lock")));
        assert!(!allowed.is_allowed(Some("foo.toml")));
        // A bare `*.lock` glob needs a dot; `mylock` has none.
        assert!(!allowed.is_allowed(Some("mylock")));
        assert!(!allowed.is_allowed(None));
    }

    #[test]
    fn test_star_allows_everything() {
        let allowed = parse(&["*"]);
        assert!(allowed.is_allowed(Some("uv.lock")));
        assert!(allowed.is_allowed(Some("foo.anything")));
    }

    #[test]
    fn test_whitespace_and_empty_entries_ignored() {
        let allowed = parse(&["  *.lock  ", ""]);
        assert!(allowed.is_allowed(Some("uv.lock")));
        assert!(!allowed.is_allowed(Some("foo.toml")));
    }

    #[test]
    fn test_compound_extension_glob() {
        let allowed = parse(&["*.pkg.lock"]);
        assert!(allowed.is_allowed(Some("uv.pkg.lock")));
        assert!(allowed.is_allowed(Some("a.b.pkg.lock")));
        assert!(!allowed.is_allowed(Some("uv.lock")));
    }

    #[test]
    fn test_whole_filename_entry() {
        // Pin a specific package-manager lockfile by its whole name.
        let allowed = parse(&["uv.lock"]);
        assert!(allowed.is_allowed(Some("uv.lock")));
        // Other `.lock` files are not opened up.
        assert!(!allowed.is_allowed(Some("poetry.lock")));
        assert!(!allowed.is_allowed(Some("Cargo.lock")));
        // An exact-name glob has no wildcard, so it does not match `nested.uv.lock`.
        assert!(!allowed.is_allowed(Some("nested.uv.lock")));
    }

    #[test]
    fn test_invalid_glob_is_an_error() {
        assert!(LoadAsAllowlist::new(vec!["[".to_owned()]).is_err());
    }
}
