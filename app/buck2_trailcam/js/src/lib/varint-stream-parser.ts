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
 * Streaming parser for varint-length-delimited protobuf messages.
 *
 * Handles messages that span decompression chunk boundaries by maintaining
 * a remainder buffer between chunks.
 */

import type {PhaseTimer} from './phase-timer';
import {nowMs} from './phase-timer';

/**
 * Read a varint from a buffer at the given offset.
 * Returns [value, bytesRead] or null if the buffer ends mid-varint.
 */
function tryReadVarint(
  buf: Uint8Array,
  offset: number,
): [number, number] | null {
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
  // Buffer ended mid-varint
  return null;
}

export class VarintStreamParser {
  private remainder: Uint8Array | null = null;

  /**
   * Feed a chunk of decompressed bytes and yield complete messages.
   * Any trailing incomplete message is buffered for the next call.
   *
   * If a `timer` is supplied, the per-message length-decode and slice-copy
   * costs are recorded into separate `varint_decode_length` and
   * `varint_slice` buckets so callers can see which dominates. The timing
   * adds ~2 `performance.now()` calls per message (~100ns).
   */
  *parse(chunk: Uint8Array, timer?: PhaseTimer): Generator<Uint8Array> {
    let buf: Uint8Array;
    if (this.remainder && this.remainder.length > 0) {
      // Prepend remainder from previous chunk
      const combined = new Uint8Array(this.remainder.length + chunk.length);
      combined.set(this.remainder);
      combined.set(chunk, this.remainder.length);
      buf = combined;
      this.remainder = null;
    } else {
      buf = chunk;
    }

    let offset = 0;
    while (offset < buf.length) {
      // Try to read the varint length prefix
      const varintT0 = timer ? nowMs() : 0;
      const varintResult = tryReadVarint(buf, offset);
      if (timer) timer.add('varint_decode_length', nowMs() - varintT0);
      if (varintResult === null) {
        // Buffer ends mid-varint — save remainder
        this.remainder = buf.slice(offset);
        return;
      }

      const [msgLen, varintBytes] = varintResult;
      const msgStart = offset + varintBytes;
      const msgEnd = msgStart + msgLen;

      if (msgEnd > buf.length) {
        // Message extends past buffer — save remainder
        this.remainder = buf.slice(offset);
        return;
      }

      // Copy the message bytes so we don't pin the entire decompressed chunk
      const sliceT0 = timer ? nowMs() : 0;
      const slice = buf.slice(msgStart, msgEnd);
      if (timer) timer.add('varint_slice', nowMs() - sliceT0);
      yield slice;
      offset = msgEnd;
    }

    // Consumed everything, no remainder
    this.remainder = null;
  }

  /**
   * Flush any remaining bytes. Should be called after the last chunk.
   * Returns the remaining bytes if any (indicates a truncated message).
   */
  flush(): Uint8Array | null {
    const r = this.remainder;
    this.remainder = null;
    return r && r.length > 0 ? r : null;
  }

  /**
   * Parse a chunk in place: returns message offsets/lengths within the
   * input chunk, plus an optional reassembled "leading" message (a
   * message that started in the previous chunk and completes at the
   * start of this one). Internally maintains a remainder for messages
   * that extend past the end of the chunk.
   *
   * Unlike `parse()`, this does NOT copy each message's bytes. Callers
   * can either subarray the chunk for in-place reads or transfer the
   * whole chunk buffer to another worker. Trade-off: the caller must
   * keep the chunk buffer alive (or transfer it) while processing the
   * returned offsets.
   */
  parseInPlace(chunk: Uint8Array): ParseInPlaceResult {
    let leading: Uint8Array | null = null;
    let chunkOffset = 0;

    if (this.remainder && this.remainder.length > 0) {
      const remainder = this.remainder;
      this.remainder = null;

      // Try to read the varint header from remainder alone.
      let varintResult = tryReadVarint(remainder, 0);
      if (varintResult) {
        const [msgLen, varintBytes] = varintResult;
        const msgEnd = varintBytes + msgLen;
        const bytesNeededFromChunk = msgEnd - remainder.length;

        if (chunk.length >= bytesNeededFromChunk) {
          // Reassemble: bytes from remainder (after varint header) + bytes
          // from start of new chunk. One small allocation per straddler.
          leading = new Uint8Array(msgLen);
          leading.set(remainder.subarray(varintBytes), 0);
          leading.set(
            chunk.subarray(0, bytesNeededFromChunk),
            remainder.length - varintBytes,
          );
          chunkOffset = bytesNeededFromChunk;
        } else {
          // Even with this chunk, can't complete the message. Combine
          // and store as the new remainder for the next chunk.
          const newRemainder = new Uint8Array(remainder.length + chunk.length);
          newRemainder.set(remainder);
          newRemainder.set(chunk, remainder.length);
          this.remainder = newRemainder;
          return EMPTY_RESULT;
        }
      } else {
        // Remainder doesn't even have the full varint header. Combine.
        const combined = new Uint8Array(remainder.length + chunk.length);
        combined.set(remainder);
        combined.set(chunk, remainder.length);
        varintResult = tryReadVarint(combined, 0);
        if (!varintResult) {
          this.remainder = combined;
          return EMPTY_RESULT;
        }
        const [msgLen, varintBytes] = varintResult;
        const msgEnd = varintBytes + msgLen;
        if (msgEnd > combined.length) {
          this.remainder = combined;
          return EMPTY_RESULT;
        }
        leading = combined.slice(varintBytes, msgEnd);
        chunkOffset = msgEnd - remainder.length;
      }
    }

    // Now parse messages in `chunk` starting at chunkOffset.
    const offsets: number[] = [];
    const lengths: number[] = [];

    while (chunkOffset < chunk.length) {
      const varintResult = tryReadVarint(chunk, chunkOffset);
      if (varintResult === null) {
        this.remainder = chunk.slice(chunkOffset);
        return {
          leading,
          offsets: new Uint32Array(offsets),
          lengths: new Uint32Array(lengths),
        };
      }
      const [msgLen, varintBytes] = varintResult;
      const msgStart = chunkOffset + varintBytes;
      const msgEnd = msgStart + msgLen;
      if (msgEnd > chunk.length) {
        this.remainder = chunk.slice(chunkOffset);
        return {
          leading,
          offsets: new Uint32Array(offsets),
          lengths: new Uint32Array(lengths),
        };
      }
      offsets.push(msgStart);
      lengths.push(msgLen);
      chunkOffset = msgEnd;
    }

    return {
      leading,
      offsets: new Uint32Array(offsets),
      lengths: new Uint32Array(lengths),
    };
  }
}

export interface ParseInPlaceResult {
  /** A message reassembled from the previous chunk's remainder + the
   *  start of this chunk. Null if no straddler was completed. Comes
   *  first in event order, before any in-chunk messages. */
  leading: Uint8Array | null;
  /** Offsets within the input chunk for messages entirely contained
   *  within it. */
  offsets: Uint32Array;
  /** Lengths matching `offsets`. */
  lengths: Uint32Array;
}

const EMPTY_U32 = new Uint32Array(0);
const EMPTY_RESULT: ParseInPlaceResult = {
  leading: null,
  offsets: EMPTY_U32,
  lengths: EMPTY_U32,
};
