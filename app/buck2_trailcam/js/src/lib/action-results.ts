/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

/**
 * Helpers for the Actions tab.
 *
 * - Status classification and execution-kind labels.
 * - Repro-command construction ported from
 *   fbcode/buck2/app/buck2_event_observer/src/what_ran.rs::command_to_string.
 * - Detail flattening that pulls the useful fields out of the on-demand-
 *   decoded `data.spanEnd.actionExecution` proto for the expanded row UI.
 *
 * No React; nothing here touches the DOM.
 */

// ============================================================================
// Status / kind classification (compact-row & filtering)
// ============================================================================

export type ActionStatus = 'success' | 'failed' | 'cached' | 'unknown';

/** Classify an action's status from the columnar EventSummary fields. */
export function classifyStatus(
  failed: boolean | undefined,
  executionKind: string | undefined,
): ActionStatus {
  if (failed === true) return 'failed';
  // Cache hits surface as ACTION_CACHE / REMOTE_DEP_FILE_CACHE / LOCAL_DEP_FILE_CACHE.
  // We treat all of those as the user-visible "cached" bucket so the default
  // "ran" filter can exclude them cleanly.
  if (executionKind && /CACHE|DEP_FILE/i.test(executionKind)) return 'cached';
  if (failed === false) return 'success';
  return 'unknown';
}

/**
 * Short, friendly label for the executionKind enum value.
 * Strips buck2's `ACTION_EXECUTION_KIND_` prefix and lowercases the rest:
 * e.g. `ACTION_EXECUTION_KIND_REMOTE` → "remote".
 */
export function executionKindLabel(kind: string | undefined): string {
  if (!kind) return 'unknown';
  return kind
    .replace(/^ACTION_EXECUTION_KIND_/, '')
    .toLowerCase()
    .replace(/_/g, ' ');
}

// ============================================================================
// Detail flattening (expanded-row UI)
// ============================================================================

export interface ActionDetail {
  /** spanEnd.actionExecution.failed */
  failed: boolean;
  /** Selected last-command exit code, if any. */
  exitCode: number | null;
  /** Last-command stdout (may be very long; UI scrolls). */
  stdout: string;
  /** Last-command stderr. */
  stderr: string;
  /** Pre-formatted hint set by buck2 when available. */
  additionalMessage: string | null;
  /** Action digest if any command had one (local/remote/cache). */
  actionDigest: string | null;
  /** A reproduction recipe derived from the last command, if possible. */
  repro: ReproCommand;
  /** Execution kind label as enum string ("LOCAL", "REMOTE", ...). */
  executionKind: string | null;
  /** Wall time in ms (real time spent) — distinct from action duration. */
  wallTimeMs: number | null;
  /** Total size of declared outputs in bytes. */
  outputSizeBytes: number | null;
  /** Hostname the command ran on, when present. */
  hostname: string | null;
  /**
   * Fully-configured target label including the cfg hash, e.g.
   * "fbcode//foo:bar (cfg:dev-linux-x86_64-fbcode-abcdef)". Falls back
   * to the unconfigured label when the configuration is absent.
   */
  configuredTargetLabel: string | null;
  /** Action category, e.g. "cxx_compile". */
  category: string | null;
  /** Action identifier (free-form, e.g. "src/foo.cpp"). */
  identifier: string | null;
}

export type ReproCommand =
  /** A runnable shell command — local exec, worker, or `frecli` for remote. */
  | {kind: 'shell'; command: string}
  /** Action had no commands (cancelled, error before scheduling, etc.). */
  | {kind: 'none'};

/**
 * Flatten the spanEnd.actionExecution proto into the fields the expanded UI
 * needs. Tolerates missing/unexpected shapes.
 */
export function flattenAction(
  spanEndActionExecution: Record<string, unknown> | null | undefined,
): ActionDetail {
  const ae = spanEndActionExecution ?? {};
  const commands =
    (ae.commands as Array<Record<string, unknown>> | undefined) ?? [];
  const last = commands.length > 0 ? commands[commands.length - 1] : undefined;
  const details =
    (last?.details as Record<string, unknown> | undefined) ?? undefined;

  const exitCodeVal = details?.signedExitCode;
  const exitCode =
    typeof exitCodeVal === 'number'
      ? exitCodeVal
      : typeof exitCodeVal === 'string'
        ? parseInt(exitCodeVal, 10)
        : null;

  const {label, configuredLabel, category, identifier} = extractIdentity(ae);

  return {
    failed: !!(ae.failed as boolean),
    exitCode: Number.isFinite(exitCode as number) ? (exitCode as number) : null,
    stdout: ((details?.cmdStdout as string) ?? '').toString(),
    stderr: ((details?.cmdStderr as string) ?? '').toString(),
    additionalMessage: ((details?.additionalMessage as string) ?? '') || null,
    actionDigest: extractActionDigest(commands),
    repro: extractRepro(details),
    executionKind: (ae.executionKind as string) ?? null,
    wallTimeMs: extractDurationMs(ae.wallTime),
    outputSizeBytes: ae.outputSize != null ? Number(ae.outputSize) : null,
    hostname: ((ae.hostname as string) ?? '') || null,
    configuredTargetLabel: configuredLabel ?? label ?? null,
    category: category ?? null,
    identifier: identifier ?? null,
  };
}

function extractIdentity(ae: Record<string, unknown>): {
  label?: string;
  configuredLabel?: string;
  category?: string;
  identifier?: string;
} {
  const out: {
    label?: string;
    configuredLabel?: string;
    category?: string;
    identifier?: string;
  } = {};
  const key = ae.key as Record<string, unknown> | undefined;
  const ctl = key?.targetLabel as Record<string, unknown> | undefined;
  if (ctl?.label) {
    const lbl = ctl.label as Record<string, unknown>;
    const pkg = (lbl.package as string) ?? '';
    const name = (lbl.name as string) ?? '';
    out.label = name ? `${pkg}:${name}` : pkg;
  }
  if (ctl?.configuration) {
    const cfg = ctl.configuration as Record<string, unknown>;
    const fullName = (cfg.fullName as string) ?? '';
    if (fullName && out.label) {
      out.configuredLabel = `${out.label} (${fullName})`;
    }
  }
  const name = ae.name as Record<string, unknown> | undefined;
  if (name) {
    out.category = (name.category as string) ?? undefined;
    out.identifier = (name.identifier as string) ?? undefined;
  }
  return out;
}

function extractDurationMs(d: unknown): number | null {
  if (!d || typeof d !== 'object') return null;
  const obj = d as Record<string, unknown>;
  const seconds =
    typeof obj.seconds === 'string'
      ? parseInt(obj.seconds, 10)
      : typeof obj.seconds === 'number'
        ? obj.seconds
        : 0;
  const nanos = typeof obj.nanos === 'number' ? obj.nanos : 0;
  return seconds * 1000 + nanos / 1e6;
}

function extractActionDigest(
  commands: Array<Record<string, unknown>>,
): string | null {
  for (let i = commands.length - 1; i >= 0; i--) {
    const details = commands[i]?.details as Record<string, unknown> | undefined;
    const kind = details?.commandKind as Record<string, unknown> | undefined;
    if (!kind) continue;
    const local = kind.localCommand as Record<string, unknown> | undefined;
    if (local?.actionDigest) return String(local.actionDigest);
    const remote = kind.remoteCommand as Record<string, unknown> | undefined;
    if (remote?.actionDigest) return String(remote.actionDigest);
    const omitted = kind.omittedLocalCommand as
      Record<string, unknown> | undefined;
    if (omitted?.actionDigest) return String(omitted.actionDigest);
  }
  return null;
}

// ============================================================================
// Repro-command construction (port of buck2's command_to_string)
// ============================================================================

function extractRepro(
  details: Record<string, unknown> | undefined,
): ReproCommand {
  if (!details) return {kind: 'none'};
  const kind = details.commandKind as Record<string, unknown> | undefined;
  if (!kind) {
    // Some statuses (cancelled, error before scheduling) leave commandKind unset.
    return {kind: 'none'};
  }

  const local = kind.localCommand as
    {argv?: string[]; env?: Array<{key: string; value: string}>} | undefined;
  if (local?.argv?.length) {
    return {
      kind: 'shell',
      command: commandToShellString(local.env ?? [], local.argv),
    };
  }

  const workerInit = kind.workerInitCommand as
    {argv?: string[]; env?: Array<{key: string; value: string}>} | undefined;
  if (workerInit?.argv?.length) {
    return {
      kind: 'shell',
      command: commandToShellString(workerInit.env ?? [], workerInit.argv),
    };
  }

  const worker = kind.workerCommand as
    | {
        argv?: string[];
        env?: Array<{key: string; value: string}>;
        fallbackExe?: string[];
      }
    | undefined;
  if (worker?.argv?.length || worker?.fallbackExe?.length) {
    const argv = [...(worker.fallbackExe ?? []), ...(worker.argv ?? [])];
    return {
      kind: 'shell',
      command: commandToShellString(worker.env ?? [], argv),
    };
  }

  // Remote / cache-hit / omitted local: no shell command, but we have an
  // action digest — surface it as a `frecli cas download-action` command so
  // the user can fetch the inputs/outputs and reproduce locally if needed.
  const remoteDigest =
    (kind.remoteCommand as {actionDigest?: string} | undefined)?.actionDigest ??
    (kind.omittedLocalCommand as {actionDigest?: string} | undefined)
      ?.actionDigest;
  if (remoteDigest) {
    return {
      kind: 'shell',
      command: `frecli cas download-action ${remoteDigest}`,
    };
  }

  return {kind: 'none'};
}

/**
 * Format `env` and `argv` into a shell-quoted command string in the same
 * shape buck2's CLI emits via `buck2 log what-ran`:
 *   env -C "$(buck2 root --kind project)" -- KEY=VAL ... arg1 arg2 ...
 */
function commandToShellString(
  env: Array<{key: string; value: string}>,
  argv: string[],
): string {
  const parts: string[] = ['env -C "$(buck2 root --kind project)" --'];
  for (const e of env) {
    parts.push(shellQuote(`${e.key}=${e.value}`));
  }
  for (const arg of argv) {
    parts.push(shellQuote(arg));
  }
  return parts.join(' ');
}

/**
 * POSIX-style shell quoting. Same rules as Python's shlex.quote / Rust's
 * shlex::try_quote: if the string is empty or contains anything outside the
 * safe set, wrap it in single quotes and escape any embedded single quotes.
 */
function shellQuote(s: string): string {
  if (s.length === 0) return "''";
  // The set of "always safe in a shell word" characters.
  if (/^[A-Za-z0-9@%+=:,./_-]+$/.test(s)) return s;
  return `'${s.replace(/'/g, `'\\''`)}'`;
}
