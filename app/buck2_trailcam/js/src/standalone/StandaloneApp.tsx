/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

import {useEffect, useState} from 'react';
import {Monitor, Moon, Sun} from 'lucide-react';
import {BackendProvider} from '../backend';
import type {InvocationInfo} from '../invocation';
import InvocationView from '../components/invocation/InvocationView';
import {TOP_BAR_SLOT_ID} from '../components/invocation/InvocationBar';
import {useTheme, type Theme} from '../lib/theme';
import {localBackend} from './localBackend';

type Load =
  | {status: 'loading'}
  | {status: 'error'; message: string}
  | {status: 'loaded'; info: InvocationInfo};

const THEME_CYCLE: Record<Theme, Theme> = {
  light: 'dark',
  dark: 'auto',
  auto: 'light',
};
const THEME_ICON = {light: Sun, dark: Moon, auto: Monitor};

function ThemeButton() {
  const {theme, setTheme} = useTheme();
  const Icon = THEME_ICON[theme];
  return (
    <button
      type="button"
      onClick={() => setTheme(THEME_CYCLE[theme])}
      title={`Theme: ${theme}`}
      aria-label="Cycle theme"
      className="text-muted-foreground hover:text-foreground rounded p-1.5 transition-colors">
      <Icon className="size-4" />
    </button>
  );
}

/** The page chrome for the standalone bundle: header, slot, viewer. */
export default function StandaloneApp() {
  const [load, setLoad] = useState<Load>({status: 'loading'});

  useEffect(() => {
    let cancelled = false;
    fetch('/api/invocation')
      .then(async res => {
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        return (await res.json()) as InvocationInfo;
      })
      .then(
        info => {
          if (!cancelled) setLoad({status: 'loaded', info});
        },
        (e: unknown) => {
          if (!cancelled) {
            setLoad({
              status: 'error',
              message: e instanceof Error ? e.message : String(e),
            });
          }
        },
      );
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <BackendProvider backend={localBackend}>
      <div className="flex h-screen flex-col overflow-hidden">
        <header className="flex h-10 shrink-0 items-center border-b">
          <div className="flex h-full shrink-0 items-center px-4">
            <span className="text-sm font-semibold tracking-tight">
              Trailcam
            </span>
          </div>
          <div
            id={TOP_BAR_SLOT_ID}
            className="flex min-w-0 flex-1 items-center px-6"
          />
          <div className="shrink-0 px-2">
            <ThemeButton />
          </div>
        </header>
        <div className="flex min-h-0 flex-1 flex-col">
          {load.status === 'loading' && (
            <p className="text-muted-foreground p-6 text-sm">
              Loading invocation...
            </p>
          )}
          {load.status === 'error' && (
            <p className="p-6 text-sm text-red-600 dark:text-red-400">
              Could not load the invocation: {load.message}
            </p>
          )}
          {load.status === 'loaded' && <InvocationView info={load.info} />}
        </div>
      </div>
    </BackendProvider>
  );
}
