/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

// The surface hosts build on. The Nest app and the standalone entry point are
// the two callers in this repository.
export {BackendProvider, useBackend} from './backend';
export type {
  BuildHistoryQuery,
  BuildHistoryRow,
  DaemonFileChangesResponse,
  FileChangeEntry,
  LogTextKind,
  TrailcamBackend,
} from './backend';
export type {
  InvocationInfo,
  InvocationMetrics,
  RemoteExecutionStats,
} from './invocation';
export {default as InvocationView} from './components/invocation/InvocationView';
export type {
  ExtraTab,
  InvocationViewProps,
} from './components/invocation/InvocationView';
export {TOP_BAR_SLOT_ID} from './components/invocation/InvocationBar';
export {MetaRow} from './components/invocation/MetadataSection';
export {applyStoredTheme, useTheme, THEME_INIT_SCRIPT} from './lib/theme';
export type {Theme} from './lib/theme';
