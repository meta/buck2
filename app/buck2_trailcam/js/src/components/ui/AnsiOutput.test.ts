/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

import {describe, it, expect} from 'vitest';
import {stripUnhandledAnsi, hasAnsiCodes} from './AnsiOutput';

const ESC = '\x1b';

describe('hasAnsiCodes', () => {
  it('returns false for plain text', () => {
    expect(hasAnsiCodes('hello world')).toBe(false);
    expect(hasAnsiCodes('')).toBe(false);
  });

  it('returns true when any CSI sequence is present', () => {
    expect(hasAnsiCodes(`hello${ESC}[31mworld${ESC}[0m`)).toBe(true);
  });
});

describe('stripUnhandledAnsi', () => {
  it('passes plain text through unchanged', () => {
    expect(stripUnhandledAnsi('hello world\n')).toBe('hello world\n');
  });

  it('preserves SGR sequences (the ones anser renders)', () => {
    const sgr = `${ESC}[31mred${ESC}[0m and ${ESC}[1mbold${ESC}[22m`;
    expect(stripUnhandledAnsi(sgr)).toBe(sgr);
  });

  it('preserves SGR with multiple parameters', () => {
    const sgr = `${ESC}[1;31;47mtext${ESC}[0m`;
    expect(stripUnhandledAnsi(sgr)).toBe(sgr);
  });

  it('strips non-SGR CSI sequences (cursor movement, clear-line)', () => {
    expect(stripUnhandledAnsi(`${ESC}[2K`)).toBe('');
    expect(stripUnhandledAnsi(`${ESC}[A`)).toBe(''); // cursor up
    expect(stripUnhandledAnsi(`${ESC}[?25h`)).toBe(''); // show cursor (private)
    expect(stripUnhandledAnsi(`${ESC}[10;5H`)).toBe(''); // cursor position
  });

  it('strips OSC 8 hyperlinks but keeps the inner visible text', () => {
    const url = 'https://example.com/foo?a=1&b=2';
    const input = `before ${ESC}]8;;${url}${ESC}\\link text${ESC}]8;;${ESC}\\ after`;
    expect(stripUnhandledAnsi(input)).toBe('before link text after');
  });

  it('strips OSC 0 (set window title) terminated by BEL', () => {
    const input = `${ESC}]0;my title\x07hello`;
    expect(stripUnhandledAnsi(input)).toBe('hello');
  });

  it('strips DCS / PM / APC string sequences', () => {
    expect(stripUnhandledAnsi(`a${ESC}Pdcs payload${ESC}\\b`)).toBe('ab');
    expect(stripUnhandledAnsi(`a${ESC}^pm payload${ESC}\\b`)).toBe('ab');
    expect(stripUnhandledAnsi(`a${ESC}_apc payload${ESC}\\b`)).toBe('ab');
  });

  it('strips two-byte Fe / Fp escapes', () => {
    expect(stripUnhandledAnsi(`a${ESC}7b${ESC}8c`)).toBe('abc'); // save / restore cursor (Fp)
    expect(stripUnhandledAnsi(`a${ESC}D b`)).toBe('a b'); // ESC D = index (Fe)
    expect(stripUnhandledAnsi(`a${ESC}M b`)).toBe('a b'); // ESC M = reverse index (Fe)
    expect(stripUnhandledAnsi(`a${ESC}cb`)).toBe('ab'); // ESC c = reset (Fp)
  });

  it('does NOT eat the `\\x1b[` prefix of an SGR sequence', () => {
    // Regression for a bug where the C1 strip pass treated `[` (0x5b) as a
    // valid Fe byte and consumed it, leaving "92m" leaking through.
    const input = `${ESC}[92mhello${ESC}[0m`;
    expect(stripUnhandledAnsi(input)).toBe(input);
  });

  it('handles SGR mixed with OSC 8 hyperlinks (kotlinc-style output)', () => {
    // Real-world shape: green opening SGR, OSC 8 hyperlink wrapping a path,
    // followed by more SGR. All hyperlinks stripped; SGR preserved.
    const url =
      'https://www.internalfb.com/intern/nuclide/open/arc/?paths[0]=foo';
    const input = `${ESC}[92m${ESC}]8;;${url}${ESC}\\foo/bar.kt${ESC}]8;;${ESC}\\${ESC}[0m:${ESC}[95m18${ESC}[0m`;
    expect(stripUnhandledAnsi(input)).toBe(
      `${ESC}[92mfoo/bar.kt${ESC}[0m:${ESC}[95m18${ESC}[0m`,
    );
  });

  it('does not get tripped up by `[` / `]` characters inside an OSC 8 URL', () => {
    // The kotlinc paste includes `lines[0]=18` inside the hyperlink URL.
    // The non-greedy [\s\S]*? must still find the next ESC\\ terminator and
    // not run away.
    const url = 'https://x?paths[0]=a&lines[0]=18';
    const input = `${ESC}]8;;${url}${ESC}\\hello${ESC}]8;;${ESC}\\`;
    expect(stripUnhandledAnsi(input)).toBe('hello');
  });

  it('handles back-to-back SGR sequences (no spaces between)', () => {
    const input = `${ESC}[91merror${ESC}[0m${ESC}[1m: msg${ESC}[0m`;
    expect(stripUnhandledAnsi(input)).toBe(input);
  });

  it('returns the input unchanged when there is no ESC byte', () => {
    const plain = 'just some text\nwith newlines\nand tabs\there';
    expect(stripUnhandledAnsi(plain)).toBe(plain);
  });

  it('strips `\\x1b\\\\` (lone string terminator) as a Fe escape', () => {
    // Should never appear standalone in practice, but if it does, treat as
    // a 2-byte Fe escape and drop it.
    expect(stripUnhandledAnsi(`a${ESC}\\b`)).toBe('ab');
  });
});
