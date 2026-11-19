/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

import type {ComponentProps} from 'react';
import {cn} from './cn';

export type BadgeVariant = 'default' | 'secondary' | 'destructive' | 'outline';

const VARIANT_CLASSES: Record<BadgeVariant, string> = {
  default: 'border-transparent bg-primary text-primary-foreground',
  secondary: 'border-transparent bg-secondary text-secondary-foreground',
  destructive:
    'border-transparent bg-destructive text-primary-foreground dark:bg-destructive/60',
  outline: 'text-foreground',
};

export function Badge({
  className,
  variant = 'default',
  ...props
}: ComponentProps<'span'> & {variant?: BadgeVariant}) {
  return (
    <span
      data-slot="badge"
      className={cn(
        'inline-flex w-fit shrink-0 items-center justify-center gap-1 overflow-hidden rounded-md border px-2 py-0.5 text-xs font-medium whitespace-nowrap [&>svg]:pointer-events-none [&>svg]:size-3',
        VARIANT_CLASSES[variant],
        className,
      )}
      {...props}
    />
  );
}
