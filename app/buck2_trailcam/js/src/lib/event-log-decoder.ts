/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

// @ts-expect-error -- generated JS module, no proper TS exports
import {buck} from './proto/bundle.js';

const {CommandProgress} = buck.daemon;
const {Invocation} = buck.data;

export interface DecodedEvent {
  /** Index in the event log stream */
  index: number;
  /** 'event' | 'result' | 'partial_result' | 'invocation' */
  type: string;
  /** The decoded protobuf message as a plain object */
  data: Record<string, unknown>;
  /** Timestamp if available */
  timestamp?: {seconds: number; nanos: number};
  /** Timestamp in ms since epoch (pre-computed for sorting/display) */
  timestampMs?: number;
  /** Span ID if available */
  spanId?: string;
  /** Parent span ID if available */
  parentId?: string;
  /** The specific event type (e.g. 'command', 'action_execution') */
  eventType?: string;

  // --- Pre-extracted fields (computed once at decode time) ---
  /** Duration in ms from spanEnd.duration */
  durationMs?: number;
  /** Action name (category + identifier) */
  actionName?: string;
  /** Execution kind: LOCAL, REMOTE, ACTION_CACHE, etc. */
  executionKind?: string;
  /** Target label extracted from various event types */
  targetLabel?: string;
  /** Pre-built search string for text filtering */
  searchText?: string;
}

/**
 * Lightweight event summary — kept in memory for list rendering and filtering.
 * Full event data is decoded on demand from the backing ArrayBuffer.
 *
 * Optimized for 1M+ events:
 * - spanId/parentId are numbers (not strings) to avoid per-event string allocs
 * - All string fields are interned via a shared StringPool
 * - Only pre-extracted fields needed for rendering/filtering
 */
export interface EventSummary {
  index: number;
  type: string;
  eventType?: string;
  timestampMs?: number;
  /** Span ID as a number (proto uint64, fits in JS number for practical values) */
  spanId?: number;
  /** Parent span ID as a number */
  parentId?: number;
  durationMs?: number;
  actionName?: string;
  executionKind?: string;
  targetLabel?: string;
  /**
   * For action-execution spanEnd events: whether the action failed.
   * Undefined for any other event type. Surfaced as a fast columnar field
   * so the Actions tab can filter by status without decoding every proto.
   */
  failed?: boolean;
  /** Which batch ArrayBuffer this event's JSON lives in */
  batchIndex: number;
  /** Byte offset of this event's JSON within the batch */
  offsetInBatch: number;
  /** Byte length of this event's JSON within the batch */
  lengthInBatch: number;
}

/**
 * String interning pool — deduplicates strings so identical values
 * share the same JS string reference. Critical for 1M+ events where
 * eventType/executionKind/type have very few unique values.
 */
export class StringPool {
  private pool = new Map<string, string>();

  intern(s: string | undefined): string | undefined {
    if (s === undefined) return undefined;
    const existing = this.pool.get(s);
    if (existing !== undefined) return existing;
    this.pool.set(s, s);
    return s;
  }
}

/** Result of one batch from the streaming decoder with summaries */
export interface DecodedBatchWithSummaries {
  summaries: EventSummary[];
  /** ArrayBuffer containing raw protobuf bytes for each event (concatenated) */
  protoBuffer: ArrayBuffer;
}

/**
 * Decode a single event's full data from raw protobuf bytes.
 * The bytes should be the raw CommandProgress message (or Invocation for index 0).
 */
export function decodeEventFromProto(
  bytes: Uint8Array,
  isInvocation: boolean,
): Record<string, unknown> {
  const toObjectOpts = {longs: String, enums: String, defaults: false};
  if (isInvocation) {
    const invocation = Invocation.decode(bytes);
    return Invocation.toObject(invocation, toObjectOpts) as Record<
      string,
      unknown
    >;
  }
  const progress = CommandProgress.decode(bytes);
  const obj = CommandProgress.toObject(progress, toObjectOpts);
  if (obj.event) return obj.event;
  if (obj.result) return obj.result;
  if (obj.partialResult) return obj.partialResult;
  return obj;
}

/**
 * Read a varint from a buffer at the given offset.
 * Returns [value, bytesRead].
 */
function readVarint(buf: Uint8Array, offset: number): [number, number] {
  let result = 0;
  let shift = 0;
  let pos = offset;
  while (pos < buf.length) {
    const byte = buf[pos];
    result |= (byte & 0x7f) << shift;
    pos++;
    if ((byte & 0x80) === 0) {
      return [result, pos - offset];
    }
    shift += 7;
    if (shift > 35) {
      throw new Error('Varint too long');
    }
  }
  throw new Error('Unexpected end of buffer while reading varint');
}

/**
 * Read length-delimited protobuf messages from a buffer.
 * Each message is prefixed by a varint length.
 */
function* readLengthDelimited(buf: Uint8Array): Generator<Uint8Array> {
  let offset = 0;
  while (offset < buf.length) {
    const [len, varintBytes] = readVarint(buf, offset);
    offset += varintBytes;
    if (offset + len > buf.length) {
      throw new Error(
        `Message at offset ${offset - varintBytes} claims ${len} bytes but only ${buf.length - offset} remain`,
      );
    }
    yield buf.subarray(offset, offset + len);
    offset += len;
  }
}

// --- Field extraction helpers (run once at decode time) ---

function extractTimestampMs(
  ts: {seconds?: number | string; nanos?: number} | undefined,
): number | undefined {
  if (!ts) return undefined;
  return Number(ts.seconds ?? 0) * 1000 + (ts.nanos ?? 0) / 1e6;
}

function extractDurationMs(data: Record<string, unknown>): number | undefined {
  const spanEnd = data.spanEnd as Record<string, unknown> | undefined;
  if (!spanEnd) return undefined;
  const dur = spanEnd.duration as
    {seconds?: string | number; nanos?: number} | undefined;
  if (!dur) return undefined;
  return Number(dur.seconds ?? 0) * 1000 + (dur.nanos ?? 0) / 1e6;
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
    if (category || identifier) {
      return [category, identifier].filter(Boolean).join(' ');
    }
  }
  return undefined;
}

function extractExecutionKind(
  data: Record<string, unknown>,
): string | undefined {
  const spanEnd = data.spanEnd as Record<string, unknown> | undefined;
  if (!spanEnd) return undefined;
  const ae = spanEnd.actionExecution as Record<string, unknown> | undefined;
  if (!ae) return undefined;
  return (ae.executionKind as string) ?? undefined;
}

function labelFromValue(val: unknown): string | undefined {
  if (typeof val === 'string') return val;
  if (val != null && typeof val === 'object' && 'label' in val) {
    const label = (val as Record<string, unknown>).label;
    if (typeof label === 'string') return label;
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

    const ae = span.actionExecution as Record<string, unknown> | undefined;
    if (ae) {
      const owner = ae.owner as Record<string, unknown> | undefined;
      if (owner?.configuredTargetLabel) {
        const label = labelFromValue(owner.configuredTargetLabel);
        if (label) return label;
      }
      const k = ae.key as Record<string, unknown> | undefined;
      if (k?.owner) {
        const owner2 = k.owner as Record<string, unknown>;
        const label = labelFromValue(owner2.configuredTargetLabel);
        if (label) return label;
      }
    }
  }

  const instant = data.instant as Record<string, unknown> | undefined;
  if (instant) {
    const testResult = instant.testResult as
      Record<string, unknown> | undefined;
    if (testResult?.targetLabel) {
      return labelFromValue(testResult.targetLabel);
    }
  }

  return undefined;
}

/** Build a lightweight search string from key fields (avoids JSON.stringify on every filter). */
function buildSearchText(
  type: string,
  eventType: string | undefined,
  actionName: string | undefined,
  executionKind: string | undefined,
  targetLabel: string | undefined,
): string {
  return [type, eventType, actionName, executionKind, targetLabel]
    .filter(Boolean)
    .join(' ')
    .toLowerCase();
}

// Top-level fields on SpanEndEvent that aren't the oneof payload
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

/** Determine the event kind: spanStart, spanEnd, instant, or event (fallback). */
function identifyEventKind(data: Record<string, unknown>): string {
  if (data.spanStart) return 'spanStart';
  if (data.spanEnd) return 'spanEnd';
  if (data.instant) return 'instant';
  if (data.record) return 'record';
  return 'event';
}

/**
 * Decode a decompressed event log buffer into structured events.
 *
 * File format: length-delimited messages where:
 *   - First message is buck.data.Invocation
 *   - Remaining messages are buck.daemon.CommandProgress
 */
export function decodeEventLog(decompressed: Uint8Array): DecodedEvent[] {
  const events: DecodedEvent[] = [];
  let index = 0;
  let isFirst = true;
  const toObjectOpts = {longs: String, enums: String, defaults: false};

  for (const msgBytes of readLengthDelimited(decompressed)) {
    try {
      if (isFirst) {
        isFirst = false;
        const invocation = Invocation.decode(msgBytes);
        const obj = Invocation.toObject(invocation, toObjectOpts);
        const ts = obj.startTime ?? undefined;
        events.push({
          index,
          type: 'invocation',
          data: obj,
          timestamp: ts,
          timestampMs: extractTimestampMs(ts),
          searchText: buildSearchText(
            'invocation',
            undefined,
            undefined,
            undefined,
            undefined,
          ),
        });
      } else {
        const progress = CommandProgress.decode(msgBytes);
        const progressObj = CommandProgress.toObject(progress, toObjectOpts);

        if (progressObj.event) {
          const evt = progressObj.event;
          const kind = identifyEventKind(evt);
          const eventType = identifyEventType(evt);
          const actionName = extractActionName(evt);
          const executionKind = extractExecutionKind(evt);
          const targetLabel = extractTargetLabel(evt);
          const durationMs = extractDurationMs(evt);
          const ts = evt.timestamp ?? undefined;
          events.push({
            index,
            type: kind,
            data: evt,
            timestamp: ts,
            timestampMs: extractTimestampMs(ts),
            spanId: evt.spanId ?? undefined,
            parentId: evt.parentId ?? undefined,
            eventType,
            durationMs,
            actionName,
            executionKind,
            targetLabel,
            searchText: buildSearchText(
              kind,
              eventType,
              actionName,
              executionKind,
              targetLabel,
            ),
          });
        } else if (progressObj.result) {
          events.push({
            index,
            type: 'result',
            data: progressObj.result,
            searchText: buildSearchText(
              'result',
              undefined,
              undefined,
              undefined,
              undefined,
            ),
          });
        } else if (progressObj.partialResult) {
          events.push({
            index,
            type: 'partial_result',
            data: progressObj.partialResult,
            searchText: buildSearchText(
              'partial_result',
              undefined,
              undefined,
              undefined,
              undefined,
            ),
          });
        }
      }
    } catch {
      // Tolerate decode failures (e.g. truncated in-progress logs)
      events.push({
        index,
        type: 'unknown',
        data: {raw: `(${msgBytes.length} bytes, decode failed)`},
      });
    }
    index++;
  }

  return events;
}

/**
 * Streaming variant: yields batches of decoded events.
 * Allows the caller (e.g. a web worker) to send results incrementally.
 */
export function* decodeEventLogBatched(
  decompressed: Uint8Array,
  batchSize = 500,
): Generator<DecodedEvent[]> {
  let batch: DecodedEvent[] = [];
  let index = 0;
  let isFirst = true;
  const toObjectOpts = {longs: String, enums: String, defaults: false};

  for (const msgBytes of readLengthDelimited(decompressed)) {
    let event: DecodedEvent | null = null;
    try {
      if (isFirst) {
        isFirst = false;
        const invocation = Invocation.decode(msgBytes);
        const obj = Invocation.toObject(invocation, toObjectOpts);
        const ts = obj.startTime ?? undefined;
        event = {
          index,
          type: 'invocation',
          data: obj,
          timestamp: ts,
          timestampMs: extractTimestampMs(ts),
          searchText: buildSearchText(
            'invocation',
            undefined,
            undefined,
            undefined,
            undefined,
          ),
        };
      } else {
        const progress = CommandProgress.decode(msgBytes);
        const progressObj = CommandProgress.toObject(progress, toObjectOpts);

        if (progressObj.event) {
          const evt = progressObj.event;
          const kind = identifyEventKind(evt);
          const eventType = identifyEventType(evt);
          const actionName = extractActionName(evt);
          const executionKind = extractExecutionKind(evt);
          const targetLabel = extractTargetLabel(evt);
          const durationMs = extractDurationMs(evt);
          const ts = evt.timestamp ?? undefined;
          event = {
            index,
            type: kind,
            data: evt,
            timestamp: ts,
            timestampMs: extractTimestampMs(ts),
            spanId: evt.spanId ?? undefined,
            parentId: evt.parentId ?? undefined,
            eventType,
            durationMs,
            actionName,
            executionKind,
            targetLabel,
            searchText: buildSearchText(
              kind,
              eventType,
              actionName,
              executionKind,
              targetLabel,
            ),
          };
        } else if (progressObj.result) {
          event = {
            index,
            type: 'result',
            data: progressObj.result,
            searchText: buildSearchText(
              'result',
              undefined,
              undefined,
              undefined,
              undefined,
            ),
          };
        } else if (progressObj.partialResult) {
          event = {
            index,
            type: 'partial_result',
            data: progressObj.partialResult,
            searchText: buildSearchText(
              'partial_result',
              undefined,
              undefined,
              undefined,
              undefined,
            ),
          };
        }
      }
    } catch {
      event = {
        index,
        type: 'unknown',
        data: {raw: `(${msgBytes.length} bytes, decode failed)`},
      };
    }

    if (event) {
      batch.push(event);
      if (batch.length >= batchSize) {
        yield batch;
        batch = [];
      }
    }
    index++;
  }

  if (batch.length > 0) {
    yield batch;
  }
}

/** Parse a span ID string to a number. Returns undefined if missing/zero. */
function parseSpanId(s: string | undefined): number | undefined {
  if (!s || s === '0') return undefined;
  return Number(s);
}

/**
 * Streaming decoder that yields batches of EventSummary + raw protobuf bytes.
 *
 * The proto buffer contains each event's raw protobuf-encoded bytes concatenated.
 * Summaries contain offsets into this buffer for on-demand decoding.
 * This avoids the cost of JSON.stringify for every event.
 */
export function* decodeEventLogWithSummaries(
  decompressed: Uint8Array,
  batchSize = 10000,
): Generator<DecodedBatchWithSummaries> {
  const pool = new StringPool();
  let batchIndex = 0;
  let currentSummaries: EventSummary[] = [];
  let currentProtoParts: Uint8Array[] = [];
  let currentOffset = 0;
  let eventCount = 0;
  let index = 0;
  let isFirst = true;
  const toObjectOpts = {longs: String, enums: String, defaults: false};

  function flushBatch(): DecodedBatchWithSummaries {
    const totalLen = currentProtoParts.reduce((sum, p) => sum + p.length, 0);
    const protoBuffer = new ArrayBuffer(totalLen);
    const view = new Uint8Array(protoBuffer);
    let offset = 0;
    for (const part of currentProtoParts) {
      view.set(part, offset);
      offset += part.length;
    }
    const result: DecodedBatchWithSummaries = {
      summaries: currentSummaries,
      protoBuffer,
    };
    currentSummaries = [];
    currentProtoParts = [];
    currentOffset = 0;
    batchIndex++;
    return result;
  }

  for (const msgBytes of readLengthDelimited(decompressed)) {
    // We need a partial decode to extract summary fields, but keep the raw bytes
    let summary: EventSummary | null = null;
    try {
      if (isFirst) {
        isFirst = false;
        const invocation = Invocation.decode(msgBytes);
        const obj = Invocation.toObject(invocation, toObjectOpts);
        const ts = obj.startTime ?? undefined;
        summary = {
          index,
          type: pool.intern('invocation')!,
          timestampMs: extractTimestampMs(ts),
          batchIndex,
          offsetInBatch: currentOffset,
          lengthInBatch: msgBytes.length,
        };
      } else {
        const progress = CommandProgress.decode(msgBytes);
        const progressObj = CommandProgress.toObject(progress, toObjectOpts);

        if (progressObj.event) {
          const evt = progressObj.event;
          const kind = identifyEventKind(evt);
          const eventType = identifyEventType(evt);
          summary = {
            index,
            type: pool.intern(kind)!,
            eventType: pool.intern(eventType),
            timestampMs: extractTimestampMs(evt.timestamp ?? undefined),
            spanId: parseSpanId(evt.spanId),
            parentId: parseSpanId(evt.parentId),
            durationMs: extractDurationMs(evt),
            actionName: pool.intern(extractActionName(evt)),
            executionKind: pool.intern(extractExecutionKind(evt)),
            targetLabel: pool.intern(extractTargetLabel(evt)),
            batchIndex,
            offsetInBatch: currentOffset,
            lengthInBatch: msgBytes.length,
          };
        } else if (progressObj.result) {
          summary = {
            index,
            type: pool.intern('result')!,
            batchIndex,
            offsetInBatch: currentOffset,
            lengthInBatch: msgBytes.length,
          };
        } else if (progressObj.partialResult) {
          summary = {
            index,
            type: pool.intern('partial_result')!,
            batchIndex,
            offsetInBatch: currentOffset,
            lengthInBatch: msgBytes.length,
          };
        }
      }
    } catch {
      summary = {
        index,
        type: pool.intern('unknown')!,
        batchIndex,
        offsetInBatch: currentOffset,
        lengthInBatch: msgBytes.length,
      };
    }

    if (summary) {
      currentSummaries.push(summary);
      // Store the raw proto bytes (a copy since msgBytes is a subarray)
      currentProtoParts.push(new Uint8Array(msgBytes));
      currentOffset += msgBytes.length;
      eventCount++;

      if (eventCount % batchSize === 0) {
        yield flushBatch();
      }
    }
    index++;
  }

  if (currentSummaries.length > 0) {
    yield flushBatch();
  }
}
