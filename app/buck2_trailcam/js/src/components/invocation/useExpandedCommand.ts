/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

'use client';

import {useEffect, useState} from 'react';
import {useEventLog} from './EventLogProvider';
import {useBackend} from '../../backend';

/** Replace the first arg (binary path) with "buck2". */
function normalizeBinary(args: string[]): string[] {
  if (args.length === 0) return args;
  return ['buck2', ...args.slice(1)];
}

/**
 * Parse the `expanded_command` artifact: strip the leading `#`-prefixed
 * comment line, then collapse whitespace into a single command line.
 */
function parseExpandedCommandArtifact(raw: string): string {
  const lines = raw.split('\n');
  const stripped = lines[0]?.trimStart().startsWith('#')
    ? lines.slice(1)
    : lines;
  const args = stripped.join('\n').trim().split(/\s+/);
  return normalizeBinary(args).join(' ');
}

/**
 * Returns the expanded buck2 command line for the invocation, preferring the
 * event log's first message (which carries `expandedCommandLineArgs`) and
 * falling back to the host's `expanded_command` artifact only when the event
 * log isn't available (e.g. older builds with no log path).
 */
export function useExpandedCommand(buildUuid: string): string | null {
  const logState = useEventLog();
  const {fetchLogText} = useBackend();
  const [fromLog, setFromLog] = useState<string | null>(null);
  const [fromArtifact, setFromArtifact] = useState<string | null>(null);

  // Path 1: extract from the event log's first (Invocation) message.
  useEffect(() => {
    if (logState.status !== 'loaded' || logState.summaries.length === 0) {
      setFromLog(null);
      return;
    }
    let cancelled = false;
    (async () => {
      const invSummary = logState.summaries.get(0);
      if (invSummary.type !== 'invocation') return;
      const data = await logState.getEventDataAsync(invSummary);
      if (cancelled) return;
      const args = (data.expandedCommandLineArgs ??
        data.expanded_command_line_args) as string[] | undefined;
      if (!args || args.length === 0) return;
      setFromLog(normalizeBinary(args).join(' '));
    })();
    return () => {
      cancelled = true;
    };
  }, [logState]);

  // Path 2: only fetch the artifact when the event log is unavailable.
  useEffect(() => {
    if (logState.status !== 'error' || !fetchLogText) {
      setFromArtifact(null);
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const content = await fetchLogText(buildUuid, 'expanded_command');
        if (cancelled || content == null) return;
        setFromArtifact(parseExpandedCommandArtifact(content));
      } catch {
        // Silently ignore — the toggle just won't appear.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [logState.status, buildUuid, fetchLogText]);

  return fromLog ?? fromArtifact;
}
