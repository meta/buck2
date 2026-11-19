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

/**
 * Renders text containing ANSI escape codes as styled React spans, using the
 * `anser` library. Designed for action stdout/stderr that buck2 captures —
 * we cover what's typically produced by actions (SGR coloring, bold, dim,
 * italic, underline) but don't try to be a full terminal emulator.
 *
 * Basic 16 ANSI colors are mapped to CSS variables (defined in globals.css)
 * so the palette can adapt to light/dark theme. 256-color and truecolor
 * pass through as literal rgb() values.
 *
 * Adapted from nest/apps/projects/metamate/lib/components/AnsiOutput.tsx.
 */

import Anser, {type AnserJsonEntry} from 'anser';
import {useMemo, type CSSProperties, type ReactNode} from 'react';

// Anser emits these RGB strings for the basic 16 colors via its built-in
// palette when use_classes is false. We swap them for CSS variable refs so
// the theme can recolor them.
const BASIC_COLOR_MAP: Record<string, string> = {
  '0, 0, 0': 'var(--ansi-black)',
  '187, 0, 0': 'var(--ansi-red)',
  '0, 187, 0': 'var(--ansi-green)',
  '187, 187, 0': 'var(--ansi-yellow)',
  '0, 0, 187': 'var(--ansi-blue)',
  '187, 0, 187': 'var(--ansi-magenta)',
  '0, 187, 187': 'var(--ansi-cyan)',
  '255,255,255': 'var(--ansi-white)',
  '85, 85, 85': 'var(--ansi-bright-black)',
  '255, 85, 85': 'var(--ansi-bright-red)',
  '0, 255, 0': 'var(--ansi-bright-green)',
  '255, 255, 85': 'var(--ansi-bright-yellow)',
  '85, 85, 255': 'var(--ansi-bright-blue)',
  '255, 85, 255': 'var(--ansi-bright-magenta)',
  '85, 255, 255': 'var(--ansi-bright-cyan)',
  '255, 255, 255': 'var(--ansi-bright-white)',
};

function rgbToColor(rgb: string): string {
  return BASIC_COLOR_MAP[rgb] ?? `rgb(${rgb})`;
}

/**
 * Apply terminal-style \r overwrite semantics so progress lines don't print
 * their intermediate states. `\r\n` is treated as a newline; a standalone
 * `\r` overwrites the start of the current line.
 */
function escapeCarriageReturn(text: string): string {
  if (text.indexOf('\r') < 0) return text;
  return text
    .replace(/\r\n/g, '\n')
    .split('\n')
    .map(line => {
      const parts = line.split('\r');
      if (parts.length === 1) return line;
      let out = parts[0];
      for (let i = 1; i < parts.length; i++) {
        const overwrite = parts[i];
        out = overwrite + out.slice(overwrite.length);
      }
      return out;
    })
    .join('\n');
}

/**
 * Remove ANSI escape sequences that anser doesn't render — cursor movement,
 * clear-line, OSC titles/hyperlinks, etc. Keeps SGR (CSI…m) sequences which
 * anser does handle. Anything else just leaks through as visible junk.
 *
 * The regex targets all standard ANSI escape forms:
 *   - CSI:  ESC [ <params> <intermediate> <final>     (kept iff final == 'm')
 *   - OSC:  ESC ] <payload> (BEL | ESC \)
 *   - DCS / SOS / PM / APC: ESC (P|X|^|_) <payload> ST
 *   - Two-byte ESC + Fe byte (e.g. ESC 7 = save cursor)
 */
export function stripUnhandledAnsi(text: string): string {
  if (text.indexOf('\x1b') < 0) return text;

  // OSC and similar string-terminator sequences.
  text = text.replace(/\x1b[\]PX^_][\s\S]*?(?:\x07|\x1b\\)/g, '');

  // CSI sequences. Keep SGR (final byte 'm'); strip the rest.
  text = text.replace(/\x1b\[[\x30-\x3f]*[\x20-\x2f]*[\x40-\x7e]/g, m =>
    m.charCodeAt(m.length - 1) === 0x6d /* 'm' */ ? m : '',
  );

  // Anything still ESC-prefixed at this point is either a two-byte Fe / Fp
  // escape (ESC 7 = save cursor, ESC c = reset, etc.) or a leftover from a
  // malformed sequence. Strip ESC + the next byte. The exception is `[` —
  // the CSI introducer, which step 2 already handled (kept iff SGR), so
  // any remaining `\x1b[` is part of an SGR sequence we want to preserve.
  text = text.replace(/\x1b[^\[]/g, '');

  return text;
}

function entryStyle(entry: AnserJsonEntry): CSSProperties | undefined {
  const style: CSSProperties = {};
  let any = false;

  if (entry.fg) {
    style.color = rgbToColor(entry.fg);
    any = true;
  }
  if (entry.bg) {
    style.backgroundColor = rgbToColor(entry.bg);
    any = true;
  }
  if (entry.decorations) {
    for (const d of entry.decorations) {
      switch (d) {
        case 'bold':
          style.fontWeight = 'bold';
          any = true;
          break;
        case 'dim':
          style.opacity = 0.6;
          any = true;
          break;
        case 'italic':
          style.fontStyle = 'italic';
          any = true;
          break;
        case 'underline':
          style.textDecoration = 'underline';
          any = true;
          break;
        case 'strikethrough':
          style.textDecoration = 'line-through';
          any = true;
          break;
        case 'hidden':
          style.visibility = 'hidden';
          any = true;
          break;
      }
    }
  }

  return any ? style : undefined;
}

interface AnsiOutputProps {
  children: string;
  className?: string;
}

export function AnsiOutput({children, className}: AnsiOutputProps): ReactNode {
  const segments = useMemo(() => {
    const cleaned = stripUnhandledAnsi(escapeCarriageReturn(children));
    return Anser.ansiToJson(cleaned, {
      use_classes: false,
      json: true,
      remove_empty: true,
    });
  }, [children]);

  return (
    <code className={className}>
      {segments.map((segment, i) => {
        const style = entryStyle(segment);
        return style ? (
          <span key={i} style={style}>
            {segment.content}
          </span>
        ) : (
          <span key={i}>{segment.content}</span>
        );
      })}
    </code>
  );
}

/** Cheap test for ANSI escape sequences. */
export function hasAnsiCodes(text: string): boolean {
  return text.indexOf('\x1b[') >= 0;
}
