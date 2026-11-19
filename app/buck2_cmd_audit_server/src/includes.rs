/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Write;

use async_trait::async_trait;
use buck2_cli_proto::ClientContext;
use buck2_cmd_audit_client::includes::AuditIncludesCommand;
use buck2_common::dice::cells::HasCellResolver;
use buck2_core::bzl::ImportPath;
use buck2_core::cells::CellResolver;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::package::PackageLabel;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_hash::buck_mut_insertion_map;
use buck2_interpreter::file_loader::LoadedModule;
use buck2_interpreter::load_module::InterpreterCalculation;
use buck2_interpreter::paths::module::StarlarkModulePath;
use buck2_node::nodes::eval_result::EvaluationResult;
use buck2_node::nodes::frontend::TargetGraphCalculation;
use buck2_query::query::graph::node::LabeledNode;
use buck2_query::query::graph::node::NodeKey;
use buck2_query::query::graph::successors::AsyncChildVisitor;
use buck2_query::query::traversal::AsyncNodeLookup;
use buck2_query::query::traversal::ChildVisitor;
use buck2_query::query::traversal::async_depth_first_postorder_traversal;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::ctx::ServerCommandDiceContext;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use derive_more::Display;
use dice::DiceComputations;
use dice::LinearRecomputeDiceComputations;
use dupe::Dupe;
use futures::FutureExt;
use futures::StreamExt;
use futures::stream::FuturesOrdered;
use itertools::Itertools;
use ref_cast::RefCast;
use serde::Serialize;
use serde::Serializer;
use serde::ser::SerializeMap;

use crate::ServerAuditSubcommand;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum AuditIncludesError {
    #[error("Error loading buildfile for `{0}` found a mismatched buildfile name (`{1}`)")]
    WrongBuildfilePath(CellPath, FileNameBuf),
    #[error("invalid buildfile path `{0}`")]
    InvalidPath(CellPath),
}

async fn get_transitive_includes(
    ctx: &mut DiceComputations<'_>,
    load_result: &EvaluationResult,
) -> buck2_error::Result<Vec<ImportPath>> {
    // We define a simple graph of LoadedModules to traverse.
    #[derive(Clone, Dupe)]
    struct Node(LoadedModule);

    impl Node {
        fn import_path(&self) -> &ImportPath {
            self.0
                .path()
                .unpack_load_file()
                .expect("only visit imports so only bzl files are expected")
        }
    }

    #[derive(Display, Debug, Hash, Eq, PartialEq, Clone, RefCast)]
    #[repr(transparent)]
    struct NodeRef(ImportPath);

    impl NodeKey for NodeRef {}

    impl LabeledNode for Node {
        type Key = NodeRef;

        fn node_key(&self) -> &NodeRef {
            NodeRef::ref_cast(self.import_path())
        }
    }

    struct Lookup<'a, 'd> {
        ctx: LinearRecomputeDiceComputations<'a, 'd>,
    }

    #[async_trait]
    impl AsyncNodeLookup<Node> for Lookup<'_, '_> {
        async fn get(&self, label: &NodeRef) -> buck2_error::Result<Node> {
            Ok(Node(
                self.ctx
                    .get()
                    .get_loaded_module(StarlarkModulePath::LoadFile(&label.0))
                    .await?
                    .dupe(),
            ))
        }
    }

    let mut imports: Vec<ImportPath> = Vec::new();
    struct Delegate;

    let visit = |target: Node| {
        imports.push(target.import_path().clone());
        Ok(())
    };

    impl AsyncChildVisitor<Node> for Delegate {
        async fn for_each_child(
            &self,
            target: &Node,
            mut func: impl ChildVisitor<Node>,
        ) -> buck2_error::Result<()> {
            for import in target.0.imports() {
                func.visit(&NodeRef(import.clone()))?;
            }
            Ok(())
        }
    }

    ctx.with_linear_recompute(|ctx| {
        async move {
            let lookup = Lookup { ctx };

            async_depth_first_postorder_traversal(
                &lookup,
                load_result
                    .imports()
                    .iter()
                    .map(NodeRef::ref_cast)
                    .collect::<Vec<_>>(),
                Delegate,
                visit,
                false, // allow_partial_graph
            )
            .await
        }
        .boxed()
    })
    .await?;
    Ok(imports)
}

async fn load_and_collect_includes(
    ctx: &mut DiceComputations<'_>,
    path: &CellPath,
) -> buck2_error::Result<Vec<ImportPath>> {
    let parent = path
        .parent()
        .ok_or_else(|| AuditIncludesError::InvalidPath(*path))?;
    let package = PackageLabel::from_cell_path(parent)?;
    let load_result = ctx.get_interpreter_results(package).await?;

    let buildfile_name = load_result.buildfile_path().filename();
    if buildfile_name
        != path
            .path()
            .file_name()
            .expect("checked that this has a parent above")
    {
        return Err(
            AuditIncludesError::WrongBuildfilePath(*path, buildfile_name.to_owned()).into(),
        );
    }

    get_transitive_includes(ctx, load_result).await
}

fn resolve_path(
    cells: &CellResolver,
    fs: &ProjectRoot,
    current_cell_abs_path: &AbsNormPath,
    path: &str,
) -> buck2_error::Result<CellPath> {
    // To match buck1, if the path is absolute we use it as-is, but if not it is treated
    // as relative to the working dir cell root (not the working dir).
    // The easiest way to consistently handle non-canonical paths
    // is to just resolve to absolute here, and then relativize.
    //
    // Note if the path is already absolute, this operation is a no-op.
    let path = current_cell_abs_path.as_abs_path().join(path);
    // input path from `buck2 audit includes [BUILD_FILES]`
    let abs_path = fs_util::canonicalize(path).categorize_input()?;

    let project_path = fs.relativize(&abs_path)?;
    Ok(cells.get_cell_path(&project_path))
}

#[async_trait]
impl ServerAuditSubcommand for AuditIncludesCommand {
    async fn server_execute(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        mut stdout: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        _client_ctx: ClientContext,
    ) -> buck2_error::Result<()> {
        Ok(server_ctx
            .with_dice_ctx(|server_ctx, ctx| async move {
                let cells = ctx.ctx().get_cell_resolver().await?;
                let cwd = server_ctx.working_dir();
                let current_cell = cells.get(cells.find(cwd))?;
                let fs = server_ctx.project_root();
                let current_cell_abs_path =
                    fs.resolve(current_cell.path().as_project_relative_path());

                let futures: FuturesOrdered<_> = self
                    .patterns
                    .iter()
                    .unique()
                    .map(|path| {
                        let path = path.to_owned();
                        let ctx = ctx.dupe();
                        let cell_path = resolve_path(&cells, fs, &current_cell_abs_path, &path);
                        async move {
                            let load_result = try {
                                let cell_path = cell_path?;
                                load_and_collect_includes(&mut ctx.ctx(), &cell_path).await?
                            };
                            (path, load_result)
                        }
                    })
                    .collect();

                let results: Vec<(_, buck2_error::Result<Vec<_>>)> = futures.collect().await;
                // This is expected to not return any errors, and so we're not careful about not propagating it.
                let to_absolute_path = move |include: ImportPath| -> buck2_error::Result<_> {
                    let include = include.path();
                    let cell = cells.get(include.cell())?;
                    let path = cell.path().join(include.path());
                    Ok(fs.resolve(&path))
                };
                let absolutize_paths =
                    |paths: Vec<ImportPath>| -> buck2_error::Result<Vec<AbsNormPathBuf>> {
                        paths
                            .into_iter()
                            .map(&to_absolute_path)
                            .collect::<Result<Vec<_>, _>>()
                    };
                let results: Vec<(String, buck2_error::Result<Vec<AbsNormPathBuf>>)> = results
                    .into_iter()
                    .map(|(path, includes)| (path, includes.and_then(absolutize_paths)))
                    .collect::<Vec<_>>();

                let mut stdout = stdout.as_writer();

                // For the printing of results, we don't need to propagate errors, just print
                // them. After we print the results, we'll propagate an error if there is one.
                if self.json {
                    let mut ser = serde_json::Serializer::pretty(&mut stdout);
                    // buck1 has a bug where it doesn't properly handle >1 arg when passed --json
                    // it also, sadly, prints just a single list of outputs for that case. we match
                    // buck1's behavior for 1 successful file and print a dictionary for multiple. This is
                    // unfortunate, but we hope that users can migrate to the equivalent query commands instead.
                    if let [(_path, Ok(includes))] = results.as_slice() {
                        includes.serialize(&mut ser)?
                    } else {
                        let mut map = ser.serialize_map(Some(results.len()))?;
                        for (path, includes) in &results {
                            match includes {
                                Ok(includes) => map.serialize_entry(
                                    path,
                                    &buck_mut_insertion_map! {"includes" => &includes},
                                )?,
                                Err(e) => map.serialize_entry(
                                    path,
                                    &buck_mut_insertion_map! {"$error" => format!("{:#}", e)},
                                )?,
                            }
                        }
                        map.end()?;
                    }

                    // flush a newline after serde output.
                    writeln!(stdout)?;
                } else {
                    for (path, includes) in &results {
                        match includes {
                            Ok(includes) => {
                                // intentionally add a blank line after the header
                                writeln!(stdout, "# {path}\n")?;
                                for include in includes {
                                    // To match buck1, we print absolute paths.
                                    writeln!(stdout, "{include}")?;
                                }
                            }
                            Err(e) => {
                                // intentionally add a blank line after the header
                                writeln!(stdout, "! {path}\n")?;
                                writeln!(stdout, "{e:#}")?;
                            }
                        }
                    }
                }

                // propagate the first error.
                for (_, result) in results {
                    result?;
                }

                Ok(())
            })
            .await?)
    }
}
