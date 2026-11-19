/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

import {twMerge, type ClassNameValue} from 'tailwind-merge';

/**
 * Joins class names, dropping falsy entries, and resolves conflicting Tailwind
 * utilities so that a caller's `className` overrides a component's defaults.
 */
export function cn(...inputs: ClassNameValue[]): string {
  return twMerge(...inputs);
}
