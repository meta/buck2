/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.testutil;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.io.file.MostFiles;
import com.facebook.buck.util.environment.EnvVariablesProvider;
import java.io.IOException;
import java.nio.file.FileVisitResult;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.SimpleFileVisitor;
import java.nio.file.attribute.BasicFileAttributes;
import java.util.Arrays;
import org.junit.rules.ExternalResource;

/**
 * Apes the API of JUnit's <code>TemporaryFolder</code> but returns {@link Path} references and can
 * be made to not delete itself after test execution.
 */
public class TemporaryPaths extends ExternalResource {

  private static final String DEFAULT_PREFIX = "junit-temp-path";

  private final String prefix;
  private final boolean keepContents;
  private AbsPath root;

  public TemporaryPaths() {
    this("1".equals(EnvVariablesProvider.getSystemEnv().get("BUCK_TEST_KEEP_TEMPORARY_PATHS")));
  }

  public TemporaryPaths(boolean keepContents) {
    this(DEFAULT_PREFIX, keepContents);
  }

  public TemporaryPaths(String prefix) {
    this(prefix, false);
  }

  public TemporaryPaths(String prefix, boolean keepContents) {
    this.prefix = prefix;
    this.keepContents = keepContents;
  }

  @Override
  public void before() throws Exception {
    if (root != null) {
      return;
    }
    root = AbsPath.of(Files.createTempDirectory(prefix).toRealPath());
  }

  public AbsPath getRoot() {
    return root;
  }

  public AbsPath newFolder() throws IOException {
    return AbsPath.of(Files.createTempDirectory(root.getPath(), "tmpFolder"));
  }

  @Override
  public void after() {
    if (root == null) {
      return;
    }

    if (keepContents) {
      System.out.printf("Contents available at %s.\n", getRoot());
      return;
    }

    try {
      Files.walkFileTree(
          root.getPath(),
          new SimpleFileVisitor<Path>() {
            @Override
            public FileVisitResult visitFile(Path file, BasicFileAttributes attrs)
                throws IOException {
              Files.delete(file);
              return FileVisitResult.CONTINUE;
            }

            @Override
            public FileVisitResult postVisitDirectory(Path dir, IOException exc)
                throws IOException {
              Files.delete(dir);
              return FileVisitResult.CONTINUE;
            }
          });
    } catch (IOException e) {
      // Swallow. Nothing sane to do.
    }
  }

  public AbsPath newFile(String fileName) throws IOException {
    AbsPath toCreate = root.resolve(fileName);

    if (Files.exists(toCreate.getPath())) {
      throw new IOException(
          "a file with the name '" + fileName + "' already exists in the test folder");
    }

    return AbsPath.of(Files.createFile(toCreate.getPath()));
  }

  public AbsPath newFile() throws IOException {
    return AbsPath.of(Files.createTempFile(root.getPath(), "junit", "file"));
  }

  public AbsPath newExecutableFile() throws IOException {
    AbsPath newFile = newFile();
    MostFiles.makeExecutable(newFile.getPath());
    return newFile;
  }

  public AbsPath newExecutableFile(String name) throws IOException {
    AbsPath newFile = newFile(name);
    MostFiles.makeExecutable(newFile.getPath());
    return newFile;
  }

  public AbsPath newFolder(String... name) throws IOException {
    AbsPath toCreate = root;
    for (String segment : name) {
      toCreate = toCreate.resolve(segment);
    }

    if (Files.exists(toCreate.getPath())) {
      throw new IOException(
          String.format(
              "a folder with the name '%s' already exists in the test folder",
              Arrays.toString(name)));
    }

    return AbsPath.of(Files.createDirectories(toCreate.getPath()));
  }
}
