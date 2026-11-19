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
 * Main-thread orchestrator for the parallel decode pipeline.
 *
 * Creates the decompress + dispatch workers + N decoder workers, wires up
 * cross-worker message routing through the main thread (so we don't have
 * to rely on nested-worker support), and exposes a Worker-shaped API
 * (`decode(...)` / `terminate()`) that EventLogProvider can plug in
 * alongside the existing single-worker path.
 */

import type {BatchData} from '../event-summary-store';
import type {AggregateData} from '../streaming-collectors';
import type {PhaseTimings, Span, TimelineLane} from '../phase-timer';
import {nowMs} from '../phase-timer';
import type {
  DecompressRequest,
  DecompressResponse,
  DispatchRequest,
  DispatchResponse,
  DecoderRequest,
  DecoderResponse,
} from './types';

export type ParallelDecoderEvent =
  | {
      type: 'progress';
      message: string;
      decompressedBytes?: number;
      phaseTimings?: PhaseTimings;
    }
  | {type: 'summaryBatch'; batch: BatchData}
  | {
      type: 'done';
      decompressedSize: number;
      /** Actual compressed bytes pulled from the network — preferred
       *  over Content-Length, which can be 0 / stale on chunked
       *  responses. */
      receivedBytes: number;
      totalEvents: number;
      chunkCount: number;
      criticalPath: AggregateData['criticalPath'];
      /** Inline copy of heavy aggregates for small logs only. Large
       *  logs leave this undefined; main reads via lazy IDB path. */
      inlineAggregates?: {
        loadSpans: AggregateData['loadSpans'];
        analysisSpans: AggregateData['analysisSpans'];
        actionSpans: AggregateData['actionSpans'];
      };
      phaseTimings: PhaseTimings;
      /** Assembled per-worker timeline. Each lane carries an `offsetMs`
       *  precomputed by the orchestrator so consumers can render all lanes
       *  on a single time axis. */
      timelineLanes: TimelineLane[];
    }
  /** Sent after `done` once the dispatcher's background IDB cache writes
   *  complete. Carries an updated `timelineLanes` snapshot that includes
   *  the dispatcher's tail spans (drain_pending_writes,
   *  write_worker_results). Updated `phaseTimings` likewise reflect the
   *  final IDB totals. */
  | {
      type: 'cacheWritten';
      timelineLanes: TimelineLane[];
      phaseTimings: PhaseTimings;
    }
  | {type: 'error'; message: string};

/** Per-worker bookkeeping kept by the orchestrator while a decode runs.
 *  `epochMs` and `offsetMs` are filled in by the ping/pong handshake; once
 *  that completes they're authoritative for the lane's lifetime. */
interface LaneAccum {
  workerName: string;
  /** Worker's `performance.now()` at ping receipt (its clock origin). */
  epochMs: number;
  /** Conversion: `displayMs = spanLocalMs + offsetMs` puts a worker-local
   *  span onto main's `performance.now()` axis. Computed as
   *  `(t_send + t_recv) / 2 - epochMs` from the ping/pong round-trip,
   *  which is the half-RTT estimate for one-way message latency. */
  offsetMs: number;
  spans: Span[];
}

function newLane(name: string): LaneAccum {
  return {workerName: name, epochMs: 0, offsetMs: 0, spans: []};
}

function laneToTimelineLane(lane: LaneAccum): TimelineLane {
  return {
    workerName: lane.workerName,
    epochMs: lane.epochMs,
    offsetMs: lane.offsetMs,
    spans: lane.spans,
  };
}

/** Result of one ping/pong handshake — applied to a lane in `decode()`. */
interface HandshakeResult {
  epochMs: number;
  offsetMs: number;
  /** A single `handshake` span emitted by the worker. */
  spans: Span[];
}

/**
 * Send a `ping` to a worker and resolve with the timing data needed to
 * align its clock to main's.
 *
 * The orchestrator records `t_send` immediately before posting and
 * `t_recv` when the pong arrives; the worker stamps its own epoch when
 * the ping arrives. We assume symmetric postMessage latency and credit
 * the worker's epoch to `(t_send + t_recv) / 2` on main's clock — this
 * is accurate to within ~½ message latency (typically sub-ms).
 *
 * Run from the constructor so it overlaps with whatever the caller is
 * doing in parallel (most importantly `fetch()` for the response body):
 * by the time `decode()` is called the workers are already warmed up
 * and the handshake is paid for.
 */
function handshake(worker: Worker): Promise<HandshakeResult> {
  return new Promise((resolve, reject) => {
    let tSend = 0;
    worker.onmessage = (
      e: MessageEvent<{type: string; epochMs?: number; spans?: Span[]}>,
    ) => {
      const msg = e.data;
      if (msg.type === 'pong' && typeof msg.epochMs === 'number') {
        const tRecv = nowMs();
        // `decode()` installs the long-term handler after applying the
        // result to its lane — clear ours first.
        worker.onmessage = null;
        resolve({
          epochMs: msg.epochMs,
          offsetMs: (tSend + tRecv) / 2 - msg.epochMs,
          spans: msg.spans ?? [],
        });
        return;
      }
      reject(new Error(`handshake: unexpected ${msg.type}`));
    };
    tSend = nowMs();
    worker.postMessage({type: 'ping'});
  });
}

function applyHandshake(lane: LaneAccum, result: HandshakeResult): void {
  lane.epochMs = result.epochMs;
  lane.offsetMs = result.offsetMs;
  for (const s of result.spans) lane.spans.push(s);
}

export interface ParallelDecodeOptions {
  numDecoders?: number;
}

export class ParallelDecoder {
  private decompressor: Worker;
  private dispatcher: Worker;
  private decoders: Worker[];
  private numDecoders: number;
  private terminated = false;

  /**
   * Per-worker handshake Promises. Kicked off in the constructor so they
   * run in parallel with whatever the caller is doing on main (notably
   * `fetch()` for the response body). `decode()` awaits all of them
   * before posting any real work.
   */
  private decompressorHandshake: Promise<HandshakeResult>;
  private dispatcherHandshake: Promise<HandshakeResult>;
  private decoderHandshakes: Promise<HandshakeResult>[];

  constructor(options: ParallelDecodeOptions = {}) {
    this.numDecoders = options.numDecoders ?? 4;
    this.decompressor = new Worker(
      new URL('./decompress-worker.ts', import.meta.url),
    );
    this.dispatcher = new Worker(
      new URL('./dispatch-worker.ts', import.meta.url),
    );
    this.decoders = Array.from(
      {length: this.numDecoders},
      () => new Worker(new URL('./decoder-worker.ts', import.meta.url)),
    );

    // Kick off the clock-sync handshake immediately so it overlaps with
    // the caller's setup work (e.g. fetch() for the response body).
    // Stash the rejection so an unawaited Promise doesn't trigger an
    // unhandledrejection if the caller terminates before calling decode().
    const swallow = (p: Promise<HandshakeResult>) => {
      p.catch(() => {});
      return p;
    };
    this.decompressorHandshake = swallow(handshake(this.decompressor));
    this.dispatcherHandshake = swallow(handshake(this.dispatcher));
    this.decoderHandshakes = this.decoders.map(d => swallow(handshake(d)));
  }

  /**
   * Start decoding a compressed event log.
   *
   * `compressedStream` is the network response body — we transfer it to
   * the decompress worker so download bytes flow straight into
   * decompression without main ever buffering them.
   */
  decode(
    compressedStream: ReadableStream<Uint8Array>,
    eventLogPath: string,
    compressedSize: number,
    onMessage: (e: ParallelDecoderEvent) => void,
  ): void {
    if (this.terminated) {
      onMessage({type: 'error', message: 'ParallelDecoder already terminated'});
      return;
    }

    // Mirrors the single-worker pipeline: above this threshold, the
    // ActionSpanCollector produces millions of entries that we can't
    // structured-clone (postMessage *or* IDB write). The treemap UI
    // already gates on isLargeLog so it won't try to use the data.
    //
    // We also skip when compressedSize is 0 (unknown — the parallel
    // pipeline is opt-in to chunked-encoding responses, where
    // Content-Length is missing). At that point we have no signal
    // about size, and "fail closed by skipping" is safer than the
    // alternative (running a 15M-entry collector and OOMing the IDB
    // write).
    const SKIP_ACTION_SPANS_COMPRESSED_BYTES = 100 * 1024 * 1024;
    const skipActionSpans =
      compressedSize <= 0 ||
      compressedSize > SKIP_ACTION_SPANS_COMPRESSED_BYTES;

    // Per-worker timeline accumulators. Filled in by the handshake (epoch +
    // offset + handshake span) and grown as work spans arrive.
    const decompressorLane = newLane('decompress');
    const dispatcherLane = newLane('dispatch');
    const decoderLanes: LaneAccum[] = Array.from(
      {length: this.numDecoders},
      (_, i) => newLane(`decoder ${i}`),
    );

    const installRealHandlers = (): void => {
      // Decompressor → main → dispatcher
      this.decompressor.onmessage = (e: MessageEvent<DecompressResponse>) => {
        if (this.terminated) return;
        const msg = e.data;
        if (msg.type === 'chunk') {
          const req: DispatchRequest = {type: 'chunk', data: msg.data};
          this.dispatcher.postMessage(req, [msg.data]);
        } else if (msg.type === 'done') {
          for (const s of msg.spans) decompressorLane.spans.push(s);
          const req: DispatchRequest = {
            type: 'decompressDone',
            decompressedBytes: msg.decompressedBytes,
            receivedBytes: msg.receivedBytes,
          };
          this.dispatcher.postMessage(req);
        } else if (msg.type === 'error') {
          onMessage({type: 'error', message: msg.message});
        }
      };

      // Dispatcher → main; main routes 'toDecoder' messages onward.
      this.dispatcher.onmessage = (e: MessageEvent<DispatchResponse>) => {
        if (this.terminated) return;
        const msg = e.data;
        switch (msg.type) {
          case 'toDecoder': {
            const target = this.decoders[msg.decoder];
            if (!target) return;
            target.postMessage(msg.payload as DecoderRequest, msg.transfer);
            break;
          }
          case 'summaryBatch':
            onMessage({type: 'summaryBatch', batch: msg.batch});
            break;
          case 'progress':
            onMessage({
              type: 'progress',
              message: msg.message,
              decompressedBytes: msg.decompressedBytes,
              phaseTimings: msg.phaseTimings,
            });
            break;
          case 'done':
            for (const s of msg.spans) dispatcherLane.spans.push(s);
            onMessage({
              type: 'done',
              decompressedSize: msg.decompressedBytes,
              receivedBytes: msg.receivedBytes,
              totalEvents: msg.totalEvents,
              chunkCount: msg.chunkCount,
              criticalPath: msg.criticalPath,
              inlineAggregates: msg.inlineAggregates,
              phaseTimings: msg.phaseTimings,
              timelineLanes: [
                laneToTimelineLane(decompressorLane),
                laneToTimelineLane(dispatcherLane),
                ...decoderLanes.map(laneToTimelineLane),
              ],
            });
            break;
          case 'cacheWritten':
            for (const s of msg.tailSpans) dispatcherLane.spans.push(s);
            onMessage({
              type: 'cacheWritten',
              timelineLanes: [
                laneToTimelineLane(decompressorLane),
                laneToTimelineLane(dispatcherLane),
                ...decoderLanes.map(laneToTimelineLane),
              ],
              phaseTimings: msg.tailPhaseTimings,
            });
            this.terminate();
            break;
          case 'error':
            onMessage({type: 'error', message: msg.message});
            break;
        }
      };

      // Decoder N → main → dispatcher
      this.decoders.forEach((decoder, i) => {
        decoder.onmessage = (e: MessageEvent<DecoderResponse>) => {
          if (this.terminated) return;
          const msg = e.data;
          if (msg.type === 'result') {
            for (const s of msg.spans) decoderLanes[i].spans.push(s);
            const req: DispatchRequest = {
              type: 'decoderResult',
              from: i,
              batchId: msg.batchId,
              batchData: msg.batchData,
              partial: msg.partial,
              spans: [],
            };
            this.dispatcher.postMessage(req);
          } else if (msg.type === 'error') {
            onMessage({type: 'error', message: `decoder ${i}: ${msg.message}`});
          }
        };
      });
    };

    this.decompressor.onerror = () => {
      onMessage({type: 'error', message: 'decompress worker crashed'});
    };
    this.dispatcher.onerror = () => {
      onMessage({type: 'error', message: 'dispatch worker crashed'});
    };
    this.decoders.forEach((decoder, i) => {
      decoder.onerror = () => {
        onMessage({type: 'error', message: `decoder ${i} worker crashed`});
      };
    });

    // The clock-sync handshakes were kicked off in the constructor so
    // they overlap with the caller's parallel work (typically fetch()).
    // Await them, populate lane state with each worker's epoch/offset/
    // handshake span, then post real work.
    Promise.all([
      this.decompressorHandshake,
      this.dispatcherHandshake,
      ...this.decoderHandshakes,
    ]).then(
      ([decompResult, dispResult, ...decoderResults]) => {
        if (this.terminated) return;
        applyHandshake(decompressorLane, decompResult);
        applyHandshake(dispatcherLane, dispResult);
        decoderResults.forEach((r, i) => applyHandshake(decoderLanes[i], r));
        installRealHandlers();

        this.decoders.forEach((decoder, i) => {
          const init: DecoderRequest = {
            type: 'init',
            decoderIndex: i,
            skipActionSpans,
          };
          decoder.postMessage(init);
        });

        const dispInit: DispatchRequest = {
          type: 'init',
          numDecoders: this.numDecoders,
          eventLogPath,
          compressedSize,
        };
        this.dispatcher.postMessage(dispInit);

        // Kick off decompression — transfer the stream so the worker reads
        // network bytes directly with no main-thread buffering.
        const decompReq: DecompressRequest = {
          type: 'decompress',
          stream: compressedStream,
        };
        this.decompressor.postMessage(decompReq, [
          compressedStream as unknown as Transferable,
        ]);
      },
      (err: unknown) => {
        onMessage({
          type: 'error',
          message:
            err instanceof Error
              ? err.message
              : 'parallel decode handshake failed',
        });
      },
    );
  }

  terminate(): void {
    this.terminated = true;
    this.decompressor.terminate();
    this.dispatcher.terminate();
    for (const d of this.decoders) d.terminate();
  }
}
