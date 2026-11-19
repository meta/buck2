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

import type {ReactNode} from 'react';
import {Card, CardContent, CardHeader, CardTitle} from '../../ui';
import CopyButton from '../ui/CopyButton';

/** One label/value row of the Metadata card; hosts use it for their own rows. */
export function MetaRow({
  label,
  children,
  copyText,
}: {
  label: string;
  children: ReactNode;
  copyText?: string;
}) {
  return (
    <div className="flex items-baseline justify-between gap-2 py-1.5">
      <span className="text-muted-foreground shrink-0 text-sm">{label}</span>
      <span className="flex items-center gap-1 text-right text-sm">
        {children}
        {copyText && <CopyButton text={copyText} label="Copy" />}
      </span>
    </div>
  );
}

export default function MetadataSection({
  username,
  hostname,
  client,
  buck2Revision,
  remoteExecutionId,
  children,
}: {
  username: string | null;
  hostname: string | null;
  client: string | null;
  buck2Revision: string | null;
  remoteExecutionId: string | null;
  /** Extra `MetaRow`s appended by the host. */
  children?: ReactNode;
}) {
  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-base">Metadata</CardTitle>
      </CardHeader>
      <CardContent className="space-y-1">
        <MetaRow label="User">{username ?? '—'}</MetaRow>
        <MetaRow label="Hostname" copyText={hostname ?? undefined}>
          <span
            className="max-w-[180px] truncate font-mono text-xs"
            title={hostname ?? undefined}>
            {hostname ?? '—'}
          </span>
        </MetaRow>
        <MetaRow label="Client">{client || 'None'}</MetaRow>
        <MetaRow label="Buck2 Rev" copyText={buck2Revision ?? undefined}>
          {buck2Revision ? (
            <code className="text-xs">{buck2Revision.slice(0, 10)}</code>
          ) : (
            '—'
          )}
        </MetaRow>
        <MetaRow label="RE Session" copyText={remoteExecutionId ?? undefined}>
          {remoteExecutionId ? (
            <code
              className="max-w-[180px] truncate text-xs"
              title={remoteExecutionId}>
              {remoteExecutionId.slice(0, 16)}...
            </code>
          ) : (
            'None'
          )}
        </MetaRow>
        {children}
      </CardContent>
    </Card>
  );
}
