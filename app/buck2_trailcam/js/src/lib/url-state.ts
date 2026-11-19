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

// Pub/sub so multiple useUrlState subscribers stay in sync when one updates.
const listeners = new Set<() => void>();

function notify() {
  for (const fn of listeners) fn();
}

function getParam(key: string): string | null {
  if (typeof window === 'undefined') return null;
  const params = new URLSearchParams(window.location.search);
  return params.get(key);
}

function getParamAll(key: string): string[] {
  if (typeof window === 'undefined') return [];
  const params = new URLSearchParams(window.location.search);
  return params.getAll(key);
}

function writeParams(mutate: (params: URLSearchParams) => void) {
  if (typeof window === 'undefined') return;
  const params = new URLSearchParams(window.location.search);
  mutate(params);
  const qs = params.toString();
  const url = `${window.location.pathname}${qs ? `?${qs}` : ''}${window.location.hash}`;
  window.history.replaceState(null, '', url);
  notify();
}

export interface UseUrlStateOptions<T> {
  parse?: (raw: string) => T;
  stringify?: (value: T) => string;
  // If true, the param is omitted when value equals defaultValue (default true)
  omitDefault?: boolean;
}

/**
 * URL-backed state. Reads from `window.location.search` on mount and updates
 * the URL via `replaceState` so navigation does not push history entries or
 * trigger Suspense remounts. Multiple components reading the same key stay in
 * sync via an internal pub/sub.
 */
export function useUrlState<T>(
  key: string,
  defaultValue: T,
  options: UseUrlStateOptions<T> = {},
): [T, (next: T) => void] {
  const parse = options.parse ?? ((s: string) => s as unknown as T);
  const stringify = options.stringify ?? ((v: T) => String(v));
  const omitDefault = options.omitDefault ?? true;

  const read = useCallback((): T => {
    const raw = getParam(key);
    if (raw == null) return defaultValue;
    try {
      return parse(raw);
    } catch {
      return defaultValue;
    }
  }, [key, defaultValue, parse]);

  const [value, setValue] = useState<T>(defaultValue);

  // Read initial value from URL on mount, and subscribe to external updates.
  useEffect(() => {
    setValue(read());
    const fn = () => setValue(read());
    listeners.add(fn);
    return () => {
      listeners.delete(fn);
    };
  }, [read]);

  const set = useCallback(
    (next: T) => {
      setValue(next);
      writeParams(params => {
        const isDefault = omitDefault && Object.is(next, defaultValue);
        if (isDefault) {
          params.delete(key);
        } else {
          params.set(key, stringify(next));
        }
      });
    },
    [key, defaultValue, stringify, omitDefault],
  );

  return [value, set];
}

/**
 * URL-backed state for a list of strings encoded as repeated query params
 * (e.g. ?f=foo&f=bar). Returns the current array and a setter.
 */
export function useUrlStateList(
  key: string,
  defaultValue: readonly string[] = [],
): [string[], (next: readonly string[]) => void] {
  const read = useCallback((): string[] => {
    const all = getParamAll(key);
    return all.length > 0 ? all : [...defaultValue];
  }, [key, defaultValue]);

  const [value, setValue] = useState<string[]>(() => [...defaultValue]);

  useEffect(() => {
    setValue(read());
    const fn = () => setValue(read());
    listeners.add(fn);
    return () => {
      listeners.delete(fn);
    };
  }, [read]);

  const set = useCallback(
    (next: readonly string[]) => {
      setValue([...next]);
      writeParams(params => {
        params.delete(key);
        for (const item of next) params.append(key, item);
      });
    },
    [key],
  );

  return [value, set];
}

/** Wipe all URL query params. Use when navigating to a different invocation. */
export function clearUrlState() {
  if (typeof window === 'undefined') return;
  const url = `${window.location.pathname}${window.location.hash}`;
  window.history.replaceState(null, '', url);
  notify();
}

/** Read all repeated values of a URL param (non-reactive). */
export function readUrlList(key: string): string[] {
  return getParamAll(key);
}

/** Write a list of values to a URL param as repeated entries (non-reactive). */
export function writeUrlList(key: string, values: readonly string[]) {
  writeParams(params => {
    params.delete(key);
    for (const v of values) params.append(key, v);
  });
}
