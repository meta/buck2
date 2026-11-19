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

import {useState} from 'react';

/**
 * A text message that's shown as a single truncated line by default and
 * expands to its full multi-line content on click. Use for errors, log
 * lines, or any potentially-long text where the first line is usually
 * enough to triage but the user may want the whole thing.
 *
 * Inherits the surrounding text color so red error styling, etc., still
 * applies — only the chevron is dimmed.
 */
export default function CollapsibleMessage({
  text,
  className = '',
}: {
  text: string;
  className?: string;
}) {
  const [expanded, setExpanded] = useState(false);
  // No expansion needed for short single-line messages — render plainly.
  const needsExpansion = text.includes('\n') || text.length > 120;

  if (!needsExpansion) {
    return <div className={className}>{text}</div>;
  }

  return (
    <button
      onClick={() => setExpanded(v => !v)}
      className={`flex w-full items-start gap-1.5 text-left ${className}`}>
      <span className="mt-0.5 shrink-0 text-[10px] opacity-60">
        {expanded ? '▾' : '▸'}
      </span>
      <span
        className={`min-w-0 flex-1 ${
          expanded ? 'whitespace-pre-wrap break-words' : 'truncate'
        }`}>
        {text}
      </span>
    </button>
  );
}
