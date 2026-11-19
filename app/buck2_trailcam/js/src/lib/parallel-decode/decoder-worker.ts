/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

/**
 * Decoder worker.
 *
 * Receives a batch of pre-sliced protobuf messages from the dispatcher,
 * decodes them, builds an EventSummary[] -> BatchData, and runs its own
 * collectors over its slice of events. Returns the BatchData (transferred)
 * + partial collector state for merging on the dispatcher / main.
 *
 * Each decoder has its own StringPool and its own collector instances —
 * there's no shared state between decoders. Cross-helper coordination
 * happens at the merge step.
 */

// @ts-expect-error -- generated JS module
import {buck} from '../proto/bundle.js';
import {type EventSummary, StringPool} from '../event-log-decoder';
import {
  LoadPackageCollector,
  AnalysisSpanCollector,
  ActionSpanCollector,
  CriticalPathCollector,
  type StreamingCollector,
} from '../streaming-collectors';
import {summariesToBatch} from '../event-summary-store';
import {SpanRecorder, nowMs} from '../phase-timer';
import type {DecoderRequest, DecoderResponse, PartialAggregates} from './types';

const post = (msg: DecoderResponse, transfer?: Transferable[]) => {
  if (transfer && transfer.length)
    (self as unknown as Worker).postMessage(msg, transfer);
  else (self as unknown as Worker).postMessage(msg);
};

// Span recorder; spans are drained into each `result` so they reach the
// main timeline in pieces. Worker-local epoch is established by the
// `ping` handshake before any other message arrives.
const spans = new SpanRecorder();
let skipActionSpans = false;

const {CommandProgress} = buck.daemon;
const {Invocation} = buck.data;

const toObjectOpts = {longs: String, enums: String, defaults: false};

const SPAN_END_SKIP_KEYS = new Set(['stats', 'duration']);

function identifyEventType(data: Record<string, unknown>): string | undefined {
  for (const spanKey of ['spanStart', 'spanEnd', 'instant'] as const) {
    const span = data[spanKey] as Record<string, unknown> | undefined;
    if (!span) continue;
    const skip = spanKey === 'spanEnd' ? SPAN_END_SKIP_KEYS : undefined;
    for (const key of Object.keys(span)) {
      if (skip?.has(key)) continue;
      if (span[key] != null) return key;
    }
  }
  return undefined;
}

function identifyEventKind(data: Record<string, unknown>): string {
  if (data.spanStart) return 'spanStart';
  if (data.spanEnd) return 'spanEnd';
  if (data.instant) return 'instant';
  if (data.record) return 'record';
  return 'event';
}

function extractTimestampMs(ts: unknown): number | undefined {
  if (!ts || typeof ts !== 'object') return undefined;
  const obj = ts as Record<string, unknown>;
  return (
    Number(obj.seconds ?? 0) * 1000 +
    (typeof obj.nanos === 'number' ? obj.nanos / 1e6 : 0)
  );
}

function extractDurationMs(data: Record<string, unknown>): number | undefined {
  const spanEnd = data.spanEnd as Record<string, unknown> | undefined;
  if (!spanEnd) return undefined;
  const dur = spanEnd.duration as Record<string, unknown> | undefined;
  if (!dur) return undefined;
  return (
    Number(dur.seconds ?? 0) * 1000 +
    (typeof dur.nanos === 'number' ? dur.nanos / 1e6 : 0)
  );
}

function extractActionName(data: Record<string, unknown>): string | undefined {
  for (const key of ['spanStart', 'spanEnd'] as const) {
    const span = data[key] as Record<string, unknown> | undefined;
    if (!span) continue;
    const ae = span.actionExecution as Record<string, unknown> | undefined;
    if (!ae) continue;
    const name = ae.name as Record<string, unknown> | undefined;
    if (!name) continue;
    const category = name.category as string | undefined;
    const identifier = name.identifier as string | undefined;
    if (category || identifier)
      return [category, identifier].filter(Boolean).join(' ');
  }
  return undefined;
}

function extractExecutionKind(
  data: Record<string, unknown>,
): string | undefined {
  const spanEnd = data.spanEnd as Record<string, unknown> | undefined;
  if (!spanEnd) return undefined;
  const ae = spanEnd.actionExecution as Record<string, unknown> | undefined;
  return (ae?.executionKind as string) ?? undefined;
}

function extractActionFailed(
  data: Record<string, unknown>,
): boolean | undefined {
  const spanEnd = data.spanEnd as Record<string, unknown> | undefined;
  if (!spanEnd) return undefined;
  const ae = spanEnd.actionExecution as Record<string, unknown> | undefined;
  if (!ae) return undefined;
  return !!(ae.failed as boolean);
}

function labelFromValue(val: unknown): string | undefined {
  if (typeof val === 'string') return val;
  if (val == null || typeof val !== 'object') return undefined;
  const obj = val as Record<string, unknown>;

  // Direct {package, name} shape (e.g. TargetLabel proto).
  if (typeof obj.package === 'string' || typeof obj.name === 'string') {
    const pkg = (obj.package as string) ?? '';
    const name = (obj.name as string) ?? '';
    if (pkg || name) return name ? `${pkg}:${name}` : pkg;
  }

  // Wrapped: {label: ...}. Used by ConfiguredTargetLabel and similar
  // protos — the inner `label` may itself be a string OR another nested
  // object with the structured {package, name} shape.
  if ('label' in obj) {
    const inner = obj.label;
    if (typeof inner === 'string') return inner;
    if (inner != null && typeof inner === 'object') {
      const lbl = inner as Record<string, unknown>;
      const pkg = (lbl.package as string) ?? '';
      const name = (lbl.name as string) ?? '';
      if (pkg || name) return name ? `${pkg}:${name}` : pkg;
    }
  }

  return undefined;
}

function extractTargetLabel(data: Record<string, unknown>): string | undefined {
  for (const key of ['spanStart', 'spanEnd'] as const) {
    const span = data[key] as Record<string, unknown> | undefined;
    if (!span) continue;
    const analysis = span.analysis as Record<string, unknown> | undefined;
    if (analysis?.target) {
      const label = labelFromValue(analysis.target);
      if (label) return label;
    }
    if (analysis?.standardTarget) {
      const label = labelFromValue(analysis.standardTarget);
      if (label) return label;
    }
    const ae = span.actionExecution as Record<string, unknown> | undefined;
    if (ae) {
      const k = ae.key as Record<string, unknown> | undefined;
      if (k?.targetLabel) {
        const label = labelFromValue(k.targetLabel);
        if (label) return label;
      }
    }
  }
  const instant = data.instant as Record<string, unknown> | undefined;
  if (instant?.testResult) {
    const tr = instant.testResult as Record<string, unknown>;
    if (tr.targetLabel) return labelFromValue(tr.targetLabel);
  }
  return undefined;
}

function parseSpanId(s: string | undefined): number | undefined {
  if (!s || s === '0') return undefined;
  return Number(s);
}

self.onmessage = (e: MessageEvent<DecoderRequest>) => {
  const msg = e.data;
  if (msg.type === 'ping') {
    // Clock-sync handshake — see orchestrator. Always the first message.
    const tEpoch = nowMs();
    spans.spans.push({phase: 'handshake', startMs: tEpoch, endMs: nowMs()});
    post({type: 'pong', epochMs: tEpoch, spans: spans.drain()});
    return;
  }
  if (msg.type === 'init') {
    skipActionSpans = msg.skipActionSpans;
    return;
  }
  if (msg.type !== 'decode') return;

  const batchSpan = spans.begin('decode_batch');
  batchSpan.set('batchId', msg.batchId);
  batchSpan.set('chunkCount', msg.chunks.length);
  let totalMessages = 0;
  let totalChunkBytes = 0;
  for (const c of msg.chunks) {
    totalMessages += c.offsets.length + (c.leading ? 1 : 0);
    totalChunkBytes += c.buffer.byteLength + (c.leading?.length ?? 0);
  }
  batchSpan.set('msgCount', totalMessages);
  batchSpan.set('totalBytes', totalChunkBytes);
  try {
    const summaries: EventSummary[] = [];
    const pool = new StringPool();
    const loadCollector = new LoadPackageCollector();
    const analysisCollector = new AnalysisSpanCollector();
    // ActionSpanCollector is the most expensive collector — its results
    // are 1-per-action and balloon to millions of entries on huge logs.
    // Skip it entirely for large logs (matching the single-worker
    // pipeline's behavior); ActionTreemap already gates on isLargeLog.
    const actionCollector = skipActionSpans ? null : new ActionSpanCollector();
    const critPathCollector = new CriticalPathCollector();
    const collectors: StreamingCollector[] = [
      loadCollector,
      analysisCollector,
      ...(actionCollector ? [actionCollector] : []),
      critPathCollector,
    ];

    let critPathEventIndex = -1;
    let eventIndex = msg.startEventIndex;
    let isFirstMessage = msg.hasInvocationFirst;

    function processMessage(
      msgBytes: Uint8Array,
      idbChunkIndex: number,
      offsetInChunk: number,
      length: number,
    ): void {
      let summary: EventSummary | null = null;
      try {
        if (isFirstMessage) {
          const inv = Invocation.decode(msgBytes);
          const obj = Invocation.toObject(inv, toObjectOpts);
          summary = {
            index: eventIndex,
            type: pool.intern('invocation')!,
            timestampMs: extractTimestampMs(obj.startTime ?? undefined),
            batchIndex: idbChunkIndex,
            offsetInBatch: offsetInChunk,
            lengthInBatch: length,
          };
          for (const c of collectors) c.processEvent(summary, obj);
        } else {
          const progress = CommandProgress.decode(msgBytes);
          const progressObj = CommandProgress.toObject(progress, toObjectOpts);

          if (progressObj.event) {
            const evt = progressObj.event;
            const kind = identifyEventKind(evt);
            const eventType = identifyEventType(evt);
            summary = {
              index: eventIndex,
              type: pool.intern(kind)!,
              eventType: pool.intern(eventType),
              timestampMs: extractTimestampMs(evt.timestamp ?? undefined),
              spanId: parseSpanId(evt.spanId),
              parentId: parseSpanId(evt.parentId),
              durationMs: extractDurationMs(evt),
              actionName: pool.intern(extractActionName(evt)),
              executionKind: pool.intern(extractExecutionKind(evt)),
              targetLabel: pool.intern(extractTargetLabel(evt)),
              failed: extractActionFailed(evt),
              batchIndex: idbChunkIndex,
              offsetInBatch: offsetInChunk,
              lengthInBatch: length,
            };
            const before = critPathCollector.result;
            for (const c of collectors) c.processEvent(summary, evt);
            if (!before && critPathCollector.result) {
              critPathEventIndex = eventIndex;
            }
          } else if (progressObj.result) {
            summary = {
              index: eventIndex,
              type: pool.intern('result')!,
              batchIndex: idbChunkIndex,
              offsetInBatch: offsetInChunk,
              lengthInBatch: length,
            };
          } else if (progressObj.partialResult) {
            summary = {
              index: eventIndex,
              type: pool.intern('partial_result')!,
              batchIndex: idbChunkIndex,
              offsetInBatch: offsetInChunk,
              lengthInBatch: length,
            };
          }
        }
      } catch {
        summary = {
          index: eventIndex,
          type: pool.intern('unknown')!,
          batchIndex: idbChunkIndex,
          offsetInBatch: offsetInChunk,
          lengthInBatch: length,
        };
      }

      if (summary) summaries.push(summary);
      isFirstMessage = false;
      eventIndex++;
    }

    for (const chunk of msg.chunks) {
      // Leading straddler (own IDB row, offset 0) comes first in event order.
      if (chunk.leading) {
        processMessage(
          chunk.leading,
          chunk.leadingIdbChunkIndex!,
          0,
          chunk.leading.length,
        );
      }
      const view = new Uint8Array(chunk.buffer);
      for (let i = 0; i < chunk.offsets.length; i++) {
        const off = chunk.offsets[i];
        const len = chunk.lengths[i];
        processMessage(
          view.subarray(off, off + len),
          chunk.idbChunkIndex,
          off,
          len,
        );
      }
    }

    const batchData = summariesToBatch(summaries);

    const partial: PartialAggregates = {
      loadSpans: loadCollector.results,
      // Pull the private buildFileMetrics out for the merged finalize step.
      // (This is per-helper; the dispatcher concatenates them.)
      loadBuildFileMetrics: Array.from(
        // Access the private map by index — needed for cross-helper merge.
        (
          loadCollector as unknown as {
            buildFileMetrics: Map<
              string,
              {
                starlarkPeakAllocatedBytes?: number;
                cpuInstructionCount?: number;
                targetCount?: number;
              }
            >;
          }
        ).buildFileMetrics,
      ),
      actionSpans: actionCollector ? actionCollector.results : [],
      analysisSpans: analysisCollector.results,
      criticalPath: critPathCollector.result,
      criticalPathEventIndex: critPathEventIndex,
    };

    batchSpan.end();
    // BatchData uses typed arrays internally; structured clone of typed
    // arrays is cheap so we don't attempt to transfer their buffers
    // explicitly here. The PartialAggregates contains plain objects.
    post({
      type: 'result',
      batchId: msg.batchId,
      batchData,
      partial,
      spans: spans.drain(),
    });
  } catch (err) {
    batchSpan.end();
    post({
      type: 'error',
      message: err instanceof Error ? err.message : 'decode failed',
    });
  }
};
