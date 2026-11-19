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
 * Message protocols for the parallel decode pipeline.
 *
 *   compressed bytes → [decompress worker] → chunks
 *                    → [dispatch worker]   → batches
 *                    → [decoder worker × 4] → batchResults
 *                    → [main orchestrator]  → summaryBatch / done
 *
 * Main routes messages between workers because Next.js's worker bundling
 * doesn't reliably support nested workers; cross-worker traffic is small
 * and uses transferable buffers, so the main hop is essentially free.
 */

import type {BatchData} from '../event-summary-store';
import type {AggregateData} from '../streaming-collectors';
import type {CriticalPathData} from '../critical-path';
import type {PhaseTimings, Span, TimelineLane} from '../phase-timer';

// ---------------------------------------------------------------------------
// Decompress worker
// ---------------------------------------------------------------------------

export type DecompressRequest =
  /** Clock-sync handshake. Always the first message sent to the worker;
   *  the worker stamps its epoch when it arrives and replies with `pong`. */
  | {type: 'ping'}
  | {
      type: 'decompress';
      /** Stream of zstd-compressed event log bytes (transferred ownership).
       *  Allows decompression to start as bytes arrive over the network
       *  rather than buffering the whole response first. */
      stream: ReadableStream<Uint8Array>;
    };

export type DecompressResponse =
  | {
      type: 'pong';
      /** Worker's `performance.now()` when it received the ping. */
      epochMs: number;
      /** A single `handshake` span covering ping-receipt → pong-send. Used
       *  as a visible sync point on the timeline. */
      spans: Span[];
    }
  | {
      type: 'chunk';
      /** Decompressed bytes (transferred ownership). */
      data: ArrayBuffer;
    }
  | {
      type: 'done';
      phaseTimings: PhaseTimings;
      decompressedBytes: number;
      /** Total compressed bytes pulled from the response stream. May
       *  differ from `compressedSize` (Content-Length) — particularly
       *  when the server uses chunked transfer encoding and omits
       *  Content-Length. */
      receivedBytes: number;
      spans: Span[];
    }
  | {type: 'error'; message: string};

// ---------------------------------------------------------------------------
// Dispatch worker
// ---------------------------------------------------------------------------

export interface DispatchPing {
  type: 'ping';
}

export interface DispatchInit {
  type: 'init';
  numDecoders: number;
  /** Caching key (manifold path); used for IDB writes. */
  eventLogPath: string;
  /** Compressed input size in bytes — used for "isVeryLarge" gating. */
  compressedSize: number;
}

export interface DispatchChunk {
  type: 'chunk';
  /** Decompressed bytes from the decompress worker (transferred). */
  data: ArrayBuffer;
}

export interface DispatchDecompressDone {
  type: 'decompressDone';
  /** Forwarded from the decompress worker so the dispatcher knows to
   *  finalize after the last chunk. */
  decompressedBytes: number;
  /** Total compressed bytes pulled from the response stream. */
  receivedBytes: number;
}

export interface DispatchDecoderResult {
  type: 'decoderResult';
  /** Which decoder posted this — 0..numDecoders-1. */
  from: number;
  batchId: number;
  /** Columnar EventSummary batch produced from `batchId`'s messages. */
  batchData: BatchData;
  /** Per-helper collector state (mergeable on main / dispatcher). */
  partial: PartialAggregates;
  /** Spans recorded since the previous decoderResult — drained each time
   *  to keep the per-message wire size small. */
  spans: Span[];
}

export type DispatchRequest =
  | DispatchPing
  | DispatchInit
  | DispatchChunk
  | DispatchDecompressDone
  | DispatchDecoderResult;

export type DispatchResponse =
  | {
      type: 'pong';
      /** Worker's `performance.now()` when it received the ping. */
      epochMs: number;
      /** A single `handshake` span. */
      spans: Span[];
    }
  /** Out: a batch to send to a specific decoder (transferable). */
  | {
      type: 'toDecoder';
      decoder: number;
      payload: DecodeBatchRequest;
      /** Buffers in `payload` to pass in the postMessage transfer list. */
      transfer: ArrayBuffer[];
    }
  /** Forwarded to main: an EventSummary batch ready for the store. */
  | {type: 'summaryBatch'; batch: BatchData}
  /** Periodic phase-timing snapshot. */
  | {
      type: 'progress';
      message: string;
      decompressedBytes: number;
      phaseTimings: PhaseTimings;
    }
  | {
      type: 'done';
      decompressedBytes: number;
      /** Actual compressed bytes received from the network. May be
       *  larger than the orchestrator's `compressedSize` (which comes
       *  from Content-Length and can be 0 / stale for chunked
       *  responses). Used to display compressed size accurately and
       *  to update the IDB rawSize column. */
      receivedBytes: number;
      totalEvents: number;
      chunkCount: number;
      /** Always present — small. */
      criticalPath: CriticalPathData | null;
      /** Inline copy of the heavy aggregates, *only* for small logs
       *  (compressed < INLINE_AGGREGATES_THRESHOLD). For large logs
       *  these are written to IDB by the dispatcher *before* `done`
       *  is posted; main reads them via `readLazyAggregate`. */
      inlineAggregates?: {
        loadSpans: AggregateData['loadSpans'];
        analysisSpans: AggregateData['analysisSpans'];
        actionSpans: AggregateData['actionSpans'];
      };
      phaseTimings: PhaseTimings;
      /** Dispatcher's own spans recorded up to the point of posting
       *  `done`. Tail spans (drain_pending_writes, finalize_invocation)
       *  are recorded *after* `done` and shipped separately via
       *  `cacheWritten`. */
      spans: Span[];
    }
  /** Sent after `done` once the background IDB writes complete. Carries
   *  the dispatcher spans recorded during that tail work (so the
   *  timeline can be amended). The orchestrator uses receipt of this
   *  message as the signal to terminate workers. */
  | {
      type: 'cacheWritten';
      tailSpans: Span[];
      tailPhaseTimings: PhaseTimings;
    }
  | {type: 'error'; message: string};

// ---------------------------------------------------------------------------
// Decoder worker
// ---------------------------------------------------------------------------

export interface DecoderPing {
  type: 'ping';
}

export interface DecoderInit {
  type: 'init';
  decoderIndex: number;
  /** Skip the (very expensive) ActionSpanCollector. The orchestrator
   *  flips this on for large logs (compressed > 100MB) to mirror the
   *  single-worker pipeline's behavior — at that scale the merged
   *  actionSpans is too large to structured-clone over postMessage. */
  skipActionSpans: boolean;
}

/** A single decompress chunk's worth of work for a decoder. */
export interface ChunkDescriptor {
  /** The decompress chunk's bytes — transferred to the decoder. */
  buffer: ArrayBuffer;
  /** Offsets within `buffer` for messages entirely contained in it. */
  offsets: Uint32Array;
  /** Lengths matching `offsets`. */
  lengths: Uint32Array;
  /** IDB chunkIndex assigned to this chunk by the dispatcher. The
   *  decoder stores it on each in-chunk EventSummary so on-demand
   *  reads can look the bytes up via `getCachedChunk`. */
  idbChunkIndex: number;
  /** Optional reassembled message that started in the *previous*
   *  decompress chunk and ended at the start of this one. Comes first
   *  in event order. */
  leading?: Uint8Array;
  /** IDB chunkIndex assigned to the leading message (its own row).
   *  Required iff `leading` is set. */
  leadingIdbChunkIndex?: number;
}

export interface DecodeBatchRequest {
  type: 'decode';
  batchId: number;
  /** Cumulative event index of the first message in this batch. */
  startEventIndex: number;
  /** True if the batch contains the very first (Invocation) message. */
  hasInvocationFirst: boolean;
  /** Decompress chunks routed to this decoder. The decoder iterates
   *  chunks in order (and within each: leading message first, then
   *  in-chunk messages in offset order). The `buffer` of each
   *  ChunkDescriptor is in the postMessage transfer list. */
  chunks: ChunkDescriptor[];
}

export type DecoderRequest = DecoderPing | DecoderInit | DecodeBatchRequest;

export type DecoderResponse =
  | {
      type: 'pong';
      epochMs: number;
      spans: Span[];
    }
  | {
      type: 'result';
      batchId: number;
      batchData: BatchData;
      partial: PartialAggregates;
      /** Spans recorded since the previous result, then drained. */
      spans: Span[];
    }
  | {type: 'error'; message: string};

// ---------------------------------------------------------------------------
// Partial aggregates returned by each decoder
// ---------------------------------------------------------------------------

/** Each decoder runs its own collectors over its slice of events. */
export interface PartialAggregates {
  /** AggregateData has loadSpans/actionSpans/analysisSpans (concat-mergeable)
   *  and criticalPath (we keep the one with lowest startEventIndex). The
   *  loadSpans path-lookup map is kept locally per helper; cross-helper
   *  spanStart/spanEnd straddlers are dropped (small fraction; acceptable
   *  for the first iteration). LoadPackage finalize is run by the main /
   *  dispatcher after merging. */
  loadSpans: AggregateData['loadSpans'];
  /** Per-helper buildFileMetrics maps for the LoadPackage finalize step
   *  (so the merged finalize sees all paths, not just the helper's own). */
  loadBuildFileMetrics: Array<
    [
      string,
      {
        starlarkPeakAllocatedBytes?: number;
        cpuInstructionCount?: number;
        targetCount?: number;
      },
    ]
  >;
  actionSpans: AggregateData['actionSpans'];
  analysisSpans: AggregateData['analysisSpans'];
  /** Helper's first-seen buildGraphInfo, plus the global event index it was
   *  seen at, so the dispatcher can pick the lowest-index across helpers. */
  criticalPath: AggregateData['criticalPath'];
  criticalPathEventIndex: number;
}
