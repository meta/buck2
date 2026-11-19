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
 * Format a duration in milliseconds to a human-readable string.
 * e.g. 125400 -> "2m 5s", 3600000 -> "1h 0m 0s"
 */
export function formatDuration(ms: number | null | undefined): string {
  if (ms == null) return '—';
  if (ms < 0) return '—';

  const totalSeconds = Math.floor(ms / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;

  if (hours > 0) {
    return `${hours}h ${minutes}m ${seconds}s`;
  }
  if (minutes > 0) {
    return `${minutes}m ${seconds}s`;
  }
  if (seconds > 0) {
    return `${seconds}s`;
  }
  return `${ms}ms`;
}

/**
 * Format bytes to a human-readable string using decimal units.
 * e.g. 1500 -> "1.50 KB", 1048576 -> "1.05 MB"
 */
export function formatBytes(bytes: string | number | null | undefined): string {
  if (bytes == null) return '—';
  const n = typeof bytes === 'string' ? Number(bytes) : bytes;
  if (isNaN(n) || n === 0) return '0 B';

  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const k = 1000;
  const i = Math.floor(Math.log(Math.abs(n)) / Math.log(k));
  const idx = Math.min(i, units.length - 1);

  return `${(n / Math.pow(k, idx)).toFixed(2)} ${units[idx]}`;
}

/**
 * Format bytes per second to a human-readable speed string.
 */
export function formatSpeed(bytesPerSecond: string | null | undefined): string {
  if (bytesPerSecond == null) return '—';
  return `${formatBytes(bytesPerSecond)}/s`;
}

/**
 * Format a unix timestamp to a locale string.
 */
export function formatTimestamp(
  unixSeconds: number | null | undefined,
): string {
  if (unixSeconds == null || unixSeconds === 0) return '—';
  return new Date(unixSeconds * 1000).toLocaleString();
}

/**
 * Format a relative time from a unix timestamp.
 * e.g. "2 minutes ago", "3 hours ago"
 */
export function formatRelativeTime(
  unixSeconds: number | null | undefined,
): string {
  if (unixSeconds == null || unixSeconds === 0) return '—';
  const now = Date.now() / 1000;
  const diff = now - unixSeconds;

  if (diff < 60) return 'just now';
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  return `${Math.floor(diff / 86400)}d ago`;
}

/**
 * Format a protobuf Duration ({seconds, nanos}) to a human-readable string.
 */
export function formatProtoDuration(
  dur: {seconds?: string | number; nanos?: number} | null | undefined,
): string {
  if (!dur) return '—';
  const ms = Number(dur.seconds ?? 0) * 1000 + (dur.nanos ?? 0) / 1e6;
  return formatDuration(ms);
}

/**
 * Format cache hit rate (0.0-1.0) to a percentage string.
 */
export function formatCacheHitRate(rate: number | null | undefined): string {
  if (rate == null) return 'None';
  const pct = (rate * 100).toFixed(2);
  if (pct === '100.00' && rate !== 1) return '99.99+%';
  if (pct === '0.00' && rate !== 0) return '<0.01%';
  return `${pct}%`;
}
