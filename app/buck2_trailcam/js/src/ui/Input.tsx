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

/**
 * Single-line text input. `className` applies to the outer pill so callers
 * can size it (e.g. `h-7 text-xs`); the inner `<input>` inherits font size.
 */
export function Input({className, ...props}: ComponentProps<'input'>) {
  return (
    <div
      data-slot="input"
      className={cn(
        'flex h-11 items-center gap-1.5 rounded-[16px] bg-secondary px-3 text-sm transition-all duration-200 has-[:focus]:bg-background has-[:focus]:ring-1 has-[:focus]:ring-ring has-[:disabled]:cursor-not-allowed has-[:disabled]:opacity-50',
        className,
      )}>
      <input
        className="w-full min-w-0 flex-1 border-0 bg-transparent text-foreground outline-none placeholder:text-muted-foreground disabled:pointer-events-none disabled:cursor-not-allowed"
        {...props}
      />
    </div>
  );
}
