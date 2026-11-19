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

import {useEffect, useState} from 'react';
import {useBackend} from '../../backend';

type State =
  | {status: 'loading'}
  | {status: 'error'; message: string}
  | {status: 'loaded'; content: string | null};

export default function OutputTab({buildUuid}: {buildUuid: string}) {
  const {fetchLogText} = useBackend();
  const [state, setState] = useState<State>({status: 'loading'});

  useEffect(() => {
    if (!fetchLogText) {
      setState({status: 'loaded', content: null});
      return;
    }
    let cancelled = false;
    setState({status: 'loading'});
    fetchLogText(buildUuid, 'simpleconsole')
      .then(content => {
        if (!cancelled) setState({status: 'loaded', content});
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setState({
          status: 'error',
          message: e instanceof Error ? e.message : 'Failed to load output',
        });
      });
    return () => {
      cancelled = true;
    };
  }, [buildUuid, fetchLogText]);

  if (state.status === 'loading') {
    return (
      <div className="text-muted-foreground flex items-center justify-center py-20">
        <p className="text-sm">Loading output...</p>
      </div>
    );
  }

  if (state.status === 'error') {
    return (
      <div className="text-muted-foreground rounded border border-dashed p-12 text-center">
        <p className="text-lg font-medium">Output unavailable</p>
        <p className="mt-1 text-sm">{state.message}</p>
      </div>
    );
  }

  if (!state.content) {
    return (
      <div className="text-muted-foreground rounded border border-dashed p-12 text-center">
        <p className="text-lg font-medium">No output</p>
        <p className="mt-1 text-sm">
          No console replay is available for this invocation.
        </p>
      </div>
    );
  }

  return (
    <pre className="overflow-auto rounded bg-amber-50 p-3 font-mono text-xs leading-relaxed dark:bg-amber-950">
      {state.content}
    </pre>
  );
}
