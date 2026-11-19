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
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useRef,
  useState,
  type ComponentProps,
  type CSSProperties,
  type KeyboardEvent,
} from 'react';
import {cn} from './cn';

interface TabsContextValue {
  value: string;
  setValue: (value: string) => void;
  baseId: string;
}

const TabsContext = createContext<TabsContextValue | null>(null);

function useTabsContext(component: string): TabsContextValue {
  const ctx = useContext(TabsContext);
  if (ctx == null) {
    throw new Error(`<${component}> must be rendered inside <Tabs>`);
  }
  return ctx;
}

/** Tab set. Controlled with `value`/`onValueChange`, or uncontrolled with `defaultValue`. */
export function Tabs({
  value,
  defaultValue,
  onValueChange,
  className,
  children,
  ...props
}: Omit<ComponentProps<'div'>, 'defaultValue'> & {
  value?: string;
  defaultValue?: string;
  onValueChange?: (value: string) => void;
}) {
  const [internal, setInternal] = useState(defaultValue ?? '');
  const current = value ?? internal;
  const setValue = useCallback(
    (next: string) => {
      if (value === undefined) setInternal(next);
      onValueChange?.(next);
    },
    [value, onValueChange],
  );
  const baseId = useId();
  return (
    <TabsContext.Provider value={{value: current, setValue, baseId}}>
      <div
        data-slot="tabs"
        className={cn('flex flex-col gap-2', className)}
        {...props}>
        {children}
      </div>
    </TabsContext.Provider>
  );
}

// Inset of the sliding pill from the list's edge; must match the list padding.
const PILL_INSET_PX = 3;

export function TabsList({
  className,
  children,
  ...props
}: ComponentProps<'div'>) {
  const {value} = useTabsContext('TabsList');
  const listRef = useRef<HTMLDivElement>(null);
  const [pillStyle, setPillStyle] = useState<CSSProperties>({opacity: 0});

  // Position the pill under the active trigger; re-measure on resize.
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const update = () => {
      const active = list.querySelector<HTMLElement>(
        '[role="tab"][data-state="active"]',
      );
      if (!active) {
        setPillStyle({opacity: 0});
        return;
      }
      setPillStyle({
        transform: `translateX(${active.offsetLeft - PILL_INSET_PX}px)`,
        width: active.offsetWidth,
        height: active.offsetHeight,
        opacity: 1,
      });
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(list);
    return () => observer.disconnect();
  }, [value, children]);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const tabs = Array.from(
      listRef.current?.querySelectorAll<HTMLButtonElement>(
        '[role="tab"]:not(:disabled)',
      ) ?? [],
    );
    const i = tabs.indexOf(document.activeElement as HTMLButtonElement);
    if (i < 0) return;
    let next: number;
    switch (e.key) {
      case 'ArrowRight':
        next = (i + 1) % tabs.length;
        break;
      case 'ArrowLeft':
        next = (i - 1 + tabs.length) % tabs.length;
        break;
      case 'Home':
        next = 0;
        break;
      case 'End':
        next = tabs.length - 1;
        break;
      default:
        return;
    }
    e.preventDefault();
    tabs[next].focus();
    tabs[next].click();
  };

  return (
    <div
      ref={listRef}
      role="tablist"
      data-slot="tabs-list"
      onKeyDown={onKeyDown}
      className={cn(
        'relative inline-flex h-9 w-fit items-center justify-center rounded-full bg-secondary p-[3px] text-muted-foreground',
        className,
      )}
      {...props}>
      <div
        aria-hidden
        className="pointer-events-none absolute rounded-full bg-background shadow-sm transition-all duration-200 ease-out"
        style={{left: PILL_INSET_PX, top: PILL_INSET_PX, ...pillStyle}}
      />
      {children}
    </div>
  );
}

export function TabsTrigger({
  value,
  className,
  onClick,
  ...props
}: ComponentProps<'button'> & {value: string}) {
  const ctx = useTabsContext('TabsTrigger');
  const active = ctx.value === value;
  return (
    <button
      type="button"
      role="tab"
      id={`${ctx.baseId}-tab-${value}`}
      aria-selected={active}
      aria-controls={`${ctx.baseId}-panel-${value}`}
      tabIndex={active ? 0 : -1}
      data-state={active ? 'active' : 'inactive'}
      data-slot="tabs-trigger"
      onClick={e => {
        onClick?.(e);
        if (!e.defaultPrevented) ctx.setValue(value);
      }}
      className={cn(
        "relative z-10 inline-flex h-full flex-1 items-center justify-center gap-1.5 whitespace-nowrap rounded-full border border-transparent px-5 py-1 text-sm font-normal text-foreground transition-[font-weight] duration-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 data-[state=active]:font-medium [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
        className,
      )}
      {...props}
    />
  );
}

/** Panel for one tab. Only the active panel is mounted. */
export function TabsContent({
  value,
  className,
  ...props
}: ComponentProps<'div'> & {value: string}) {
  const ctx = useTabsContext('TabsContent');
  if (ctx.value !== value) return null;
  return (
    <div
      role="tabpanel"
      id={`${ctx.baseId}-panel-${value}`}
      aria-labelledby={`${ctx.baseId}-tab-${value}`}
      tabIndex={0}
      data-state="active"
      data-slot="tabs-content"
      className={cn('flex-1 outline-none', className)}
      {...props}
    />
  );
}
