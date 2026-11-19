/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::OnceLock;
use std::time::SystemTime;

use buck2_common::argv::ArgFileKind;
use buck2_common::argv::ArgFilePath;
use buck2_common::init::DaemonStartupConfig;
use buck2_common::invocation_roots::InvocationRoots;
use buck2_common::invocation_roots::find_invocation_roots;
use buck2_common::legacy_configs::cells::BuckConfigBasedCells;
use buck2_common::legacy_configs::configs::LegacyBuckConfig;
#[cfg(fbcode_build)]
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_core::buck2_env;
use buck2_core::cells::CellAliasResolver;
use buck2_core::cells::CellResolver;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::cell_root_path::CellRootPathBuf;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::soft_error;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::internal_error;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::abs_path::AbsPath;
use buck2_fs::paths::abs_path::AbsPathBuf;
use buck2_fs::working_dir::AbsWorkingDir;
use buck2_settings::BuckSettings;
use buck2_settings::ResolvedSettings;
use buck2_settings::parser::parse_settings;
use buck2_settings::parser::resolve_settings;
use prost::Message;

/// Lazy-computed immediate config data. This is produced by reading the root buckconfig (but not
/// processing any includes).
struct ImmediateConfigContextData {
    cell_resolver: CellResolver,
    cwd_cell_alias_resolver: CellAliasResolver,
    // Config retained for deferred `DaemonStartupConfig` creation.
    root_config: LegacyBuckConfig,
    #[cfg(fbcode_build)]
    allow_daemon_start_unsandboxed_via_wrapper: bool,
    project_filesystem: ProjectRoot,
    paranoid_info_path: AbsPathBuf,
}

impl ImmediateConfigContextData {
    /// Performs a parse of the root `.buckconfig` for the cell _only_ without following includes
    /// and without parsing any configs for any referenced cells. This means this function might return
    /// an empty mapping if the root `.buckconfig` does not contain the cell definitions.
    fn parse(roots: InvocationRoots) -> buck2_error::Result<Self> {
        let paranoid_info_path = roots.paranoid_info_path()?;
        // This function is non-reentrant, and blocking for a bit should be ok
        let cells = futures::executor::block_on(BuckConfigBasedCells::parse_with_config_args(
            &roots.project_root,
            &[],
        ))?;

        let cwd_cell_alias_resolver = futures::executor::block_on(
            cells.get_cell_alias_resolver_for_cwd_fast(&roots.project_root, &roots.cwd),
        )?;

        #[cfg(fbcode_build)]
        let allow_daemon_start_unsandboxed_via_wrapper = cells
            .root_config
            .parse::<bool>(BuckconfigKeyRef {
                section: "buck2",
                property: "allow_daemon_start_unsandboxed_via_wrapper",
            })?
            .unwrap_or(false);

        Ok(Self {
            cell_resolver: cells.cell_resolver,
            cwd_cell_alias_resolver,
            root_config: cells.root_config,
            #[cfg(fbcode_build)]
            allow_daemon_start_unsandboxed_via_wrapper,
            project_filesystem: roots.project_root,
            paranoid_info_path,
        })
    }
}

pub struct ImmediateConfigContext<'a> {
    // Deliberately use `OnceLock` rather than `Lazy` because `Lazy` forces
    // us to have a shared reference to the underlying `buck2_error::Error` which
    // we cannot use to correct chain the errors. Using `OnceLock` means
    // we don't get the result by a shared reference but instead as local
    // value which can be returned.
    data: OnceLock<ImmediateConfigContextData>,
    setting_arg_layers: OnceLock<Vec<toml::Table>>,
    buck_settings: OnceLock<BuckSettings>,
    // Initialized after setting argument layers are available.
    daemon_startup_config: OnceLock<DaemonStartupConfig>,
    cwd: &'a AbsWorkingDir,
    trace: Vec<AbsNormPathBuf>,
}

impl<'a> ImmediateConfigContext<'a> {
    pub fn new(cwd: &'a AbsWorkingDir) -> Self {
        Self {
            data: OnceLock::new(),
            setting_arg_layers: OnceLock::new(),
            buck_settings: OnceLock::new(),
            daemon_startup_config: OnceLock::new(),
            cwd,
            trace: Vec::new(),
        }
    }

    pub(crate) fn push_trace(&mut self, path: &AbsNormPath) {
        self.trace.push(path.to_buf());
    }

    pub(crate) fn trace(&self) -> &[AbsNormPathBuf] {
        &self.trace
    }

    /// Reads settings for the invocation's repository without loading its build configuration.
    /// All callers must use the invocation's repository root, since the result is cached once.
    pub fn buck_settings(&self, project_root: &ProjectRoot) -> buck2_error::Result<&BuckSettings> {
        let setting_arg_layers = self.setting_arg_layers.get().ok_or_else(|| {
            internal_error!("Buck settings read before setting arguments were set")
        })?;
        self.buck_settings
            .get_or_try_init(|| parse_settings(project_root, setting_arg_layers))
    }

    /// Like [`buck_settings`](Self::buck_settings), with where each value came
    /// from. Not cached; for reporting.
    pub fn resolved_settings(
        &self,
        project_root: &ProjectRoot,
    ) -> buck2_error::Result<ResolvedSettings> {
        let setting_arg_layers = self.setting_arg_layers.get().ok_or_else(|| {
            internal_error!("Buck settings read before setting arguments were set")
        })?;
        resolve_settings(project_root, setting_arg_layers)
    }

    /// The root `.buckconfig`, which is what the buckconfig fallbacks of
    /// settings read on the client.
    pub fn root_config(&self) -> buck2_error::Result<&LegacyBuckConfig> {
        Ok(&self.data()?.root_config)
    }

    pub fn daemon_startup_config(&self) -> buck2_error::Result<&DaemonStartupConfig> {
        self.daemon_startup_config
            .get_or_try_init(|| {
                let data = self.data()?;
                let buck_settings = self.buck_settings(&data.project_filesystem)?;
                let paranoid = match is_paranoid_enabled(&data.paranoid_info_path) {
                    Ok(paranoid) => paranoid,
                    Err(e) => {
                        soft_error!(
                            "paranoid_info_unreadable",
                            e.context(format!(
                                "Failed to determine whether paranoid is enabled in `{}`",
                                data.paranoid_info_path
                            ))
                        );
                        false
                    }
                };

                DaemonStartupConfig::new(&data.root_config, &buck_settings.daemon, paranoid)
                    .buck_error_context("Error loading daemon startup config")
            })
            .buck_error_context("Error creating daemon startup config")
    }

    pub fn set_setting_arg_layers(
        &self,
        setting_arg_layers: Vec<toml::Table>,
    ) -> buck2_error::Result<()> {
        self.setting_arg_layers
            .set(setting_arg_layers)
            .map_err(|_| internal_error!("Attempted to set setting argument layers more than once"))
    }

    pub fn allow_daemon_start_unsandboxed_via_wrapper(&self) -> buck2_error::Result<bool> {
        #[cfg(fbcode_build)]
        {
            Ok(self.data()?.allow_daemon_start_unsandboxed_via_wrapper)
        }

        #[cfg(not(fbcode_build))]
        {
            Ok(false)
        }
    }

    /// Resolves a cell path (i.e., contains `//`) into an absolute path. The cell path must have
    /// been split into two components: `cell_alias` and `cell_path`. For example, if the cell path
    /// is `cell//path/to/file`, then:
    ///   - `cell_alias` would be `cell`
    ///   - `cell_relative_path` would be `path/to/file`
    pub(crate) fn resolve_cell_path(
        &self,
        cell_alias: &str,
        cell_relative_path: &str,
    ) -> buck2_error::Result<AbsNormPathBuf> {
        let data = self.data()?;

        let cell = data.cwd_cell_alias_resolver.resolve(cell_alias)?;
        let cell = data.cell_resolver.get(cell)?;
        let path = cell.path().join_normalized(cell_relative_path)?;
        Ok(data.project_filesystem.resolve(&path))
    }

    pub(crate) fn resolve_project_path(
        &self,
        path: CellPath,
    ) -> buck2_error::Result<AbsNormPathBuf> {
        let data = self.data()?;
        Ok(data
            .project_filesystem
            .resolve(data.cell_resolver.resolve_path(path)?))
    }

    pub fn resolve_alias_to_path_in_cwd(
        &self,
        alias: &str,
    ) -> buck2_error::Result<CellRootPathBuf> {
        let data = self.data()?;
        let cell = data.cwd_cell_alias_resolver.resolve(alias)?;
        Ok(data.cell_resolver.get(cell)?.path().to_buf())
    }

    fn data(&self) -> buck2_error::Result<&ImmediateConfigContextData> {
        self.data
            .get_or_try_init(|| ImmediateConfigContextData::parse(find_invocation_roots(self.cwd)?))
            .buck_error_context("Error creating cell resolver")
    }

    pub(crate) fn resolve_argfile_kind(
        &self,
        canonicalized_path: AbsNormPathBuf,
        flag: Option<&str>,
    ) -> Result<buck2_common::argv::ArgFileKind, buck2_error::Error> {
        let is_py = canonicalized_path.extension() == Some("py".as_ref());
        let resolved_path =
            match self.data() {
                Ok(data) if canonicalized_path.starts_with(data.project_filesystem.root()) => {
                    ArgFilePath::Project(data.cell_resolver.get_cell_path_from_abs_path(
                        &canonicalized_path,
                        &data.project_filesystem,
                    )?)
                }
                _ => ArgFilePath::External(canonicalized_path),
            };
        if is_py {
            Ok(ArgFileKind::PythonExecutable(
                resolved_path,
                flag.map(ToOwned::to_owned),
            ))
        } else {
            Ok(ArgFileKind::Path(resolved_path))
        }
    }
}

fn is_paranoid_enabled(path: &AbsPath) -> buck2_error::Result<bool> {
    if let Some(p) = buck2_env!("BUCK_PARANOID", type=bool)? {
        return Ok(p);
    }

    let bytes = match fs_util::read_if_exists(path)? {
        Some(b) => b,
        None => return Ok(false),
    };

    let info = buck2_cli_proto::ParanoidInfo::decode(bytes.as_slice())
        .buck_error_context("Invalid data ")?;

    let now = SystemTime::now();
    let expires_at = SystemTime::try_from(info.expires_at.internal_error("Missing expires_at")?)
        .buck_error_context("Invalid expires_at")?;
    Ok(now < expires_at)
}

#[cfg(test)]
mod tests {
    use buck2_common::init::LogDownloadMethod;
    use buck2_fs::fs_util::uncategorized as fs_util;

    use super::*;

    fn working_dir(path: &AbsPath) -> buck2_error::Result<AbsWorkingDir> {
        Ok(AbsWorkingDir::unchecked_new(fs_util::canonicalize(path)?))
    }

    #[test]
    fn test_set_setting_arg_layers() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        fs_util::write(root.join(".buckconfig"), "[cells]\nroot = .")?;
        let cwd = working_dir(root)?;
        let project_root = ProjectRoot::new(cwd.path().to_buf())?;
        let context = ImmediateConfigContext::new(&cwd);

        assert!(
            context.buck_settings(&project_root).is_err(),
            "invocation settings require setting argument layers to be initialized"
        );
        assert!(
            context.daemon_startup_config().is_err(),
            "daemon startup config must not be available before setting argument layers are set"
        );
        context.set_setting_arg_layers(Vec::new())?;
        assert!(
            context.set_setting_arg_layers(Vec::new()).is_err(),
            "setting argument layers must only be set once"
        );

        Ok(())
    }

    #[test]
    fn test_settings_without_valid_build_config() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        fs_util::write(root.join(".buckconfig"), "invalid buckconfig")?;
        fs_util::write(root.join(".buckroot"), "")?;
        let cwd = working_dir(root)?;
        let project_root = ProjectRoot::new(cwd.path().to_buf())?;
        let context = ImmediateConfigContext::new(&cwd);
        context.set_setting_arg_layers(vec![toml::from_str(
            "[log_download]\nlog_url = \"https://command-line/\"",
        )?])?;

        assert_eq!(
            context
                .buck_settings(&project_root)?
                .daemon
                .log_download
                .log_url(&mut buck2_settings::NoBuckconfig)?
                .as_deref(),
            Some("https://command-line/")
        );
        assert!(
            context.daemon_startup_config().is_err(),
            "invalid build configuration must still prevent daemon configuration"
        );
        Ok(())
    }

    #[test]
    fn test_setting_arg_layers_applied() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = AbsPath::new(tempdir.path())?;
        fs_util::write(root.join(".buckconfig"), "[cells]\nroot = .")?;
        let cwd = working_dir(root)?;
        let context = ImmediateConfigContext::new(&cwd);

        // Ensure early cell initialization does not discard setting args.
        context.resolve_cell_path("", "")?;

        let mut log_download = toml::Table::new();
        log_download.insert("log_use_manifold".to_owned(), toml::Value::Boolean(false));
        log_download.insert(
            "log_url".to_owned(),
            toml::Value::String("setting_arg".to_owned()),
        );
        let mut setting_arg_layer = toml::Table::new();
        setting_arg_layer.insert("log_download".to_owned(), toml::Value::Table(log_download));

        context.set_setting_arg_layers(vec![setting_arg_layer])?;
        assert_eq!(
            context
                .daemon_startup_config()?
                .startup_options
                .log_download_method,
            LogDownloadMethod::Curl("setting_arg".to_owned())
        );

        Ok(())
    }
}
