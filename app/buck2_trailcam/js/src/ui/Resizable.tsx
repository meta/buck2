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

import {
  Children,
  cloneElement,
  isValidElement,
  useMemo,
  useRef,
  useState,
  type ComponentProps,
  type PointerEvent as ReactPointerEvent,
  type ReactElement,
} from 'react';
import {cn} from './cn';

type Direction = 'horizontal' | 'vertical';

interface PanelProps extends ComponentProps<'div'> {
  /** Initial share of the group in percent. Shares are normalized to sum to 100. */
  defaultSize?: number;
  /** Smallest share the panel can be dragged down to, in percent. */
  minSize?: number;
}

interface HandleProps extends Omit<ComponentProps<'div'>, 'onPointerDown'> {
  /** Draw a visible grip in the middle of the handle. */
  withHandle?: boolean;
}

// Injected by ResizablePanelGroup when it clones its children.
interface InjectedPanelProps {
  __share?: number;
}
interface InjectedHandleProps {
  __direction?: Direction;
  __onPointerDown?: (e: ReactPointerEvent<HTMLDivElement>) => void;
}

function normalize(shares: number[]): number[] {
  const total = shares.reduce((a, b) => a + b, 0) || 1;
  return shares.map(s => (s / total) * 100);
}

/**
 * Lays out `ResizablePanel` children along one axis, separated by
 * `ResizableHandle`s the user can drag. Direct children only.
 */
export function ResizablePanelGroup({
  direction = 'horizontal',
  className,
  children,
  ...props
}: ComponentProps<'div'> & {direction?: Direction}) {
  const items = Children.toArray(children).filter(
    isValidElement,
  ) as ReactElement[];
  const panels = items.filter(
    el => el.type === ResizablePanel,
  ) as ReactElement<PanelProps>[];
  const defaultKey = panels.map(p => p.props.defaultSize ?? '').join(',');
  const defaults = useMemo(
    () =>
      normalize(panels.map(p => p.props.defaultSize ?? 100 / panels.length)),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [defaultKey, panels.length],
  );
  const [shares, setShares] = useState<number[] | null>(null);
  const current = shares?.length === panels.length ? shares : defaults;
  const containerRef = useRef<HTMLDivElement>(null);

  const startDrag = (handle: number, e: ReactPointerEvent<HTMLDivElement>) => {
    const container = containerRef.current;
    if (!container || handle < 0 || handle + 1 >= panels.length) return;
    e.preventDefault();
    const horizontal = direction === 'horizontal';
    const total = horizontal ? container.clientWidth : container.clientHeight;
    const startPos = horizontal ? e.clientX : e.clientY;
    const start = current;
    const minA = panels[handle].props.minSize ?? 0;
    const minB = panels[handle + 1].props.minSize ?? 0;
    const onMove = (ev: PointerEvent) => {
      const pos = horizontal ? ev.clientX : ev.clientY;
      const a = start[handle];
      const b = start[handle + 1];
      const delta = Math.max(
        minA - a,
        Math.min(b - minB, ((pos - startPos) / total) * 100),
      );
      const next = start.slice();
      next[handle] = a + delta;
      next[handle + 1] = b - delta;
      setShares(next);
    };
    const onUp = () => {
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    };
    document.body.style.cursor = horizontal ? 'col-resize' : 'row-resize';
    document.body.style.userSelect = 'none';
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
  };

  let panelIndex = 0;
  const rendered = items.map(el => {
    if (el.type === ResizablePanel) {
      const idx = panelIndex++;
      return cloneElement(el as ReactElement<InjectedPanelProps>, {
        key: el.key ?? `panel-${idx}`,
        __share: current[idx],
      });
    }
    if (el.type === ResizableHandle) {
      const idx = panelIndex - 1;
      return cloneElement(el as ReactElement<InjectedHandleProps>, {
        key: el.key ?? `handle-${idx}`,
        __direction: direction,
        __onPointerDown: e => startDrag(idx, e),
      });
    }
    return el;
  });

  return (
    <div
      ref={containerRef}
      data-slot="resizable-panel-group"
      data-panel-group-direction={direction}
      className={cn(
        'flex h-full w-full',
        direction === 'vertical' && 'flex-col',
        className,
      )}
      {...props}>
      {rendered}
    </div>
  );
}

export function ResizablePanel({
  defaultSize: _defaultSize,
  minSize: _minSize,
  __share,
  className,
  style,
  ...props
}: PanelProps & InjectedPanelProps) {
  return (
    <div
      data-slot="resizable-panel"
      className={cn('min-h-0 min-w-0 overflow-hidden', className)}
      style={{flex: `${__share ?? 1} 1 0px`, ...style}}
      {...props}
    />
  );
}

export function ResizableHandle({
  withHandle = false,
  className,
  __direction = 'horizontal',
  __onPointerDown,
  ...props
}: HandleProps & InjectedHandleProps) {
  const horizontal = __direction === 'horizontal';
  return (
    <div
      role="separator"
      aria-orientation={horizontal ? 'vertical' : 'horizontal'}
      data-slot="resizable-handle"
      data-panel-group-direction={__direction}
      onPointerDown={__onPointerDown}
      className={cn(
        'group relative flex shrink-0 items-center justify-center bg-transparent outline-none',
        horizontal
          ? '-mx-4 w-8 cursor-col-resize'
          : '-my-4 h-8 w-full cursor-row-resize',
        className,
      )}
      {...props}>
      {withHandle && (
        <div
          className={cn(
            'pointer-events-none rounded-full bg-muted-foreground/10 transition-colors group-hover:bg-muted-foreground/40 group-active:bg-muted-foreground/70',
            horizontal ? 'h-10 w-1' : 'h-1 w-10',
          )}
        />
      )}
    </div>
  );
}
