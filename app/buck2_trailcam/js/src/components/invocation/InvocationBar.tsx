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

import {useState, useCallback, useEffect, useRef} from 'react';
import {createPortal} from 'react-dom';
import {Copy, Check} from 'lucide-react';
import {useEventLog} from './EventLogProvider';
import {useExpandedCommand} from './useExpandedCommand';

/**
 * Id of the element the bar renders into. Hosts place an empty element with
 * this id in their page header; the bar falls back to rendering inline when
 * none exists.
 */
export const TOP_BAR_SLOT_ID = 'trailcam-top-bar-slot';

function outcomeColor(outcome: string | null | undefined): string {
  switch (outcome) {
    case 'SUCCESS':
      return 'border-green-500';
    case 'FAILURE':
    case 'CRASHED':
      return 'border-red-500';
    case 'RUNNING':
      return 'border-blue-500';
    case 'CANCELED':
      return 'border-gray-400';
    default:
      return 'border-gray-300 dark:border-gray-600';
  }
}

/** Replace the first arg (binary path) with "buck2". */
function normalizeBinary(args: string[]): string[] {
  if (args.length === 0) return args;
  return ['buck2', ...args.slice(1)];
}

function CopyIcon({text}: {text: string}) {
  const [copied, setCopied] = useState(false);
  const handleCopy = useCallback(async () => {
    await navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  }, [text]);

  return (
    <button
      onClick={handleCopy}
      className="text-muted-foreground hover:text-foreground shrink-0 p-0.5 transition-colors"
      title="Copy command">
      {copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
    </button>
  );
}

export default function InvocationBar({
  id,
  commandOutcome,
  cliArgs,
}: {
  id: string | null;
  commandOutcome: string | null;
  cliArgs: ReadonlyArray<string | null> | null;
}) {
  const fullCommand = normalizeBinary(
    (cliArgs?.filter(Boolean) as string[]) ?? [],
  ).join(' ');

  const expandedCommand = useExpandedCommand(id ?? '');
  const logState = useEventLog();
  const expandedLoading =
    logState.status === 'idle' || logState.status === 'loading';
  const [showExpanded, setShowExpanded] = useState(false);

  // Find the portal target after mount
  const [slotEl, setSlotEl] = useState<HTMLElement | null>(null);
  useEffect(() => {
    setSlotEl(document.getElementById(TOP_BAR_SLOT_ID));
  }, []);

  const hasDifferentExpanded =
    expandedCommand != null && expandedCommand !== fullCommand;
  const displayedCommand =
    showExpanded && hasDifferentExpanded ? expandedCommand : fullCommand;

  // Expand-on-hover/click for truncated command
  const [cmdExpanded, setCmdExpanded] = useState(false);
  const [cmdRect, setCmdRect] = useState<{left: number; top: number} | null>(
    null,
  );
  const cmdRef = useRef<HTMLDivElement>(null);
  const codeRef = useRef<HTMLElement>(null);
  const hoverTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const isTruncated = useCallback(() => {
    const el = codeRef.current;
    if (!el) return false;
    return el.scrollWidth > el.clientWidth;
  }, []);

  const expandCmd = useCallback(() => {
    if (!isTruncated()) return;
    if (cmdRef.current) {
      const rect = cmdRef.current.getBoundingClientRect();
      setCmdRect({left: rect.left, top: rect.top});
    }
    setCmdExpanded(true);
  }, [isTruncated]);

  const hasSelection = useCallback(() => {
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed || !cmdRef.current) return false;
    return cmdRef.current.contains(sel.anchorNode);
  }, []);

  const collapseCmd = useCallback(() => {
    if (hasSelection()) return;
    setCmdExpanded(false);
    if (hoverTimer.current) {
      clearTimeout(hoverTimer.current);
      hoverTimer.current = null;
    }
  }, [hasSelection]);

  const onMouseEnter = useCallback(() => {
    if (hoverTimer.current) clearTimeout(hoverTimer.current);
    hoverTimer.current = setTimeout(expandCmd, 500);
  }, [expandCmd]);

  const collapseTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const onMouseLeave = useCallback(() => {
    if (hoverTimer.current) {
      clearTimeout(hoverTimer.current);
      hoverTimer.current = null;
    }
    collapseTimer.current = setTimeout(collapseCmd, 300);
  }, [collapseCmd]);

  // Cancel collapse if mouse re-enters
  const onMouseEnterOuter = useCallback(() => {
    if (collapseTimer.current) {
      clearTimeout(collapseTimer.current);
      collapseTimer.current = null;
    }
    onMouseEnter();
  }, [onMouseEnter]);

  // Keep expanded while text is selected within the command box
  useEffect(() => {
    function onSelectionChange() {
      if (hasSelection()) {
        expandCmd();
      }
    }
    document.addEventListener('selectionchange', onSelectionChange);
    return () =>
      document.removeEventListener('selectionchange', onSelectionChange);
  }, [hasSelection, expandCmd]);

  const onClick = useCallback(() => {
    expandCmd();
  }, [expandCmd]);

  const content = (
    <div className="flex min-w-0 flex-1 items-center gap-3">
      {/* UUID pill with status border */}
      <code
        className={`shrink-0 rounded-md border-2 px-2 py-0.5 text-xs ${outcomeColor(commandOutcome)}`}>
        {id ?? '—'}
      </code>

      {/* Command args — expands in place when truncated */}
      <div
        ref={cmdRef}
        className={`relative z-40 flex min-w-0 items-start gap-1.5 rounded bg-amber-50 px-2 py-0.5 transition-all duration-200 dark:bg-amber-950 ${
          cmdExpanded ? 'shadow-lg' : ''
        }`}
        style={
          cmdExpanded && cmdRect
            ? {
                position: 'fixed',
                left: cmdRect.left,
                top: cmdRect.top,
                right: 48,
              }
            : undefined
        }
        onMouseEnter={onMouseEnterOuter}
        onMouseLeave={onMouseLeave}
        onClick={onClick}>
        {displayedCommand && <CopyIcon text={displayedCommand} />}
        <code
          ref={codeRef}
          className={`min-w-0 cursor-default font-mono text-sm transition-all duration-200 ${
            cmdExpanded ? 'whitespace-pre-wrap break-all' : 'truncate'
          }`}>
          {displayedCommand}
        </code>
        {hasDifferentExpanded ? (
          <button
            onClick={e => {
              e.stopPropagation();
              setShowExpanded(!showExpanded);
            }}
            className="text-muted-foreground hover:text-foreground shrink-0 text-[10px] font-medium transition-colors"
            title={showExpanded ? 'Show simple args' : 'Show expanded args'}>
            {showExpanded ? '[show simple]' : '[show expanded]'}
          </button>
        ) : expandedLoading ? (
          <div className="size-3 shrink-0 animate-spin rounded-full border border-gray-300 border-t-gray-600 dark:border-gray-600 dark:border-t-gray-300" />
        ) : null}
      </div>
    </div>
  );

  // Render into the top bar slot via portal
  if (slotEl) {
    return createPortal(content, slotEl);
  }

  // Fallback: render inline (shouldn't normally happen)
  return (
    <div className="flex h-10 shrink-0 items-center border-b px-4">
      {content}
    </div>
  );
}
