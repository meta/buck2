/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use async_trait::async_trait;
use buck2_core::bzl::ImportPath;
use buck2_core::package::PackageLabel;
use buck2_util::late_binding::LateBinding;
use dice::DiceComputations;
use futures::FutureExt;
use futures::future::BoxFuture;
use starlark::environment::Globals;

use crate::file_loader::LoadedModule;
use crate::file_loader::ModuleDeps;
use crate::paths::module::StarlarkModulePath;
use crate::paths::package::PackageFilePath;
use crate::prelude_path::PreludePath;

#[async_trait]
pub trait InterpreterCalculationImpl: Send + Sync + 'static {
    fn get_loaded_module<'a, 'd>(
        &self,
        ctx: &'a mut DiceComputations<'d>,
        path: StarlarkModulePath<'_>,
    ) -> BoxFuture<'a, buck2_error::Result<&'d LoadedModule>>
    where
        'd: 'a;

    async fn get_module_deps(
        &self,
        ctx: &mut DiceComputations<'_>,
        package: PackageLabel,
    ) -> buck2_error::Result<ModuleDeps>;

    /// Return `None` if the PACKAGE file doesn't exist.
    async fn get_package_file_deps(
        &self,
        ctx: &mut DiceComputations<'_>,
        package: PackageLabel,
    ) -> buck2_error::Result<Option<(PackageFilePath, Vec<ImportPath>)>>;

    async fn global_env(&self, ctx: &mut DiceComputations<'_>) -> buck2_error::Result<Globals>;

    async fn prelude_import(
        &self,
        ctx: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<Option<PreludePath>>;
}

pub static INTERPRETER_CALCULATION_IMPL: LateBinding<&'static dyn InterpreterCalculationImpl> =
    LateBinding::new("INTERPRETER_CALCULATION_IMPL");

pub trait InterpreterCalculation<'d> {
    /// Returns the LoadedModule for a given starlark file. This is cached on the dice graph.
    fn get_loaded_module<'a>(
        &'a mut self,
        path: StarlarkModulePath<'_>,
    ) -> BoxFuture<'a, buck2_error::Result<&'d LoadedModule>>
    where
        'd: 'a;

    fn get_loaded_module_from_import_path<'a>(
        &'a mut self,
        path: &'a ImportPath,
    ) -> BoxFuture<'a, buck2_error::Result<&'d LoadedModule>>
    where
        'd: 'a,
    {
        let module_path = match path.path().path().extension() {
            Some("json") => StarlarkModulePath::JsonFile(path),
            Some("toml") => StarlarkModulePath::TomlFile(path),
            _ => StarlarkModulePath::LoadFile(path),
        };
        self.get_loaded_module(module_path)
    }

    fn get_loaded_module_imports<'a>(
        &'a mut self,
        path: &'a ImportPath,
    ) -> BoxFuture<'a, buck2_error::Result<Vec<ImportPath>>>
    where
        'd: 'a,
    {
        //TODO(benfoxman): Don't need to get the whole module, just parse the imports.
        self.get_loaded_module_from_import_path(path)
            .map(|r| Ok(r?.imports().cloned().collect()))
            .boxed()
    }
}

impl<'d> InterpreterCalculation<'d> for DiceComputations<'d> {
    fn get_loaded_module<'a>(
        &'a mut self,
        path: StarlarkModulePath<'_>,
    ) -> BoxFuture<'a, buck2_error::Result<&'d LoadedModule>>
    where
        'd: 'a,
    {
        match INTERPRETER_CALCULATION_IMPL.get() {
            Ok(i) => i.get_loaded_module(self, path),
            Err(e) => futures::future::ready(Err(e)).boxed(),
        }
    }
}
