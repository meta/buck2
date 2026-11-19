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

import {useCallback, useEffect, useState} from 'react';

export type Theme = 'light' | 'dark' | 'auto';

const STORAGE_KEY = 'trailcam:theme';

/**
 * Pre-paint script — must run inline in `<head>` before React hydrates so the
 * page doesn't flash light theme before switching to dark. Reads the same
 * STORAGE_KEY this hook writes to, applies the same fallback to the OS
 * `prefers-color-scheme` setting when the value is missing or 'auto'.
 *
 * Lives here as an exported string so layout.tsx can render it via
 * `dangerouslySetInnerHTML` next to the rest of the theme code.
 */
export const THEME_INIT_SCRIPT = `(function() {
  try {
    var p = localStorage.getItem('${STORAGE_KEY}');
    var dark = p === 'dark' || (p !== 'light' && matchMedia('(prefers-color-scheme: dark)').matches);
    if (dark) document.documentElement.classList.add('dark');
  } catch (e) {}
})();`;

function readStored(): Theme {
  if (typeof window === 'undefined') return 'auto';
  const v = window.localStorage.getItem(STORAGE_KEY);
  return v === 'light' || v === 'dark' ? v : 'auto';
}

function applyTheme(t: Theme): void {
  const dark =
    t === 'dark' ||
    (t === 'auto' && window.matchMedia('(prefers-color-scheme: dark)').matches);
  document.documentElement.classList.toggle('dark', dark);
}

/**
 * Applies the stored preference to `<html>` immediately. Hosts that render on
 * the client call this before mounting; server-rendering hosts inline
 * `THEME_INIT_SCRIPT` instead, which does the same thing before hydration.
 */
export function applyStoredTheme(): void {
  applyTheme(readStored());
}

/**
 * Theme controller. Returns the current theme preference and a setter that
 * persists to localStorage and applies the `.dark` class to `<html>` on the
 * fly. While in 'auto' mode, listens for OS color-scheme changes and updates
 * the class accordingly.
 */
export function useTheme(): {theme: Theme; setTheme: (t: Theme) => void} {
  const [theme, setThemeState] = useState<Theme>(() =>
    typeof window === 'undefined' ? 'auto' : readStored(),
  );

  useEffect(() => {
    applyTheme(theme);
    if (theme !== 'auto') return;
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    const onChange = () => applyTheme('auto');
    mq.addEventListener('change', onChange);
    return () => mq.removeEventListener('change', onChange);
  }, [theme]);

  const setTheme = useCallback((t: Theme) => {
    if (t === 'auto') window.localStorage.removeItem(STORAGE_KEY);
    else window.localStorage.setItem(STORAGE_KEY, t);
    setThemeState(t);
  }, []);

  return {theme, setTheme};
}
