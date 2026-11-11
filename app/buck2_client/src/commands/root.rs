/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::str::FromStr;

use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::immediate_config::ImmediateConfigContext;
use buck2_client_ctx::path_arg::PathArg;
use buck2_common::argv::Argv;
use buck2_common::argv::SanitizedArgv;
use buck2_common::invocation_roots::find_invocation_roots;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::working_dir::AbsWorkingDir;

#[derive(Debug, Clone, clap::ValueEnum)]
enum RootKind {
    Cell,
    Project,
    Daemon,
}

impl FromStr for RootKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "cell" => Ok(Self::Cell),
            "project" => Ok(Self::Project),
            "daemon" => Ok(Self::Daemon),
            _ => Err("expected one of `package`, `cell`, `project`, or `daemon`".to_owned()),
        }
    }
}

#[derive(Debug, clap::Parser)]
#[clap(about = "Find buck cell, project or package root")]
pub struct RootCommand {
    #[clap(
        short,
        long,
        help("which root to print"),
        default_value("cell"),
        value_enum
    )]
    kind: RootKind,
    #[clap(
        help(
            "determine the root for a specific directory (if not provided, finds the root for the current directory)"
        ),
        value_name = "PATH",
        long
    )]
    dir: Option<PathArg>,
}

impl RootCommand {
    /// Whether this prints the daemon directory, which depends on the daemon routing.
    pub fn prints_daemon_dir(&self) -> bool {
        matches!(self.kind, RootKind::Daemon)
    }

    pub fn exec(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
    ) -> buck2_error::Result<()> {
        if matches!(self.kind, RootKind::Daemon) {
            let daemon_dir =
                ctx.with_runtime(|ctx| async move { ctx.resolve_tenant_daemon_dir().await })?;
            buck2_client_ctx::println!("{}", daemon_dir.path.to_string_lossy())?;
            return Ok(());
        }
        let root = {
            let working_dir_data;
            let imm_ctx_data;
            let (roots, imm_ctx) = match self.dir.clone() {
                Some(dir) => {
                    let base_dir = dir.resolve(&ctx.working_dir);
                    // Note: While `canonicalize` is usually wrong, in this case it's necessary
                    // because our definition of where the project root is doesn't make sense for
                    // non-normalized paths
                    let base_dir = fs_util::canonicalize(&base_dir).categorize_internal()?;
                    working_dir_data = AbsWorkingDir::unchecked_new(base_dir);
                    let roots = find_invocation_roots(&working_dir_data)?;
                    imm_ctx_data = ImmediateConfigContext::new(&working_dir_data);
                    (roots, &imm_ctx_data)
                }
                None => (ctx.paths()?.roots.clone(), ctx.immediate_config),
            };
            match self.kind {
                RootKind::Cell => {
                    let root = imm_ctx.resolve_alias_to_path_in_cwd("")?;
                    roots.project_root.resolve(&*root)
                }
                RootKind::Project => roots.project_root.root().to_owned(),
                // Handled above
                RootKind::Daemon => unreachable!(),
            }
        };

        buck2_client_ctx::println!("{}", root.to_string_lossy())?;
        Ok(())
    }

    pub fn sanitize_argv(&self, argv: Argv) -> SanitizedArgv {
        argv.no_need_to_sanitize()
    }
}
