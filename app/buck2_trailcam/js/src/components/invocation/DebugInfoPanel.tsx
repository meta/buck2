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

import {ChevronLeftIcon, ChevronRightIcon} from 'lucide-react';
import {useState, type ReactNode} from 'react';
import {useLocalStorageState} from '../../lib/use-local-storage-state';

const STORAGE_KEY = 'trailcam:debug-info-panel:collapsed';
const COLLAPSED_W = 'w-10';
const EXPANDED_W = 'w-72';
const ANIMATION_MS = 150;

/**
 * Right-side panel on the Overview tab for debug-oriented information
 * (Scuba links, log downloads, etc.). Collapse state persists across
 * reloads and across navigating between invocation UUIDs.
 *
 * Width animates between expanded and collapsed states (~150ms). To avoid
 * a jarring content swap mid-animation:
 *   - When collapsing, keep the expanded contents rendered (clipped by
 *     overflow:hidden as the width shrinks), then swap to the collapsed
 *     bar after `transitionend`.
 *   - When expanding, swap to expanded contents immediately so they fill
 *     in as the width grows.
 */
export default function DebugInfoPanel({children}: {children: ReactNode}) {
  const [collapsed, setCollapsed] = useLocalStorageState<boolean>(
    STORAGE_KEY,
    false,
  );
  // Tracks whether to render the expanded view. During a collapse animation
  // this stays `true` while `collapsed` is `true` (until transitionend fires).
  const [showExpanded, setShowExpanded] = useState(!collapsed);

  function toggle() {
    const next = !collapsed;
    setCollapsed(next);
    if (!next) {
      // Expanding: show expanded contents now so they're visible as the
      // panel grows.
      setShowExpanded(true);
    }
    // Collapsing: keep showing expanded view; swap on transitionend below.
  }

  function onTransitionEnd(e: React.TransitionEvent<HTMLDivElement>) {
    // Only react to the width transition (not nested transitions like
    // opacity on hover backgrounds).
    if (e.propertyName !== 'width') return;
    if (collapsed) setShowExpanded(false);
  }

  return (
    <div
      onTransitionEnd={onTransitionEnd}
      className={`shrink-0 transition-[width] ease-out ${collapsed ? COLLAPSED_W : EXPANDED_W}`}
      style={{transitionDuration: `${ANIMATION_MS}ms`}}>
      <div className="text-card-foreground sticky top-0 overflow-hidden rounded-xl border border-border bg-card shadow">
        {showExpanded ? (
          <div className={EXPANDED_W}>
            <div className="flex items-center justify-between gap-2 border-b border-border px-3 py-2">
              <span className="text-sm font-semibold">Debug info</span>
              <button
                onClick={toggle}
                className="text-muted-foreground hover:text-foreground rounded p-0.5 hover:bg-gray-100 dark:hover:bg-gray-800"
                title="Hide debug info">
                <ChevronRightIcon className="size-4" />
              </button>
            </div>
            <div className="divide-y divide-border">{children}</div>
          </div>
        ) : (
          <button
            onClick={toggle}
            className="text-muted-foreground hover:text-foreground flex h-32 w-full flex-col items-center justify-start gap-2 py-2"
            title="Show debug info">
            <ChevronLeftIcon className="size-4 shrink-0" />
            <span
              className="text-sm font-semibold"
              style={{writingMode: 'vertical-rl'}}>
              Debug info
            </span>
          </button>
        )}
      </div>
    </div>
  );
}

export function DebugInfoSection({
  title,
  children,
}: {
  title: string;
  children: ReactNode;
}) {
  return (
    <div className="space-y-2 px-3 py-2.5">
      <div className="text-muted-foreground text-[10px] font-semibold uppercase tracking-wide">
        {title}
      </div>
      {children}
    </div>
  );
}
