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

import {useState, type ComponentProps} from 'react';
import {cn} from './cn';

export function Switch({
  checked,
  defaultChecked,
  onCheckedChange,
  className,
  onClick,
  ...props
}: Omit<ComponentProps<'button'>, 'onChange'> & {
  checked?: boolean;
  defaultChecked?: boolean;
  onCheckedChange?: (checked: boolean) => void;
}) {
  const [internal, setInternal] = useState(defaultChecked ?? false);
  const isChecked = checked ?? internal;
  return (
    <button
      type="button"
      role="switch"
      aria-checked={isChecked}
      data-state={isChecked ? 'checked' : 'unchecked'}
      data-slot="switch"
      onClick={e => {
        onClick?.(e);
        if (e.defaultPrevented) return;
        const next = !isChecked;
        if (checked === undefined) setInternal(next);
        onCheckedChange?.(next);
      }}
      className={cn(
        'relative inline-flex h-[22px] w-9 shrink-0 cursor-pointer items-center rounded-full border-0 p-0 outline-none transition-colors duration-200 focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50',
        isChecked ? 'bg-primary' : 'bg-[var(--switch-track)]',
        className,
      )}
      {...props}>
      <span
        data-slot="switch-thumb"
        className="pointer-events-none block size-[17px] rounded-full bg-white shadow-[0_2px_6px_rgba(0,0,0,0.2)] transition-transform duration-200"
        style={{
          transform: isChecked ? 'translateX(16.5px)' : 'translateX(2.5px)',
        }}
      />
    </button>
  );
}
