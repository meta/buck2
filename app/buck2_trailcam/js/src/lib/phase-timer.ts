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
 * Lightweight phase timer used to break down where time goes during event
 * log decoding. Designed to be cheap enough to leave on by default —
 * `performance.now()` costs ~50ns per call, so a few calls per protobuf
 * message adds up but is small compared to the protobuf decode itself.
 *
 * Used by both the streaming worker and the standalone Node benchmark
 * script. Snapshots are plain objects so they survive a postMessage
 * structured clone with no special handling.
 */

export interface PhaseTotals {
  totalMs: number;
  count: number;
}

export type PhaseTimings = Record<string, PhaseTotals>;

// `performance.now()` is available in browsers, web workers, and Node 16+
// (via globalThis.performance), so no fallback shim is needed.
const now = (): number => performance.now();

export class PhaseTimer {
  private totals = new Map<string, PhaseTotals>();

  /** Record a phase by manual timing (use when you've already measured dt). */
  add(phase: string, ms: number, count = 1): void {
    const existing = this.totals.get(phase);
    if (existing) {
      existing.totalMs += ms;
      existing.count += count;
    } else {
      this.totals.set(phase, {totalMs: ms, count});
    }
  }

  /** Time a synchronous block and accumulate into `phase`. Returns the result. */
  time<T>(phase: string, fn: () => T): T {
    const t0 = now();
    const result = fn();
    this.add(phase, now() - t0);
    return result;
  }

  /** Time an awaited block. */
  async timeAsync<T>(phase: string, fn: () => Promise<T>): Promise<T> {
    const t0 = now();
    const result = await fn();
    this.add(phase, now() - t0);
    return result;
  }

  /** Snapshot of current totals as a plain object — safe for postMessage. */
  report(): PhaseTimings {
    const out: PhaseTimings = {};
    for (const [k, v] of this.totals) {
      out[k] = {totalMs: v.totalMs, count: v.count};
    }
    return out;
  }

  reset(): void {
    this.totals.clear();
  }
}

/** Read the current high-resolution clock (ms). */
export function nowMs(): number {
  return now();
}

// ---------------------------------------------------------------------------
// Span recording
// ---------------------------------------------------------------------------

export interface Span {
  /** Phase / span name, e.g. 'decode_batch'. */
  phase: string;
  /** Worker-local performance.now() at start. */
  startMs: number;
  /** Worker-local performance.now() at end. */
  endMs: number;
  /** Optional key/value annotations attached during the span (e.g. row
   *  counts, byte sizes, decoder index). Rendered in the timeline
   *  tooltip so the reader can correlate a slow span with its inputs.
   *  Kept small — this rides through structured clone on every span. */
  attrs?: Record<string, string | number | boolean>;
}

/**
 * Records discrete (start, end) intervals for activities. Use this for the
 * coarse spans that drive the timeline visualization (per-chunk,
 * per-batch). Avoid using it inside hot per-message loops — the resulting
 * number of spans is unmanageable both in memory and at render time. For
 * cumulative per-message timing use `PhaseTimer` instead.
 */
export class SpanRecorder {
  spans: Span[] = [];

  /** Time a synchronous block; record one span. */
  time<T>(phase: string, fn: () => T): T {
    const startMs = now();
    const result = fn();
    this.spans.push({phase, startMs, endMs: now()});
    return result;
  }

  /** Time an awaited block. */
  async timeAsync<T>(phase: string, fn: () => Promise<T>): Promise<T> {
    const startMs = now();
    const result = await fn();
    this.spans.push({phase, startMs, endMs: now()});
    return result;
  }

  /**
   * Manual "begin … later .end()" pattern for spans whose end isn't
   * expressible as a single function-call boundary. Use the returned
   * `set(key, value)` to attach annotations that show up in the
   * timeline tooltip — useful for explaining *why* a span is long.
   */
  begin(phase: string): {
    end: () => void;
    set: (key: string, value: string | number | boolean) => void;
  } {
    const startMs = now();
    let attrs: Record<string, string | number | boolean> | undefined;
    return {
      end: () => this.spans.push({phase, startMs, endMs: now(), attrs}),
      set: (key, value) => {
        if (!attrs) attrs = {};
        attrs[key] = value;
      },
    };
  }

  /** Take all spans recorded so far, leaving the recorder empty. */
  drain(): Span[] {
    const out = this.spans;
    this.spans = [];
    return out;
  }
}

/**
 * One worker's contribution to the timeline view. Spans use the worker's
 * own `performance.now()` clock; the orchestrator computes `offsetMs`
 * once via a ping/pong handshake at startup
 * (`(t_send + t_recv) / 2 - workerEpoch` — half-RTT estimate of one-way
 * message latency) and applies `displayMs = span.startMs + offsetMs` to
 * render on the main thread's `performance.now()` axis.
 */
export interface TimelineLane {
  /** Display name, e.g. 'decompress', 'dispatch', 'decoder 0'. */
  workerName: string;
  /** Worker's `performance.now()` at handshake (its clock origin). */
  epochMs: number;
  /** Estimated offset (ms) to translate worker-local time to the main
   *  thread's `performance.now()` axis. */
  offsetMs: number;
  spans: Span[];
}

/** Sum up totals across phases. */
export function totalMs(timings: PhaseTimings): number {
  let sum = 0;
  for (const k of Object.keys(timings)) sum += timings[k].totalMs;
  return sum;
}

/**
 * Format a phase breakdown as a readable string, sorted by total time
 * descending. Used by the Node script and the in-browser stats panel
 * tooltip.
 */
export function formatPhases(timings: PhaseTimings): string {
  const entries = Object.entries(timings).sort(
    (a, b) => b[1].totalMs - a[1].totalMs,
  );
  const total = totalMs(timings);
  const lines: string[] = [];
  for (const [name, t] of entries) {
    const pct = total > 0 ? (t.totalMs / total) * 100 : 0;
    const avgUs = t.count > 0 ? (t.totalMs * 1000) / t.count : 0;
    lines.push(
      `  ${name.padEnd(20)} ${t.totalMs.toFixed(1).padStart(8)}ms  (${pct.toFixed(1).padStart(5)}%)  ${t.count.toString().padStart(8)} ops  avg ${avgUs.toFixed(1)}us`,
    );
  }
  lines.push(`  ${'TOTAL'.padEnd(20)} ${total.toFixed(1).padStart(8)}ms`);
  return lines.join('\n');
}
