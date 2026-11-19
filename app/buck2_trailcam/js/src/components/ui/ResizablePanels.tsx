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

import {useState, useCallback, useRef, useEffect} from 'react';

interface ResizablePanelsProps {
  left: React.ReactNode;
  right: React.ReactNode;
  defaultLeftWidth?: number;
  minLeftWidth?: number;
  maxLeftWidth?: number;
}

export default function ResizablePanels({
  left,
  right,
  defaultLeftWidth = 320,
  minLeftWidth = 200,
  maxLeftWidth = 600,
}: ResizablePanelsProps) {
  const [leftWidth, setLeftWidth] = useState(defaultLeftWidth);
  const dragging = useRef(false);
  const containerRef = useRef<HTMLDivElement>(null);

  const onMouseDown = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    dragging.current = true;
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
  }, []);

  useEffect(() => {
    const onMouseMove = (e: MouseEvent) => {
      if (!dragging.current || !containerRef.current) return;
      const rect = containerRef.current.getBoundingClientRect();
      const x = e.clientX - rect.left;
      setLeftWidth(Math.min(maxLeftWidth, Math.max(minLeftWidth, x)));
    };

    const onMouseUp = () => {
      if (!dragging.current) return;
      dragging.current = false;
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    };

    document.addEventListener('mousemove', onMouseMove);
    document.addEventListener('mouseup', onMouseUp);
    return () => {
      document.removeEventListener('mousemove', onMouseMove);
      document.removeEventListener('mouseup', onMouseUp);
    };
  }, [minLeftWidth, maxLeftWidth]);

  // Publish sidebar width as a CSS custom property so the top bar can align
  useEffect(() => {
    document.documentElement.style.setProperty(
      '--sidebar-width',
      `${leftWidth}px`,
    );
  }, [leftWidth]);

  return (
    <div ref={containerRef} className="flex h-full">
      <div className="shrink-0 overflow-y-auto" style={{width: leftWidth}}>
        {left}
      </div>
      <div
        onMouseDown={onMouseDown}
        className="w-1 shrink-0 cursor-col-resize border-l border-r border-gray-200 bg-gray-100 transition-colors hover:bg-blue-300 active:bg-blue-400 dark:border-gray-700 dark:bg-gray-800 dark:hover:bg-blue-700 dark:active:bg-blue-600"
      />
      <div className="min-w-0 flex-1 overflow-hidden">{right}</div>
    </div>
  );
}
