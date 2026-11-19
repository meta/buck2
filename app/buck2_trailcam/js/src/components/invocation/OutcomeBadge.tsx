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

import {Badge} from '../../ui';

const outcomeStyles: Record<
  string,
  {variant: 'default' | 'destructive' | 'secondary' | 'outline'; label: string}
> = {
  SUCCESS: {variant: 'default', label: 'Success'},
  FAILURE: {variant: 'destructive', label: 'Failure'},
  RUNNING: {variant: 'secondary', label: 'Running'},
  CANCELED: {variant: 'outline', label: 'Canceled'},
  CRASHED: {variant: 'destructive', label: 'Crashed'},
  UNKNOWN: {variant: 'outline', label: 'Unknown'},
};

export default function OutcomeBadge({
  outcome,
}: {
  outcome: string | null | undefined;
}) {
  const style = outcomeStyles[outcome ?? 'UNKNOWN'] ?? outcomeStyles.UNKNOWN;
  return <Badge variant={style.variant}>{style.label}</Badge>;
}
