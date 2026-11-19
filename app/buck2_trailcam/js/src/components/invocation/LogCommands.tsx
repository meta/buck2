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

import {useState, useCallback} from 'react';
import CopyButton from '../ui/CopyButton';
import {
  useBackend,
  type LogTextKind,
  type TrailcamBackend,
} from '../../backend';

const LOG_COMMANDS: {name: string; desc: string; extra?: string}[] = [
  {name: 'show', desc: 'Output log in JSON format'},
  {name: 'replay', desc: 'Replay the event log', extra: ' --speed 10'},
  {name: 'what-failed', desc: 'Show failed commands'},
  {name: 'what-materialized', desc: 'Show materializations'},
  {name: 'what-up', desc: 'Show spans open when log ended'},
  {name: 'what-uploaded', desc: 'Show upload stats to RE'},
  {name: 'cmd', desc: 'Show CLI args', extra: ' --expand'},
  {name: 'critical-path', desc: 'Show critical path'},
  {name: 'external-configs', desc: 'Show external config values'},
];

const VIEWABLE_LOGS: {logType: LogTextKind; label: string; desc: string}[] = [
  {logType: 'simpleconsole', label: 'Replay', desc: 'Console replay output'},
  {logType: 'whatran', label: 'What Ran', desc: 'Actions that were executed'},
  {
    logType: 'expanded_command',
    label: 'Expanded Command',
    desc: 'Full expanded CLI args',
  },
];

function InlineLogViewer({
  uuid,
  logType,
  label,
  fetchLogText,
}: {
  uuid: string;
  logType: LogTextKind;
  label: string;
  fetchLogText: NonNullable<TrailcamBackend['fetchLogText']>;
}) {
  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState(false);

  const fetchLog = useCallback(async () => {
    if (content != null) {
      setExpanded(!expanded);
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const logContent = await fetchLogText(uuid, logType);
      if (logContent == null) throw new Error('Not available');
      setContent(logContent);
      setExpanded(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load');
    } finally {
      setLoading(false);
    }
  }, [uuid, logType, content, expanded, fetchLogText]);

  return (
    <div>
      <button
        onClick={fetchLog}
        disabled={loading}
        className="text-xs text-blue-600 hover:underline disabled:opacity-50 dark:text-blue-400">
        {loading ? 'Loading...' : expanded ? `Hide ${label}` : `View ${label}`}
      </button>
      {error && (
        <p className="mt-1 text-xs text-red-600 dark:text-red-400">{error}</p>
      )}
      {expanded && content != null && (
        <pre className="mt-2 max-h-96 overflow-auto rounded bg-amber-50 p-2 font-mono text-xs dark:bg-amber-950">
          {content}
        </pre>
      )}
    </div>
  );
}

/**
 * Bare log commands and viewable logs. Designed to render inside the
 * DebugInfoPanel — no surrounding Card.
 */
export default function LogCommands({uuid}: {uuid: string}) {
  const {fetchLogText} = useBackend();
  return (
    <div className="space-y-3">
      {fetchLogText && (
        <div className="space-y-1.5">
          {VIEWABLE_LOGS.map(({logType, label, desc}) => (
            <div key={logType} className="flex items-center gap-2">
              <InlineLogViewer
                uuid={uuid}
                logType={logType}
                label={label}
                fetchLogText={fetchLogText}
              />
              <span className="text-muted-foreground text-[10px] truncate">
                — {desc}
              </span>
            </div>
          ))}
        </div>
      )}

      {/* Copyable CLI commands */}
      <div
        className={`space-y-1.5 ${fetchLogText ? 'border-t border-border pt-2' : ''}`}>
        <p className="text-muted-foreground text-[10px] font-semibold uppercase tracking-wide">
          CLI Commands
        </p>
        {LOG_COMMANDS.map(({name, desc, extra}) => {
          const cmd = `buck2 log ${name} --trace-id ${uuid}${extra ?? ''}`;
          return (
            <div key={name} className="flex items-start justify-between gap-2">
              <div className="min-w-0 flex-1">
                <code className="block truncate text-[10px]" title={cmd}>
                  {cmd}
                </code>
                <p className="text-muted-foreground text-[10px]">{desc}</p>
              </div>
              <CopyButton text={cmd} label="" />
            </div>
          );
        })}
      </div>
    </div>
  );
}
