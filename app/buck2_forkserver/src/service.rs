/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::ffi::OsStr;
#[cfg(all(fbcode_build, target_os = "linux"))]
use std::fs::OpenOptions;
use std::os::unix::ffi::OsStrExt;
#[cfg(all(fbcode_build, target_os = "linux"))]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Command;
use std::sync::Arc;

use buck2_common::convert::ProstDurationExt;
use buck2_core::logging::LogConfigurationReloadHandle;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::internal_error;
use buck2_execute_local::DefaultKillProcess;
use buck2_execute_local::GatherOutputStatus;
use buck2_execute_local::StdRedirectPaths;
use buck2_execute_local::maybe_absolutize_exe;
use buck2_execute_local::spawn_command_and_stream_events;
use buck2_execute_local::status_decoder::DefaultStatusDecoder;
use buck2_execute_local::status_decoder::MiniperfStatusDecoder;
use buck2_forkserver_proto::CommandRequest;
use buck2_forkserver_proto::RequestEvent;
use buck2_forkserver_proto::SetLogFilterRequest;
use buck2_forkserver_proto::SetLogFilterResponse;
use buck2_forkserver_proto::forkserver_server::Forkserver;
#[cfg(all(fbcode_build, target_os = "linux"))]
use buck2_fs::error::IoResultExt;
#[cfg(all(fbcode_build, target_os = "linux"))]
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::abs_path::AbsPath;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_grpc::to_tonic;
use buck2_resource_control::ActionFreezeEvent;
use buck2_resource_control::ActionFreezeEventReceiver;
use buck2_resource_control::path::CgroupPathBuf;
use buck2_util::process::background_command;
use futures::FutureExt;
use futures::pin_mut;
use futures::stream::Stream;
use futures::stream::StreamExt;
use rand::distr::Alphanumeric;
use rand::distr::SampleString;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tonic::Request;
use tonic::Response;
use tonic::Status;
use tonic::Streaming;

use crate::convert::encode_event_stream;

// Not quite BoxStream: it has to be Sync (...)
type RunStream =
    Pin<Box<dyn Stream<Item = Result<buck2_forkserver_proto::CommandEvent, Status>> + Send>>;

struct ValidatedCommand {
    exe: PathBuf,
    argv: Vec<Vec<u8>>,
    env: Vec<buck2_forkserver_proto::EnvDirective>,
    cwd: PathBuf,
    timeout: Option<std::time::Duration>,
    enable_miniperf: bool,
    std_redirects: Option<StdRedirectPaths>,
    graceful_shutdown_timeout_s: Option<u32>,
    timeout_grace_period_s: Option<u32>,
    command_cgroup: Option<CgroupPathBuf>,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    network_access: Option<buck2_data::NetworkAccess>,
    kill_detached_descendants: bool,
}

impl ValidatedCommand {
    fn try_from(cmd_request: CommandRequest) -> buck2_error::Result<Self> {
        let CommandRequest {
            exe,
            argv,
            env,
            cwd,
            timeout,
            enable_miniperf,
            std_redirects,
            graceful_shutdown_timeout_s,
            command_cgroup,
            network_access,
            timeout_grace_period_s,
            kill_detached_descendants,
        } = cmd_request;

        let exe = OsStr::from_bytes(&exe);
        let cwd = OsStr::from_bytes(&cwd.as_ref().internal_error("Missing cwd")?.path);
        let cwd = AbsPath::new(Path::new(cwd)).buck_error_context("Invalid cwd")?;

        let timeout = timeout
            .map(|t| t.try_into_duration())
            .transpose()
            .buck_error_context("Invalid timeout")?;

        let exe = maybe_absolutize_exe(exe, cwd)?;

        let command_cgroup = command_cgroup.map(|cg| {
            CgroupPathBuf::new(AbsNormPathBuf::new(cg.into()).expect("Set correctly by caller"))
        });

        let std_redirects = std_redirects.map(|std_redirects| StdRedirectPaths {
            stdout: AbsNormPathBuf::new(std_redirects.stdout.into())
                .expect("Set correctly by caller"),
            stderr: AbsNormPathBuf::new(std_redirects.stderr.into())
                .expect("Set correctly by caller"),
        });

        Ok(ValidatedCommand {
            exe: exe.into_owned(),
            argv,
            env,
            cwd: cwd.to_path_buf(),
            timeout,
            enable_miniperf,
            std_redirects,
            graceful_shutdown_timeout_s,
            timeout_grace_period_s,
            command_cgroup,
            network_access: network_access
                .map(buck2_data::NetworkAccess::try_from)
                .transpose()
                .map_err(|v| internal_error!("Invalid network_access value: {}", v))?,
            kill_detached_descendants: kill_detached_descendants.unwrap_or(false),
        })
    }
}

pub(crate) struct UnixForkserverService {
    log_reload_handle: Arc<dyn LogConfigurationReloadHandle>,

    /// State for Miniperf.
    miniperf: Option<MiniperfContainer>,
}

impl UnixForkserverService {
    pub(crate) fn new(
        log_reload_handle: Arc<dyn LogConfigurationReloadHandle>,
        state_dir: &AbsNormPath,
    ) -> buck2_error::Result<Self> {
        let miniperf = MiniperfContainer::new(state_dir)?;
        Ok(Self {
            log_reload_handle,
            miniperf,
        })
    }

    async fn parse_command_request(
        stream: &mut Streaming<RequestEvent>,
    ) -> buck2_error::Result<CommandRequest> {
        let cmd_request = stream
            .message()
            .await?
            .and_then(|m| m.data)
            .and_then(|m| match m {
                buck2_forkserver_proto::request_event::Data::CommandRequest(r) => Some(r),
                _ => None,
            })
            .internal_error("RequestEvent was not a CommandRequest!")?;
        Ok(cmd_request)
    }

    fn configure_environment(
        cmd: &mut std::process::Command,
        env_directives: &[buck2_forkserver_proto::EnvDirective],
    ) -> buck2_error::Result<()> {
        use buck2_forkserver_proto::env_directive::Data;

        for directive in env_directives {
            match directive
                .data
                .as_ref()
                .internal_error("EnvDirective is missing data")?
            {
                Data::Clear(..) => {
                    cmd.env_clear();
                }
                Data::Set(var) => {
                    cmd.env(OsStr::from_bytes(&var.key), OsStr::from_bytes(&var.value));
                }
                Data::Remove(var) => {
                    cmd.env_remove(OsStr::from_bytes(&var.key));
                }
            }
        }
        Ok(())
    }

    fn setup_process_command(
        &self,
        validated_cmd: &ValidatedCommand,
    ) -> buck2_error::Result<(Command, Option<AbsNormPathBuf>)> {
        let (mut cmd, miniperf_output) = match (validated_cmd.enable_miniperf, &self.miniperf) {
            // Wraps the user command with miniperf for performance monitoring
            (true, Some(miniperf)) => {
                let mut cmd = background_command(miniperf.miniperf.as_path());
                let output_path = miniperf.allocate_output_path();
                cmd.arg(output_path.as_path());
                cmd.arg(&validated_cmd.exe);
                (cmd, Some(output_path))
            }
            // Direct execution of the command
            _ => (background_command(&validated_cmd.exe), None),
        };

        cmd.current_dir(&validated_cmd.cwd);
        cmd.args(validated_cmd.argv.iter().map(|a| OsStr::from_bytes(a)));

        Self::configure_environment(&mut cmd, &validated_cmd.env)?;

        #[cfg(target_os = "linux")]
        if validated_cmd
            .network_access
            .is_some_and(is_restricted_network_access)
        {
            #[cfg(fbcode_build)]
            {
                cmd.env("INSIDE_NETWORK_ISOLATION", "1");
                cmd.env("DOTSLASH_OFFLINE", "1");
            }

            use std::os::unix::process::CommandExt;
            // Safety: unshare() is async-signal-safe.
            // It only makes a single syscall with no memory allocation.
            unsafe {
                cmd.pre_exec(|| {
                    nix::sched::unshare(
                        nix::sched::CloneFlags::CLONE_NEWUSER
                            | nix::sched::CloneFlags::CLONE_NEWNET,
                    )
                    .map_err(|e| std::io::Error::from_raw_os_error(e as i32))
                });
            }
        }

        // cmd: ready-to-spawn process command
        // miniperf_output: path to miniperf output file (if monitoring)
        Ok((cmd, miniperf_output))
    }

    async fn create_command_stream(
        cmd: Command,
        validated_cmd: ValidatedCommand,
        cancellation: impl futures::Future<Output = buck2_error::Result<GatherOutputStatus>>
        + Send
        + Unpin
        + 'static,
        miniperf_output: Option<AbsNormPathBuf>,
        freeze_rx: impl ActionFreezeEventReceiver,
    ) -> buck2_error::Result<RunStream> {
        let timeout = validated_cmd.timeout;
        let kill_process = DefaultKillProcess {
            graceful_shutdown_timeout_s: validated_cmd.graceful_shutdown_timeout_s,
            timeout_grace_period_s: validated_cmd.timeout_grace_period_s,
            kill_detached_descendants: validated_cmd.kill_detached_descendants,
        };
        let std_redirects = validated_cmd.std_redirects;
        let cgroup = validated_cmd.command_cgroup.clone();
        let stream = match miniperf_output {
            Some(out) => spawn_command_and_stream_events(
                cmd,
                timeout,
                cancellation,
                MiniperfStatusDecoder::new(out),
                kill_process,
                std_redirects,
                false,
                cgroup,
                freeze_rx,
            )
            .await?
            .left_stream(),
            None => spawn_command_and_stream_events(
                cmd,
                timeout,
                cancellation,
                DefaultStatusDecoder,
                kill_process,
                std_redirects,
                false,
                cgroup,
                freeze_rx,
            )
            .await?
            .right_stream(),
        };

        let stream = encode_event_stream(stream);
        Ok(Box::pin(stream) as _)
    }
}

#[cfg(target_os = "linux")]
fn is_restricted_network_access(network_access: buck2_data::NetworkAccess) -> bool {
    match network_access {
        buck2_data::NetworkAccess::All => false,
        buck2_data::NetworkAccess::None
        | buck2_data::NetworkAccess::Loopback
        | buck2_data::NetworkAccess::Strict
        | buck2_data::NetworkAccess::Private => true,
    }
}

#[async_trait::async_trait]
impl Forkserver for UnixForkserverService {
    type RunStream = RunStream;

    async fn run(
        &self,
        req: Request<Streaming<RequestEvent>>,
    ) -> Result<Response<Self::RunStream>, Status> {
        to_tonic(async move {
            let mut stream = req.into_inner();

            let cmd_request = Self::parse_command_request(&mut stream).await?;
            let validated_cmd = ValidatedCommand::try_from(cmd_request)?;

            let (cancel_tx, cancel_rx) = oneshot::channel();
            let (freeze_tx, freeze_rx) = mpsc::unbounded_channel();

            // A task that splits the incoming events up into the freeze requests and cancel request
            tokio::spawn(async move {
                pin_mut!(stream);
                while let Some(m) = stream.next().await {
                    let err = match m {
                        Ok(m) => match m.data {
                            Some(d) => match d {
                                buck2_forkserver_proto::request_event::Data::CommandRequest(_) => {
                                    buck2_error::internal_error!("Non-leading command request")
                                }
                                buck2_forkserver_proto::request_event::Data::CancelRequest(_) => {
                                    drop(cancel_tx.send(Ok(GatherOutputStatus::Cancelled)));
                                    break;
                                }
                                buck2_forkserver_proto::request_event::Data::FreezeRequest(_) => {
                                    drop(freeze_tx.send(ActionFreezeEvent::Freeze));
                                    continue;
                                }
                                buck2_forkserver_proto::request_event::Data::UnfreezeRequest(_) => {
                                    drop(freeze_tx.send(ActionFreezeEvent::Unfreeze));
                                    continue;
                                }
                            },
                            None => continue,
                        },
                        Err(e) => e.into(),
                    };
                    drop(cancel_tx.send(Err(err)));
                    break;
                }
            });

            let cancel = cancel_rx.map(|r| match r {
                Ok(res) => res,
                Err(_) => Ok(GatherOutputStatus::Cancelled),
            });

            let (cmd, miniperf_output) = self.setup_process_command(&validated_cmd)?;

            let stream = Self::create_command_stream(
                cmd,
                validated_cmd,
                cancel,
                miniperf_output,
                UnboundedReceiverStream::new(freeze_rx),
            )
            .await?;

            Ok(stream)
        })
        .await
    }

    async fn set_log_filter(
        &self,
        req: Request<SetLogFilterRequest>,
    ) -> Result<Response<SetLogFilterResponse>, Status> {
        self.log_reload_handle
            .update_log_filter(&req.get_ref().log_filter)
            .buck_error_context("Error updating forkserver filter")
            .map_err(|e| Status::invalid_argument(format!("{e:#}")))?;

        Ok(Response::new(SetLogFilterResponse {}))
    }
}

struct MiniperfContainer {
    /// The Miniperf binary
    miniperf: AbsNormPathBuf,

    /// The directory where Miniperf outputs go.
    output_dir: AbsNormPathBuf,
}

impl MiniperfContainer {
    /// Must match the section key in `//buck2/app/buck2:buck2-bin`.
    #[cfg(all(fbcode_build, target_os = "linux"))]
    const SECTION_NAME: &'static str = "buck2_miniperf";

    fn new(forkserver_state_dir: &AbsNormPath) -> buck2_error::Result<Option<Self>> {
        #[cfg(not(all(fbcode_build, target_os = "linux")))]
        {
            let _ = forkserver_state_dir;
            return Ok(None);
        };

        #[cfg(all(fbcode_build, target_os = "linux"))]
        {
            let miniperf =
                forkserver_state_dir.join(ForwardRelativePath::unchecked_new("miniperf"));
            let output_dir = forkserver_state_dir.join(ForwardRelativePath::unchecked_new("out"));

            fs_util::remove_all(&miniperf).categorize_internal()?;
            fs_util::remove_all(&output_dir).categorize_internal()?;
            fs_util::create_dir_all(&output_dir)?;

            let mut opts = OpenOptions::new();
            opts.create_new(true);
            opts.write(true);

            opts.mode(0o755);

            let mut miniperf_writer = opts
                .open(miniperf.as_path())
                .with_buck_error_context(|| format!("Error opening: `{}`", miniperf.display()))?;

            let section = buck2_embedded_section::EmbeddedSection {
                name: Self::SECTION_NAME,
                encoding: buck2_embedded_section::SectionEncoding::Zstd,
            };

            section
                .copy_to(&mut miniperf_writer)
                .with_buck_error_context(|| {
                    format!("Error writing miniperf to `{}`", miniperf.display())
                })?;

            Ok(Some(Self {
                miniperf,
                output_dir,
            }))
        }
    }

    fn allocate_output_path(&self) -> AbsNormPathBuf {
        let name = Alphanumeric.sample_string(&mut rand::rng(), 16);
        self.output_dir
            .join(ForwardRelativePath::unchecked_new(&name))
    }
}
