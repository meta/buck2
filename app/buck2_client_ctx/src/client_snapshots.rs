/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::SystemTime;

use buck2_cli_proto::daemon_api_client::DaemonApiClient;
use buck2_events::BuckEvent;
use tonic::Request;
use tonic::codegen::InterceptedService;
use tonic::transport::Channel;

use crate::client_cpu_tracker::ClientCpuTracker;
use crate::daemon::client::BuckdClient;
use crate::daemon::client::connect::BuckAddAuthTokenInterceptor;

/// Makes a `ClientSnapshot` of this client for each `Snapshot` that it receives from the
/// daemon that runs its command, and sends it to that daemon as an event of the command.
pub(crate) struct ClientSnapshots {
    client: DaemonApiClient<InterceptedService<Channel, BuckAddAuthTokenInterceptor>>,
    cpu: ClientCpuTracker,
}

impl ClientSnapshots {
    pub(crate) fn new(daemon: &BuckdClient) -> Self {
        Self {
            client: daemon.client.clone(),
            cpu: ClientCpuTracker::new(),
        }
    }

    /// If `event`, which the client has just received, is a `Snapshot`: the `ClientSnapshot`
    /// made for it, which is on its way to the daemon.
    pub(crate) fn received(&mut self, event: &BuckEvent) -> Option<buck2_data::ClientSnapshot> {
        let buck2_data::buck_event::Data::Instant(instant) = event.data() else {
            return None;
        };
        let Some(buck2_data::instant_event::Data::Snapshot(_)) = &instant.data else {
            return None;
        };
        let trace_id = event.trace_id().ok()?;
        let now = SystemTime::now();
        let snapshot = buck2_data::ClientSnapshot {
            daemon_snapshot_delay_ms: delay_ms(event.timestamp(), now),
            cpu_percents: self.cpu.tick_cpu_time_percents(),
        };
        let instant = buck2_data::InstantEvent {
            data: Some(snapshot.into()),
        };
        let client_event = BuckEvent::new(now, trace_id, None, None, instant.into());
        let client_event = Box::<buck2_data::BuckEvent>::from(client_event);

        let mut client = self.client.clone();
        // The command's events do not wait for this. It fails with a daemon that does not
        // take events from clients, which is not this command's problem.
        tokio::spawn(async move {
            let _ignored = client.client_event(Request::new(*client_event)).await;
        });
        Some(snapshot)
    }
}

/// How long after `sent` is `received`, negative if it is before. `None` if the two are more
/// than `i64::MAX` milliseconds apart.
fn delay_ms(sent: SystemTime, received: SystemTime) -> Option<i64> {
    match received.duration_since(sent) {
        Ok(delay) => i64::try_from(delay.as_millis()).ok(),
        Err(e) => i64::try_from(e.duration().as_millis()).ok().map(|ms| -ms),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn test_delay_ms() {
        let sent = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let delay = Duration::from_millis(250);
        assert_eq!(delay_ms(sent, sent + delay), Some(250));
        assert_eq!(
            delay_ms(sent, sent - delay),
            Some(-250),
            "The client's clock is behind"
        );
        assert_eq!(delay_ms(sent, sent), Some(0));
    }
}
