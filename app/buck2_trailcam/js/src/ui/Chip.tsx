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

import type {ComponentProps} from 'react';
import {X} from 'lucide-react';
import {cn} from './cn';

export type ChipVariant = 'default' | 'outline' | 'filter';
export type ChipSize = 'sm' | 'default' | 'lg';

const VARIANT_CLASSES: Record<ChipVariant, string> = {
  default: 'bg-secondary text-secondary-foreground hover:bg-accent',
  outline: 'border border-border text-foreground hover:bg-accent',
  filter: 'bg-accent text-accent-foreground hover:bg-accent/80',
};

const SIZE_CLASSES: Record<ChipSize, string> = {
  sm: 'h-6 gap-1.5 rounded-md px-2 py-1 text-xs',
  default: 'h-7 gap-1.5 rounded-lg px-2.5 py-1.5 text-sm',
  lg: 'h-8 gap-1.5 rounded-lg px-3 py-2 text-sm',
};

/** A compact tag. Clickable when `onChipClick` is given; removable with `onRemove`. */
export function Chip({
  className,
  variant = 'default',
  size = 'default',
  selected = false,
  disabled = false,
  onRemove,
  onChipClick,
  children,
  ...props
}: Omit<ComponentProps<'div'>, 'onClick'> & {
  variant?: ChipVariant;
  size?: ChipSize;
  selected?: boolean;
  disabled?: boolean;
  onRemove?: () => void;
  onChipClick?: () => void;
}) {
  const clickable = onChipClick != null;
  return (
    <div
      data-slot="chip"
      role={clickable ? 'button' : undefined}
      tabIndex={clickable && !disabled ? 0 : undefined}
      onClick={onChipClick}
      onKeyDown={e => {
        if (clickable && !disabled && (e.key === 'Enter' || e.key === ' ')) {
          e.preventDefault();
          onChipClick();
        }
      }}
      className={cn(
        'inline-flex shrink-0 cursor-default select-none items-center font-medium outline-none transition-all focus-visible:ring-2 focus-visible:ring-ring',
        VARIANT_CLASSES[variant],
        SIZE_CLASSES[size],
        selected && 'bg-primary text-primary-foreground',
        clickable && 'cursor-pointer hover:opacity-80 active:opacity-60',
        disabled && 'pointer-events-none opacity-50',
        className,
      )}
      {...props}>
      {children}
      {onRemove && (
        <button
          type="button"
          aria-label="Remove"
          disabled={disabled}
          onClick={e => {
            e.stopPropagation();
            onRemove();
          }}
          className="ml-0.5 rounded-sm opacity-70 transition-opacity hover:opacity-100 focus:opacity-100 focus:outline-none focus:ring-1 focus:ring-ring">
          <X className="size-3" />
        </button>
      )}
    </div>
  );
}
