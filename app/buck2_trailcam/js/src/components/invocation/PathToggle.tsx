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

import {Tabs, TabsList, TabsTrigger} from '../../ui';

export type PathMode = 'critical' | 'slowest';

/**
 * Critical / Slowest path selector. Built on the NDS Tabs primitive so it
 * shares the pill / sliding-active-state look with the page-level tabs,
 * but tinted amber to make it clear it's a sub-control on its own card
 * rather than a top-level navigation.
 */
export default function PathToggle({
  value,
  onValueChange,
  className,
}: {
  value: PathMode;
  onValueChange: (m: PathMode) => void;
  className?: string;
}) {
  return (
    <Tabs
      value={value}
      onValueChange={v => onValueChange(v as PathMode)}
      className={className}>
      <TabsList className="bg-amber-100 dark:bg-amber-900/30">
        <TabsTrigger
          value="slowest"
          className="data-[state=active]:text-amber-900 dark:data-[state=active]:text-amber-200">
          Slowest Path
        </TabsTrigger>
        <TabsTrigger
          value="critical"
          className="data-[state=active]:text-amber-900 dark:data-[state=active]:text-amber-200">
          Critical Path
        </TabsTrigger>
      </TabsList>
    </Tabs>
  );
}
