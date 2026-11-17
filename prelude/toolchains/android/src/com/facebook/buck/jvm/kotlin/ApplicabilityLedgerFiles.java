/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.infer.annotation.Nullsafe;
import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.ArrayList;
import java.util.List;

/**
 * The Kosabi applicability plugin's cross-file ledger in an action's incremental state: per-file
 * facts and suppressed findings that cross-file exemptions read for the files an incremental round
 * does not recompile. The plugin writes each round's ledger as pending; it becomes the committed
 * ledger only when the action succeeds. The file names match the plugin's {@code
 * ApplicabilityLedger}.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public final class ApplicabilityLedgerFiles {
  static final String COMMITTED_FILE = "applicability_ledger.bin";
  static final String PENDING_FILE = "applicability_ledger.pending.bin";
  static final String SOURCE_LIST_FILE = "applicability_sources.txt";

  private ApplicabilityLedgerFiles() {}

  /**
   * Removes the pending ledger of an earlier failed round, so that a later round that succeeds
   * without writing its own cannot promote it, and says whether a committed ledger exists.
   */
  public static boolean discardPendingAndCheckCommitted(AbsPath incrementalStateDir)
      throws IOException {
    Files.deleteIfExists(incrementalStateDir.resolve(PENDING_FILE).getPath());
    // An unreadable ledger would leave this round without the other files' entries.
    Path committed = incrementalStateDir.resolve(COMMITTED_FILE).getPath();
    return Files.isRegularFile(committed) && Files.isReadable(committed);
  }

  /**
   * Lists the round's Kotlin sources, relative to the project root, for the plugin: it keeps a
   * ledger entry only for a file that is still a source, since a file dropped from the target may
   * stay on disk.
   */
  public static void writeSourceList(AbsPath incrementalStateDir, Iterable<RelPath> sources)
      throws IOException {
    List<String> lines = new ArrayList<>();
    for (RelPath source : sources) {
      lines.add(source.normalize().toString().replace(File.separatorChar, '/'));
    }
    Path directory = incrementalStateDir.getPath();
    Files.createDirectories(directory);
    // Complete or absent: a partial list would drop the entries of files that are still sources.
    Path temporary = Files.createTempFile(directory, SOURCE_LIST_FILE, ".tmp");
    try {
      Files.write(temporary, lines, StandardCharsets.UTF_8);
      Files.move(
          temporary,
          directory.resolve(SOURCE_LIST_FILE),
          StandardCopyOption.REPLACE_EXISTING,
          StandardCopyOption.ATOMIC_MOVE);
    } finally {
      Files.deleteIfExists(temporary);
    }
  }

  /** Removes the source list, so that a round without one never reads an earlier round's. */
  public static void deleteSourceList(AbsPath incrementalStateDir) throws IOException {
    Files.deleteIfExists(incrementalStateDir.resolve(SOURCE_LIST_FILE).getPath());
  }

  /** Makes the pending ledger the committed one: call only once the action succeeded. */
  public static void promotePending(Path incrementalStateDir) throws IOException {
    Path pending = incrementalStateDir.resolve(PENDING_FILE);
    if (Files.exists(pending)) {
      Files.move(
          pending,
          incrementalStateDir.resolve(COMMITTED_FILE),
          StandardCopyOption.REPLACE_EXISTING);
    }
  }
}
