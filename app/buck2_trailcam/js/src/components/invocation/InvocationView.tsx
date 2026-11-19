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

import {useMemo, useState, type ReactNode} from 'react';
import {BugIcon} from 'lucide-react';
import {Tabs, TabsContent, TabsList, TabsTrigger} from '../../ui';
import type {InvocationInfo, InvocationMetrics} from '../../invocation';
import {useUrlState} from '../../lib/url-state';
import InvocationBar from './InvocationBar';
import StatsPanel from './StatsPanel';
import MetadataSection from './MetadataSection';
import REStatsSection from './REStatsSection';
import LogsSection from './LogsSection';
import LogCommands from './LogCommands';
import OutputTab from './OutputTab';
import PerformanceTab from './PerformanceTab';
import DecodeStatsPanel from './DecodeStatsPanel';
import DecodeTimeline from './DecodeTimeline';
import CriticalPathChart from './CriticalPathChart';
import SpeedDoctor from './SpeedDoctor';
import EventLogViewer from './EventLogViewer';
import {EventLogProvider, useEventLog} from './EventLogProvider';
import FileChanges from './FileChanges';
import TestSummaryCard from './TestSummaryCard';
import TestResultsTab from './TestResultsTab';
import ActionResultsTab from './ActionResultsTab';
import FailedActionsCard from './FailedActionsCard';
import CollapsibleMessage from '../ui/CollapsibleMessage';
import DebugInfoPanel, {DebugInfoSection} from './DebugInfoPanel';

/** Ids of the tabs the view always renders, for positioning extra tabs. */
export type BuiltinTabId =
  'overview' | 'tests' | 'actions' | 'output' | 'performance' | 'logs';

export interface ExtraTab {
  id: string;
  label: string;
  content: ReactNode;
  /** Built-in tab to insert after; appended at the end when absent. */
  after?: BuiltinTabId;
}

export interface InvocationViewProps {
  info: InvocationInfo;
  /** Host-measured machine facts; null when the host has none. */
  metrics?: InvocationMetrics | null;
  /** Rendered above the tab bar, e.g. incident banners. */
  banner?: ReactNode;
  extraTabs?: readonly ExtraTab[];
  /**
   * Sections for the overview's debug panel. Rendered between the built-in
   * Logs and Log Commands sections.
   */
  debugSections?: ReadonlyArray<{title: string; content: ReactNode}>;
  /** Extra `MetaRow`s for the Metadata card. */
  metadataRows?: ReactNode;
  /** See `TestResultsTab`. */
  testInfraResults?: ReactNode;
}

// Inactive tabs get a subtle hover tint so the cursor reads as clickable.
// (Active tab is indicated by the sliding pill.)
const tabTriggerHoverClass =
  'cursor-pointer transition-colors data-[state=inactive]:hover:bg-[var(--background)]/70';

interface TabDef {
  id: string;
  label: string;
  content: ReactNode;
  /** Whether the panel scrolls itself (true) or lays out a full-height column. */
  scroll: boolean;
}

function orderTabs(
  builtin: readonly TabDef[],
  extra: readonly ExtraTab[],
): TabDef[] {
  const tabs = [...builtin];
  for (const t of extra) {
    const def: TabDef = {
      id: t.id,
      label: t.label,
      content: t.content,
      scroll: false,
    };
    const i = t.after ? tabs.findIndex(b => b.id === t.after) : -1;
    if (i < 0) tabs.push(def);
    else tabs.splice(i + 1, 0, def);
  }
  return tabs;
}

/**
 * The whole invocation page below the host's chrome: the command bar, the
 * tab strip and every tab. Host-specific content comes in through the slot
 * props; everything else is driven by `info` and the event log.
 */
export default function InvocationView({
  info,
  metrics = null,
  banner,
  extraTabs = [],
  debugSections = [],
  metadataRows,
  testInfraResults,
}: InvocationViewProps) {
  const uuid = info.uuid;
  const [tab, setTab] = useUrlState('tab', 'overview');
  const [showDecodeDebug, setShowDecodeDebug] = useState(false);
  const startTimeMs =
    info.startTime != null && info.startTime > 0 ? info.startTime * 1000 : null;

  const builtinTabs: TabDef[] = [
    {
      id: 'overview',
      label: 'Overview',
      scroll: true,
      content: (
        <>
          <TopLevelErrors errorMessages={info.errorMessages} />
          <div className="flex gap-6">
            {/* Stats panel */}
            <div className="flex w-80 shrink-0 flex-col gap-4">
              <StatsPanel
                durationMs={info.durationMs}
                commandDurationMs={info.commandDurationMs}
                commandOutcome={info.commandOutcome}
                startTime={info.startTime}
                creationTime={info.creationTime}
                localActionsCount={info.localActionsCount}
                remoteActionsCount={info.remoteActionsCount}
                skippedActionsCount={info.skippedActionsCount}
                cacheHitCount={info.cacheHitCount}
                cacheHitRate={info.cacheHitRate}
                firstBuildSinceRebase={info.firstBuildSinceRebase}
                errorMessages={info.errorMessages}
              />
              <MetadataSection
                username={info.username}
                hostname={info.hostname}
                client={info.client}
                buck2Revision={info.buck2Revision}
                remoteExecutionId={info.remoteExecutionId}>
                {metadataRows}
              </MetadataSection>
              {info.remoteExecution && (
                <REStatsSection
                  uploadSpeedMax={info.remoteExecution.uploadSpeedMax}
                  uploadSpeedAvg={info.remoteExecution.uploadSpeedAvg}
                  downloadSpeedMax={info.remoteExecution.downloadSpeedMax}
                  downloadSpeedAvg={info.remoteExecution.downloadSpeedAvg}
                  bytesUploaded={info.remoteExecution.bytesUploaded}
                  bytesDownloaded={info.remoteExecution.bytesDownloaded}
                />
              )}
            </div>

            {/* Debugging & Insights */}
            <div className="flex min-w-0 flex-1 flex-col gap-4">
              {info.command === 'test' && <TestSummaryCard />}
              <FailedActionsCard />
              <CriticalPathChart />
              <FileChanges
                uuid={uuid}
                buildStartTimeMs={startTimeMs}
                changesSinceLastBuild={metrics?.fileChangesSinceLastBuild ?? []}
                changesSinceLastBuildCount={
                  metrics?.fileChangesSinceLastBuildCount ?? null
                }
              />
              <SpeedDoctor metrics={metrics} />
            </div>

            {/* Right-side debug info panel — collapsible, persists state in localStorage */}
            <DebugInfoPanel>
              <DebugInfoSection title="Logs">
                <LogsSection
                  hasEventLog={info.hasEventLog}
                  eventLogPath={info.eventLogRef}
                  reLogPath={info.reLogRef}
                  remoteExecutionId={info.remoteExecutionId}
                />
              </DebugInfoSection>
              {debugSections.map(s => (
                <DebugInfoSection key={s.title} title={s.title}>
                  {s.content}
                </DebugInfoSection>
              ))}
              <DebugInfoSection title="Log Commands">
                <LogCommands uuid={uuid} />
              </DebugInfoSection>
            </DebugInfoPanel>
          </div>
        </>
      ),
    },
    ...(info.command === 'test'
      ? [
          {
            id: 'tests',
            label: 'Tests',
            scroll: true,
            content: <TestResultsTab testInfraResults={testInfraResults} />,
          },
        ]
      : []),
    {
      id: 'actions',
      label: 'Actions',
      scroll: false,
      content: <ActionResultsTab />,
    },
    {
      id: 'output',
      label: 'Output',
      scroll: true,
      content: <OutputTab buildUuid={uuid} />,
    },
    {
      id: 'performance',
      label: 'Performance',
      scroll: true,
      content: <PerformanceTab buildUuid={uuid} />,
    },
    {id: 'logs', label: 'Logs', scroll: false, content: <EventLogViewer />},
  ];
  const tabs = orderTabs(builtinTabs, extraTabs);

  return (
    <EventLogProvider eventLogPath={info.eventLogRef}>
      <InvocationBar
        id={uuid}
        commandOutcome={info.commandOutcome}
        cliArgs={info.cliArgs}
      />

      {banner && <div className="mx-6 mt-3 shrink-0 space-y-3">{banner}</div>}

      <Tabs
        value={tab}
        onValueChange={setTab}
        className="mt-3 flex min-h-0 flex-1 flex-col">
        <div className="mx-6 flex shrink-0 items-center gap-2">
          <TabsList>
            {tabs.map(t => (
              <TabsTrigger
                key={t.id}
                value={t.id}
                className={tabTriggerHoverClass}>
                {t.label}
              </TabsTrigger>
            ))}
          </TabsList>
          <button
            type="button"
            aria-label={
              showDecodeDebug
                ? 'Hide decode debug panels'
                : 'Show decode debug panels'
            }
            aria-pressed={showDecodeDebug}
            title="Decode debug (timing & worker timeline)"
            onClick={() => setShowDecodeDebug(v => !v)}
            className={`ml-auto inline-flex size-7 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-gray-100 hover:text-foreground dark:hover:bg-gray-800 ${
              showDecodeDebug
                ? 'bg-gray-100 text-foreground dark:bg-gray-800'
                : ''
            }`}>
            <BugIcon className="size-4" />
          </button>
        </div>
        {showDecodeDebug && (
          <div className="mx-6 mt-3 shrink-0 space-y-3">
            <DecodeStatsPanel />
            <DecodeTimeline />
          </div>
        )}

        {tabs.map(t =>
          t.scroll ? (
            <TabsContent
              key={t.id}
              value={t.id}
              className="mt-4 min-h-0 flex-1 overflow-y-auto">
              <div className="px-6 pb-6">{t.content}</div>
            </TabsContent>
          ) : (
            <TabsContent
              key={t.id}
              value={t.id}
              className="mt-4 min-h-0 flex-1">
              <div className="flex h-full min-h-0 flex-col px-6 pb-6">
                {t.content}
              </div>
            </TabsContent>
          ),
        )}
      </Tabs>
    </EventLogProvider>
  );
}

/**
 * Top-of-overview error banner. When the event log has any failed actions,
 * the host's top-level error messages are almost always restating those
 * failures, so we suppress the noisy duplicate dump and just point at the
 * Failed Actions card below. Falls back to listing the raw error messages
 * (each collapsed to a single line via CollapsibleMessage) when the event
 * log isn't loaded or has no failed actions — that catches load errors,
 * configuration errors, and the like that the action card wouldn't show.
 */
function TopLevelErrors({errorMessages}: {errorMessages: readonly string[]}) {
  const logState = useEventLog();
  const failedActionCount = useMemo(() => {
    if (logState.status !== 'loaded') return 0;
    const summaries = logState.summaries;
    let count = 0;
    for (let i = 0; i < summaries.length; i++) {
      if (summaries.getEventType(i) !== 'actionExecution') continue;
      if (summaries.getType(i) !== 'spanEnd') continue;
      if (summaries.getFailed(i) === true) count++;
    }
    return count;
  }, [logState]);

  if (errorMessages.length === 0 && failedActionCount === 0) return null;

  if (failedActionCount > 0) {
    return (
      <div className="mb-4 rounded border border-red-200 bg-red-50 p-3 text-sm text-red-800 dark:border-red-800 dark:bg-red-950 dark:text-red-200">
        Build failed —{' '}
        <span className="font-medium">
          {failedActionCount} action{failedActionCount === 1 ? '' : 's'} failed
        </span>
        . See the Failed Actions card below for details.
      </div>
    );
  }

  return (
    <div className="mb-4 rounded border border-red-200 bg-red-50 p-3 dark:border-red-800 dark:bg-red-950">
      <p className="mb-1 text-sm font-medium text-red-800 dark:text-red-200">
        Errors
      </p>
      <div className="space-y-1">
        {errorMessages.map((msg, i) => (
          <CollapsibleMessage
            key={i}
            text={msg}
            className="text-sm text-red-700 dark:text-red-300"
          />
        ))}
      </div>
    </div>
  );
}
