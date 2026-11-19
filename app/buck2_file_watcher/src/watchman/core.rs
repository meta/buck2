/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;
use std::fmt::Display;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use buck2_certs::validate::validate_certs;
use buck2_common::manifold::Bucket;
use buck2_common::manifold::ManifoldClient;
use buck2_common::manifold::Ttl;
use buck2_core::buck2_env;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::ErrorTag;
use buck2_error::internal_error;
use dupe::Dupe;
use futures::future::Future;
use serde::Deserialize;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use watchman_client::prelude::*;

fn watchman_error_tag(e: &watchman_client::Error) -> ErrorTag {
    match e {
        watchman_client::Error::ConnectionError { .. } => ErrorTag::WatchmanConnectionError,
        watchman_client::Error::ConnectionLost(_) => ErrorTag::WatchmanConnectionLost,
        watchman_client::Error::ConnectionDiscovery { .. } => ErrorTag::WatchmanConnectionDiscovery,
        watchman_client::Error::WatchmanServerError { message, .. } => {
            if message.contains("RootNotConnectedError") {
                ErrorTag::WatchmanRootNotConnectedError
            } else if message
                .contains("cannot compute status while a checkout is currently in progress")
            {
                ErrorTag::WatchmanCheckoutInProgress
            } else {
                ErrorTag::WatchmanServerError
            }
        }
        watchman_client::Error::WatchmanResponseError { .. } => ErrorTag::WatchmanResponseError,
        watchman_client::Error::MissingField { .. } => ErrorTag::WatchmanMissingField,
        watchman_client::Error::Deserialize { .. } => ErrorTag::WatchmanDeserialize,
        watchman_client::Error::Serialize { .. } => ErrorTag::WatchmanSerialize,
        watchman_client::Error::Connect { .. } => ErrorTag::WatchmanConnect,
    }
}

#[derive(Debug, buck2_error::Error)]
enum WatchmanClientError {
    #[buck2(input)]
    #[error("Configured timeout is zero")]
    ZeroTimeout,
    #[buck2(tag = WatchmanTimeout)]
    #[error(
        "Watchman request timed out after {0}s; try restarting watchman, probably via `watchman shutdown-server`"
    )]
    Timeout(u64),
    #[buck2(tag = watchman_error_tag(&inner))]
    #[error(transparent)]
    RequestFailed { inner: watchman_client::Error },
}

// We use the "new" field. This is marked as deprecated, but buck1 uses it and
// I'm unaware of issues due to its use there.
//
// Putting this in it own mod was the best way to scope the allow(deprecated).
#[allow(deprecated)]
mod types {
    use super::*;
    query_result_type! {
        pub(crate) struct BuckQueryResult {
            name: NameField,
            file_type: FileTypeField,
            exists: ExistsField,
            new: NewField,
        }
    }

    impl BuckQueryResult {
        pub fn into_event(self) -> Option<WatchmanEvent> {
            let kind = match *self.file_type {
                FileType::BlockSpecial
                | FileType::CharSpecial
                | FileType::Fifo
                | FileType::Socket
                | FileType::SolarisDoor
                | FileType::Unknown => {
                    return None;
                }
                FileType::Directory => WatchmanKind::Directory,
                FileType::Regular => WatchmanKind::File,
                FileType::Symlink => WatchmanKind::Symlink,
            };

            let event = match (*self.exists, *self.new) {
                (true, true) => WatchmanEventType::Create,
                (false, _) => WatchmanEventType::Delete,
                (true, false) => WatchmanEventType::Modify,
            };

            Some(WatchmanEvent {
                kind,
                event,
                path: self.name.into_inner(),
            })
        }
    }
}

use types::*;

#[derive(Debug)]
pub(crate) enum WatchmanEventType {
    Create,
    Modify,
    Delete,
}

#[derive(Debug)]
pub(crate) enum WatchmanKind {
    File,
    Directory,
    Symlink,
}

impl WatchmanKind {
    fn symbol(&self) -> &'static str {
        match self {
            WatchmanKind::File => "f",
            WatchmanKind::Directory => "d",
            WatchmanKind::Symlink => "l",
        }
    }
}

#[derive(Debug)]
pub(crate) struct WatchmanEvent {
    pub kind: WatchmanKind,
    pub event: WatchmanEventType,
    pub path: PathBuf,
}

impl Display for WatchmanEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?}{}<{}>",
            self.event,
            self.kind.symbol(),
            self.path.to_string_lossy()
        )
    }
}

#[derive(Dupe, Clone)]
struct WatchmanClient(Arc<(watchman_client::Client, ResolvedRoot)>);

impl Debug for WatchmanClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "watchman<{:?}>", self.0.1)
    }
}

async fn with_timeout<R>(
    fut: impl Future<Output = Result<R, watchman_client::Error>> + Send,
) -> buck2_error::Result<R> {
    let timeout = buck2_env!("BUCK2_WATCHMAN_TIMEOUT", type=u64, default=57)?;
    if timeout == 0 {
        return Err(WatchmanClientError::ZeroTimeout.into());
    }
    match tokio::time::timeout(Duration::from_secs(timeout), fut).await {
        Ok(Ok(res)) => Ok(res),
        Ok(Err(e)) => {
            validate_certs()
                .await
                .buck_error_context("Watchman Request Failed")?;

            let request_err: buck2_error::Error =
                WatchmanClientError::RequestFailed { inner: e }.into();

            if request_err.has_tag(buck2_error::ErrorTag::WatchmanServerError) {
                Err(
                    request_err.context(get_watchman_eden_error_logs().await.unwrap_or(
                        "Attempted to retrieve Watchman and Eden rage logs but failed".to_owned(),
                    )),
                )
            } else {
                Err(request_err)
            }
        }
        Err(_) => {
            validate_certs()
                .await
                .buck_error_context("Watchman Timed Out")?;
            Err(WatchmanClientError::Timeout(timeout).into())
        }
    }
}

async fn write_to_manifold(buf: &[u8], name: &str) -> Option<String> {
    let manifold = ManifoldClient::new().await.ok()?;

    let filename = format!("flat/{}_{}_logs", uuid::Uuid::new_v4(), name);
    let ttl = Ttl::from_days(14); // 14 days should be plenty of time to take action

    let bucket = Bucket::RAGE_DUMPS;
    let mut cursor = std::io::Cursor::new(buf);

    manifold
        .read_and_upload(bucket, &filename, ttl, &mut cursor)
        .await
        .ok()?;

    let url = format!(
        "https://interncache-all.fbcdn.net/manifold/{}/{}",
        bucket.name, filename
    );

    Some(url)
}

async fn cmd_logs_to_manifold(cmd: &str, args: Vec<&str>) -> Option<String> {
    let async_cmd = buck2_util::process::async_background_command(cmd)
        .args(args)
        .output()
        .await;

    if let Ok(result) = async_cmd {
        if result.status.success() {
            return write_to_manifold(&result.stdout, cmd).await;
        }
    }

    None
}

// Best effort to get watchman and eden rage logs to help with debugging.
async fn get_watchman_eden_error_logs() -> Option<String> {
    let watchman_cmd = cmd_logs_to_manifold("watchmanctl", vec!["rage"]);
    let eden_cmd = cmd_logs_to_manifold("eden", vec!["debug", "log", "--full", "--stdout"]);

    let (watchman_cmd, eden_cmd) = tokio::join!(watchman_cmd, eden_cmd);

    match (watchman_cmd, eden_cmd) {
        (Some(watchman_cmd), Some(eden_cmd)) => Some(format!(
            "Watchman and Eden rage logs:\n{watchman_cmd}\n{eden_cmd}"
        )),
        (Some(watchman_cmd), None) => Some(format!("Watchman rage logs:\n{watchman_cmd}")),
        (None, Some(eden_cmd)) => Some(format!("Eden rage logs:\n{eden_cmd}")),
        (None, None) => None,
    }
}

impl WatchmanClient {
    async fn connect(
        connector: &Connector,
        path: CanonicalPath,
    ) -> buck2_error::Result<WatchmanClient> {
        let client = with_timeout(connector.connect())
            .await
            .buck_error_context("Connecting to watchman")?;
        let root = with_timeout(client.resolve_root(path))
            .await
            .buck_error_context("Resolving watchman root")?;
        Ok(Self(Arc::new((client, root))))
    }

    async fn query<F: serde::de::DeserializeOwned + std::fmt::Debug + Clone + QueryFieldList>(
        &self,
        query: QueryRequestCommon,
    ) -> buck2_error::Result<QueryResult<F>> {
        let fut = self.client().query(self.root(), query);

        with_timeout(fut).await
    }

    fn root(&self) -> &ResolvedRoot {
        &self.0.1
    }

    fn client(&self) -> &watchman_client::Client {
        &self.0.0
    }
}

#[async_trait]
pub(crate) trait SyncableQueryProcessor: Send + Sync {
    type Output;
    type Payload;

    /// Process a set of filesystem change events.
    async fn process_events(
        &mut self,
        payload: Self::Payload,
        events: Vec<WatchmanEvent>,
        mergebase: &Option<String>,
        watchman_version: Option<String>,
    ) -> buck2_error::Result<(Self::Output, Self::Payload)>;

    /// Indicates that all derived data should be invalidated. This could happen, for example, if the watchman server restarts.
    ///
    /// `events_since_mergebase` says whether `events` lists every file that differs from
    /// `mergebase`, rather than what changed since the previous query.
    async fn on_fresh_instance(
        &mut self,
        dice: Self::Payload,
        events: Vec<WatchmanEvent>,
        mergebase: &Option<String>,
        watchman_version: Option<String>,
        events_since_mergebase: bool,
    ) -> buck2_error::Result<(Self::Output, Self::Payload)>;
}

/// commands to be sent to the SyncableQueryHandler.
enum SyncableQueryCommand<T, P> {
    Sync(P, oneshot::Sender<buck2_error::Result<(T, P)>>),
}

/// A SyncableQuery is similar to a subscription. When created, it accepts a query expression
/// and a SyncableQueryProcessor. It can be used to maintain some state that depends on the
/// results of the query expression. As changes are made to the filesystem, the SyncableQueryProcessor
/// will be called to process the events.
///
/// It provides a single function `sync()` that will ensure that all file events seen by watchman have
/// been processed by the SyncableQueryProcessor.
///
/// In the background, the SyncableQuery may use a subscription to eagerly process updates, but this is
/// only an optimization and users should use `sync()` when they want events to have been processed.
pub struct SyncableQuery<T, P> {
    control_tx: UnboundedSender<SyncableQueryCommand<T, P>>,
}

enum WatchmanSyncResult {
    FreshInstance {
        events: Vec<WatchmanEvent>,
        merge_base: Option<String>,
        clock: ClockSpec,
        watchman_version: Option<String>,
    },
    Events {
        events: Vec<WatchmanEvent>,
        merge_base: Option<String>,
        clock: ClockSpec,
        watchman_version: Option<String>,
    },
}

/// The SyncableQueryHandler runs a processing loop that communicates with watchman and invokes the SyncableQueryProcessor. As
/// these only happen within the handler's run loop, for a particular SyncableQuery all watchman invocations and processor
/// processing will happen in a linear order.
///
/// The SyncableQueryHandler maintains the clock and last mergebase and updates them with each request.
struct SyncableQueryHandler<T, P> {
    connector: Connector,
    path: CanonicalPath,
    processor: Box<dyn SyncableQueryProcessor<Output = T, Payload = P>>,
    query: QueryRequestCommon,
    last_clock: ClockSpec,
    last_mergebase: Option<String>,
    mergebase_with: Option<String>,
    dice_clear_on_mergebase_change: bool,
    control_rx: UnboundedReceiver<SyncableQueryCommand<T, P>>,
}

impl<T, P> SyncableQueryHandler<T, P>
where
    T: Send + 'static,
    P: Send + 'static,
{
    async fn run_loop(mut self) {
        // We discard the first error here, if there is one. It's a bit unfortunate but it makes
        // everything a lot simpler below, and kicking it off earlier is desirable because it can
        // give Watchman time to warm up.
        let mut client = None;
        if let Err(e) = self.reconnect(&mut client).await {
            tracing::debug!("Connecting to Watchman failed (will re-attempt): {:#}", e);
        };

        loop {
            match self.control_rx.recv().await {
                Some(SyncableQueryCommand::Sync(dice, sync_tx)) => {
                    let res = self.sync(dice, &mut client).await;

                    // NOTE: If the receiver is gone, then they won't be told we finished their
                    // job. That's fine.
                    let _ignore = sync_tx.send(res);
                }
                None => {
                    // This indicates the controlling SyncableQuery has been dropped.
                    return;
                }
            }
        }
    }

    /// sync() will send a since query to watchman and invoke the processor
    /// with either the received events or a fresh instance call.
    async fn sync(
        &mut self,
        payload: P,
        client: &mut Option<WatchmanClient>,
    ) -> buck2_error::Result<(T, P)> {
        let sync_res = match self.sync_query(client).await {
            Ok(res) => Ok(res),
            Err(e) => self
                .reconnect_and_sync_query(client)
                .await
                .buck_error_context(e.to_string()),
        }?;

        let (res, new_mergebase, clock) = match sync_res {
            WatchmanSyncResult::Events {
                events,
                merge_base,
                clock,
                watchman_version,
            } => {
                if self.mergebase_with.is_none()
                    || self.last_mergebase.is_some() && self.last_mergebase == merge_base
                    || !self.dice_clear_on_mergebase_change
                {
                    (
                        self.processor
                            .process_events(payload, events, &merge_base, watchman_version)
                            .await?,
                        merge_base,
                        clock,
                    )
                } else {
                    (
                        self.processor
                            .on_fresh_instance(
                                payload,
                                events,
                                &merge_base,
                                watchman_version,
                                false,
                            )
                            .await?,
                        merge_base,
                        clock,
                    )
                }
            }
            WatchmanSyncResult::FreshInstance {
                events,
                merge_base,
                clock,
                watchman_version,
            } => (
                self.processor
                    .on_fresh_instance(
                        payload,
                        events,
                        &merge_base,
                        watchman_version,
                        self.mergebase_with.is_some() && !self.query.empty_on_fresh_instance,
                    )
                    .await?,
                merge_base,
                clock,
            ),
        };

        self.last_mergebase = new_mergebase;
        self.last_clock = clock;

        Ok(res)
    }

    async fn reconnect(&mut self, client: &mut Option<WatchmanClient>) -> buck2_error::Result<()> {
        self.last_clock = Default::default();
        self.last_mergebase = None;
        *client = Some(
            WatchmanClient::connect(&self.connector, self.path.clone())
                .await
                .buck_error_context("Error reconnecting to Watchman")?,
        );
        Ok(())
    }

    async fn reconnect_and_sync_query(
        &mut self,
        client: &mut Option<WatchmanClient>,
    ) -> buck2_error::Result<WatchmanSyncResult> {
        self.reconnect(client).await?;

        let out = self.sync_query(client).await?;

        Ok(out)
    }

    async fn sync_query(
        &mut self,
        client: &mut Option<WatchmanClient>,
    ) -> buck2_error::Result<WatchmanSyncResult> {
        let client = client.as_mut().internal_error("No Watchman connection")?;

        let make_query = |last_clock, last_mergebase| {
            let mut query = self.query.clone();
            query.since = if let Some(mergebase_with) = self.mergebase_with.as_ref() {
                Some(Clock::ScmAware(FatClockData {
                    clock: last_clock,
                    scm: Some(ScmAwareClockData {
                        mergebase: last_mergebase,
                        mergebase_with: Some(mergebase_with.clone()),
                        saved_state: None,
                    }),
                }))
            } else {
                Some(Clock::Spec(last_clock))
            };
            query
        };

        let mut query = make_query(self.last_clock.clone(), self.last_mergebase.clone());

        // watchman currently has a performance problem when using scm-aware queries and empty_on_fresh_instance=false
        // in that scenario, you would expect that when the merge base changes the performance with and without a since clock
        // would have the same performance as both simply need to return the new mergebase and the changes since the mergebase,
        // but actually watchman is extremely slow when including the previous clock.
        //
        // To work around that, in our queries we pass empty_on_fresh_instance=true and then on a fresh instance send a new
        // query with empty_on_fresh_instance=false and no previous clock.

        let needs_watchman_perf_workaround =
            self.last_mergebase.is_some() && !query.empty_on_fresh_instance;

        if needs_watchman_perf_workaround {
            query.empty_on_fresh_instance = true;
        }

        let mut query_result = client.query::<BuckQueryResult>(query).await?;
        if needs_watchman_perf_workaround && query_result.is_fresh_instance {
            let query = make_query(ClockSpec::default(), None);
            query_result = client.query::<BuckQueryResult>(query).await?;
        }

        let QueryResult {
            version,
            is_fresh_instance,
            files,
            clock,
            // subscription_canceled,
            // state_enter,
            // state_leave,
            // state_metadata,
            // saved_state_info,
            ..
        } = query_result;

        // While we use scm-based queries, the processor api doesn't really support them yet so we just treat it as a fresh instance.
        let (new_mergebase, clock) = unpack_clock(clock);

        // When the mergebase moved and the caller handles that incrementally (no DICE clear),
        // the scm-aware result lists only the files that differ from the *new* mergebase: it
        // misses files changed between the two mergebases and local edits the checkout dropped.
        // Ask for everything since the last clock instead.
        let (is_fresh_instance, files, clock) = if !is_fresh_instance
            && !self.dice_clear_on_mergebase_change
            && self.last_mergebase.is_some()
            && new_mergebase != self.last_mergebase
        {
            let mut query = self.query.clone();
            query.since = Some(Clock::Spec(self.last_clock.clone()));
            let since_result = client.query::<BuckQueryResult>(query).await?;
            let (_, clock) = unpack_clock(since_result.clock);
            (since_result.is_fresh_instance, since_result.files, clock)
        } else {
            (is_fresh_instance, files, clock)
        };

        let events = match files {
            None if is_fresh_instance => vec![],
            None => {
                return Err(internal_error!(
                    "unexpected missing files in watchman query"
                ));
            }
            Some(v) => v.into_iter().filter_map(|f| f.into_event()).collect(),
        };

        Ok(if is_fresh_instance {
            WatchmanSyncResult::FreshInstance {
                events,
                merge_base: new_mergebase,
                clock,
                watchman_version: Some(version),
            }
        } else {
            WatchmanSyncResult::Events {
                events,
                merge_base: new_mergebase,
                clock,
                watchman_version: Some(version),
            }
        })
    }
}

/// Unpacks the clock returned for an scm-aware query into a tuple of the mergebase and the clockspec.
fn unpack_clock(clock: Clock) -> (Option<String>, ClockSpec) {
    match clock {
        Clock::Spec(clock_spec) => (None, clock_spec),
        Clock::ScmAware(FatClockData {
            clock: clock_spec,
            scm:
                Some(ScmAwareClockData {
                    mergebase: Some(mergebase),
                    ..
                }),
        }) => (Some(mergebase), clock_spec),
        clock => panic!("requested watchman query, got unexpected clock `{clock:?}`"),
    }
}

impl<T, P> SyncableQuery<T, P>
where
    T: Send + 'static,
    P: Send + 'static,
{
    /// Ensures that the processor has been sent all changes that watchman has seen.
    pub(crate) fn sync(
        &self,
        dice: P,
    ) -> impl Future<Output = buck2_error::Result<(T, P)>> + Send + 'static + use<T, P> {
        let (sync_done_tx, sync_done_rx) = tokio::sync::oneshot::channel();
        let tx_res = self
            .control_tx
            .send(SyncableQueryCommand::Sync(dice, sync_done_tx));

        async move {
            tx_res
                .ok()
                .internal_error("SyncableQueryHandler has exited")?;

            let out = sync_done_rx
                .await
                .buck_error_context(
                    "SyncableQueryHandler did not return a response for sync request",
                )?
                .buck_error_context("SyncableQueryHandler returned an error")?;

            Ok(out)
        }
    }

    pub(crate) fn new(
        connector: Connector,
        path: impl AsRef<Path>,
        expr: Expr,
        processor: Box<dyn SyncableQueryProcessor<Output = T, Payload = P>>,
        mergebase_with: Option<String>,
        empty_on_fresh_instance: bool,
        dice_clear_on_mergebase_change: bool,
    ) -> buck2_error::Result<SyncableQuery<T, P>> {
        let path = path.as_ref();
        let path = CanonicalPath::canonicalize(path)
            .with_buck_error_context(|| format!("Error canonicalizing: `{}`", path.display()))?;

        let query = QueryRequestCommon {
            expression: Some(expr),
            fields: vec!["name"],
            empty_on_fresh_instance,
            relative_root: None,
            case_sensitive: true,
            dedup_results: false,
            // Required or we miss directory events
            always_include_directories: true,
            // TODO(cjhopman): Figure out reasonable timeouts.
            // sync_timeout: ???,
            // lock_timeout: ???,
            ..QueryRequestCommon::default()
        };

        let (control_tx, control_rx) =
            tokio::sync::mpsc::unbounded_channel::<SyncableQueryCommand<T, P>>();

        tokio::spawn(async move {
            let handler = SyncableQueryHandler {
                connector,
                path,
                query,
                last_clock: ClockSpec::default(),
                last_mergebase: None,
                mergebase_with,
                dice_clear_on_mergebase_change,
                processor,
                control_rx,
            };
            handler.run_loop().await
        });

        Ok(Self { control_tx })
    }
}
