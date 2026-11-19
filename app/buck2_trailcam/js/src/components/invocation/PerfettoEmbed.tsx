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

import {useEffect, useRef, useState, useCallback} from 'react';

type Status = 'loading-ui' | 'fetching-trace' | 'sending' | 'ready' | 'error';

/**
 * Embeds the Perfetto UI and loads a Chrome trace into it over postMessage.
 * `perfettoUiUrl` must be frameable from the page (same origin, or a host
 * that allows it) and should select embedded mode, e.g.
 * `.../index.html#!/?mode=embedded&hideSidebar=true`.
 */
export default function PerfettoEmbed({
  traceUrl,
  perfettoUiUrl,
  title,
}: {
  traceUrl: string;
  perfettoUiUrl: string;
  title?: string;
}) {
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const [status, setStatus] = useState<Status>('loading-ui');
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const sentRef = useRef(false);

  const sendTrace = useCallback(async () => {
    const iframe = iframeRef.current;
    if (!iframe?.contentWindow || sentRef.current) return;
    sentRef.current = true;

    setStatus('fetching-trace');
    try {
      const response = await fetch(traceUrl);
      if (!response.ok) {
        throw new Error(`Failed to fetch trace: HTTP ${response.status}`);
      }
      const buffer = await response.arrayBuffer();

      setStatus('sending');
      iframe.contentWindow.postMessage(
        {
          perfetto: {
            buffer,
            title: title ?? 'Buck2 Trace',
            keepApiOpen: true,
          },
        },
        '*',
      );
      setStatus('ready');
    } catch (e) {
      setErrorMsg(e instanceof Error ? e.message : 'Failed to load trace');
      setStatus('error');
    }
  }, [traceUrl, title]);

  useEffect(() => {
    // PING/PONG handshake: poll until Perfetto UI is ready
    const iframe = iframeRef.current;
    if (!iframe) return;

    let timer: ReturnType<typeof setInterval> | null = null;
    let timeout: ReturnType<typeof setTimeout> | null = null;

    function onMessage(evt: MessageEvent) {
      if (evt.data !== 'PONG') return;
      if (timer) clearInterval(timer);
      if (timeout) clearTimeout(timeout);
      window.removeEventListener('message', onMessage);
      sendTrace();
    }

    window.addEventListener('message', onMessage);

    // Start pinging once iframe loads
    iframe.onload = () => {
      timer = setInterval(() => {
        iframe.contentWindow?.postMessage('PING', '*');
      }, 50);

      // Timeout after 10s
      timeout = setTimeout(() => {
        if (timer) clearInterval(timer);
        window.removeEventListener('message', onMessage);
        setErrorMsg('Perfetto UI failed to respond');
        setStatus('error');
      }, 10000);
    };

    return () => {
      if (timer) clearInterval(timer);
      if (timeout) clearTimeout(timeout);
      window.removeEventListener('message', onMessage);
    };
  }, [sendTrace]);

  return (
    <div className="relative h-[calc(100vh-14rem)] w-full">
      {status !== 'ready' && status !== 'error' && (
        <div className="absolute inset-0 z-10 flex items-center justify-center bg-white/80 dark:bg-gray-950/80">
          <p className="text-muted-foreground text-sm">
            {status === 'loading-ui' && 'Loading Perfetto UI...'}
            {status === 'fetching-trace' && 'Fetching trace...'}
            {status === 'sending' && 'Loading trace into viewer...'}
          </p>
        </div>
      )}
      {status === 'error' && (
        <div className="absolute inset-0 z-10 flex items-center justify-center">
          <p className="text-sm text-red-600 dark:text-red-400">{errorMsg}</p>
        </div>
      )}
      <iframe
        ref={iframeRef}
        src={perfettoUiUrl}
        className="h-full w-full border-0"
        sandbox="allow-scripts allow-same-origin"
        allow="cross-origin-isolated"
      />
    </div>
  );
}
