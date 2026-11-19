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

import {FilterIcon} from 'lucide-react';

export default function FilterableValue({
  path,
  value,
  isActive,
  onFilter,
}: {
  path: string;
  value: unknown;
  isActive: boolean;
  onFilter: (path: string, value: string) => void;
}) {
  const strVal = String(value);
  const displayVal =
    strVal.length > 200 ? strVal.slice(0, 200) + '...' : strVal;

  const colorClass =
    typeof value === 'string'
      ? 'text-green-600 dark:text-green-400'
      : typeof value === 'number'
        ? 'text-blue-600 dark:text-blue-400'
        : typeof value === 'boolean'
          ? 'text-purple-600 dark:text-purple-400'
          : value === null
            ? 'text-gray-400 italic'
            : '';

  return (
    <span
      className={`group/fv inline-flex items-center gap-1 cursor-pointer rounded px-0.5 hover:bg-blue-50 dark:hover:bg-blue-950 ${
        isActive ? 'bg-blue-100 dark:bg-blue-900' : ''
      } ${colorClass}`}
      onClick={e => {
        e.stopPropagation();
        onFilter(path, strVal);
      }}
      title={`Filter: ${path} = ${strVal}`}>
      <span>{value === null ? 'null' : displayVal}</span>
      <FilterIcon className="size-3 opacity-0 group-hover/fv:opacity-50 shrink-0" />
    </span>
  );
}
