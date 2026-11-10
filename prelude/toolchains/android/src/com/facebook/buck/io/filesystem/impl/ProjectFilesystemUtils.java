/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.io.filesystem.impl;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.io.file.MorePaths;
import com.facebook.buck.io.file.MorePosixFilePermissions;
import com.facebook.buck.io.file.MostFiles;
import com.facebook.buck.io.file.PathMatcher;
import com.facebook.buck.io.filesystem.CopySourceMode;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.FluentIterable;
import com.google.common.collect.ImmutableCollection;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableSet;
import com.google.common.collect.UnmodifiableIterator;
import java.io.BufferedInputStream;
import java.io.BufferedOutputStream;
import java.io.BufferedWriter;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.io.OutputStreamWriter;
import java.io.Writer;
import java.nio.channels.Channels;
import java.nio.charset.StandardCharsets;
import java.nio.file.DirectoryStream;
import java.nio.file.FileAlreadyExistsException;
import java.nio.file.FileSystemLoopException;
import java.nio.file.FileVisitOption;
import java.nio.file.FileVisitResult;
import java.nio.file.FileVisitor;
import java.nio.file.Files;
import java.nio.file.LinkOption;
import java.nio.file.Path;
import java.nio.file.SimpleFileVisitor;
import java.nio.file.StandardCopyOption;
import java.nio.file.StandardOpenOption;
import java.nio.file.attribute.BasicFileAttributes;
import java.nio.file.attribute.FileAttribute;
import java.nio.file.attribute.FileTime;
import java.nio.file.attribute.PosixFileAttributeView;
import java.nio.file.attribute.PosixFilePermission;
import java.util.ArrayDeque;
import java.util.Comparator;
import java.util.EnumSet;
import java.util.List;
import java.util.Objects;
import java.util.Optional;
import java.util.Set;
import java.util.function.Function;
import java.util.function.Predicate;
import org.jspecify.annotations.Nullable;

/**
 * A stateless filesystem utils that helps to interact with the filesystem relative to the project
 * root.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ProjectFilesystemUtils {

  private ProjectFilesystemUtils() {}

  /**
   * Gets a list of paths of the contents of the given directory, obeying the ignores. All paths are
   * relative to the root of this view.
   */
  public static ImmutableCollection<Path> getDirectoryContents(
      AbsPath root, ImmutableCollection<PathMatcher> ignores, Path pathRelativeToProjectRoot)
      throws IOException {
    Path path = getPathForRelativePath(root, pathRelativeToProjectRoot);
    try (DirectoryStream<Path> stream = Files.newDirectoryStream(path)) {
      return FluentIterable.from(stream)
          .filter(input -> !isIgnored(relativize(root, input), ignores))
          .transform(absolutePath -> MorePaths.relativize(root.getPath(), absolutePath))
          .toSortedList(Comparator.naturalOrder());
    }
  }

  /** Get path that is an absolute path resolved against the root path. */
  public static Path getPathForRelativePath(AbsPath root, Path pathRelativeToProjectRoot) {
    return getAbsPathForRelativePath(root, pathRelativeToProjectRoot).getPath();
  }

  /** Get path that is an absolute path resolved against the root path. */
  public static Path getPathForRelativePath(AbsPath root, RelPath pathRelativeToProjectRoot) {
    return getPathForRelativePath(root, pathRelativeToProjectRoot.getPath());
  }

  /** Get absolute path that is resolved against the root path. */
  public static AbsPath getAbsPathForRelativePath(AbsPath root, Path pathRelativeToProjectRoot) {
    AbsPath result;
    // We often create {@link Path} instances using
    // {@link java.nio.file.Paths#get(String, String...)}, but there's no guarantee that the
    // underlying {@link FileSystem} is the default one.
    if (pathRelativeToProjectRoot.getFileSystem().equals(root.getFileSystem())) {
      result = root.resolve(pathRelativeToProjectRoot);
    } else {
      result = root.resolve(pathRelativeToProjectRoot.toString());
    }

    return MorePaths.normalize(result);
  }

  /** Get absolute path that is resolved against the root path. */
  public static AbsPath getAbsPathForRelativePath(AbsPath root, RelPath pathRelativeToProjectRoot) {
    return getAbsPathForRelativePath(root, pathRelativeToProjectRoot.getPath());
  }

  /** Construct a relative path between the project root and a given path. */
  public static RelPath relativize(AbsPath root, Path path) {
    return root.relativize(path);
  }

  /** Construct a relative path between the project root and a given path. */
  public static RelPath relativize(AbsPath root, AbsPath path) {
    return root.relativize(path);
  }

  /**
   * @param path the path to check.
   * @return whether ignored paths contains path or any of its ancestors.
   */
  public static boolean isIgnored(
      RelPath path, ImmutableCollection<? extends PathMatcher> ignores) {
    return ignores.stream().anyMatch(pathMatcher -> pathMatcher.matches(path));
  }

  /**
   * @return the absolute path of {@code pathRelativeToProjectRoot} resolved against {@code root}.
   */
  public static AbsPath getPathForRelativePath(AbsPath root, String pathRelativeToProjectRoot) {
    return MorePaths.normalize(root.resolve(pathRelativeToProjectRoot));
  }

  /** Tests whether a file exists. */
  public static boolean exists(
      AbsPath root, Path pathRelativeToProjectRoot, LinkOption... options) {
    return Files.exists(getPathForRelativePath(root, pathRelativeToProjectRoot), options);
  }

  /** Returns the posix file mode of a file */
  public static long getPosixFileModes(AbsPath root, Path pathRelativeToProjectRoot)
      throws IOException {
    long mode = 0;
    // Support executable files.  If we detect this file is executable, store this
    // information as 0100 in the field typically used in zip implementations for
    // POSIX file permissions.  We'll use this information when unzipping.
    Path path = getPathForRelativePath(root, pathRelativeToProjectRoot);
    if (isExecutable(root, path)) {
      mode |= MorePosixFilePermissions.toMode(EnumSet.of(PosixFilePermission.OWNER_EXECUTE));
    }

    if (isDirectory(root, path)) {
      mode |= MostFiles.S_IFDIR;
    } else if (isFile(root, path)) {
      mode |= MostFiles.S_IFREG;
    }

    // Propagate any additional permissions
    mode |= MorePosixFilePermissions.toMode(getPosixFilePermissions(root, path));

    return mode;
  }

  /**
   * Deletes a file specified by its path relative to the project root.
   *
   * @param pathRelativeToProjectRoot path to the file
   */
  public static void deleteFileAtPath(AbsPath root, Path pathRelativeToProjectRoot)
      throws IOException {
    Files.delete(getPathForRelativePath(root, pathRelativeToProjectRoot));
  }

  /** Opens a file, returning an input stream to read from the file. */
  public static InputStream newFileInputStream(AbsPath root, Path pathRelativeToProjectRoot)
      throws IOException {
    return new BufferedInputStream(
        Files.newInputStream(getPathForRelativePath(root, pathRelativeToProjectRoot)));
  }

  /** Checks whether there is a normal file at the specified path. */
  public static boolean isFile(
      AbsPath root, Path pathRelativeToProjectRoot, LinkOption... options) {
    return Files.isRegularFile(getPathForRelativePath(root, pathRelativeToProjectRoot), options);
  }

  /** Allows {@link Files#isDirectory} to be faked in tests. */
  public static boolean isDirectory(
      AbsPath root, Path pathRelativeToProjectRoot, LinkOption... linkOptions) {
    return MorePaths.isDirectory(
        getPathForRelativePath(root, pathRelativeToProjectRoot), linkOptions);
  }

  /** Allows {@link Files#isExecutable} to be faked in tests. */
  public static boolean isExecutable(AbsPath root, Path pathRelativeToProjectRoot) {
    return Files.isExecutable(getPathForRelativePath(root, pathRelativeToProjectRoot));
  }

  /** Sets the last modified time for the given path. */
  public static Path setLastModifiedTime(
      AbsPath root, Path pathRelativeToProjectRoot, FileTime time) throws IOException {
    Path path = getPathForRelativePath(root, pathRelativeToProjectRoot);
    return Files.setLastModifiedTime(path, time);
  }

  /**
   * Recursively delete everything under the specified path. Ignore the failure if the file at the
   * specified path does not exist.
   */
  public static void deleteRecursivelyIfExists(AbsPath root, Path pathRelativeToProjectRoot)
      throws IOException {
    MostFiles.deleteRecursivelyIfExists(getPathForRelativePath(root, pathRelativeToProjectRoot));
  }

  /**
   * @param pathRelativeToProjectRoot Must identify a file, not a directory. (Unfortunately, we have
   *     no way to assert this because the path is not expected to exist yet.)
   */
  public static void createParentDirs(AbsPath root, Path pathRelativeToProjectRoot)
      throws IOException {
    Path path = getPathForRelativePath(root, pathRelativeToProjectRoot);
    mkdirs(root, Objects.requireNonNull(path.getParent()));
  }

  /**
   * Resolves the relative path against the project root and then calls {@link
   * Files#createDirectories(Path, FileAttribute[])}
   */
  public static void mkdirs(AbsPath root, Path pathRelativeToProjectRoot) throws IOException {
    Path resolved = getPathForRelativePath(root, pathRelativeToProjectRoot);
    try {
      Files.createDirectories(resolved);
    } catch (FileAlreadyExistsException e) {
      // Don't complain if the file is a symlink that points to a valid directory.
      // This check is done only on exception as it's a rare case, and lstat is not free.
      if (!Files.isDirectory(resolved)) {
        throw e;
      }
    }
  }

  /**
   * Writes each line in {@code lines} with a trailing newline to a file at the specified path.
   *
   * <p>The parent path of {@code pathRelativeToProjectRoot} must exist.
   */
  public static void writeLinesToPath(
      AbsPath root,
      Iterable<String> lines,
      Path pathRelativeToProjectRoot,
      FileAttribute<?>... attrs)
      throws IOException {
    try (Writer writer =
        new BufferedWriter(
            new OutputStreamWriter(
                newFileOutputStream(root, pathRelativeToProjectRoot, attrs),
                StandardCharsets.UTF_8))) {
      for (String line : lines) {
        writer.write(line);
        writer.write('\n');
      }
    }
  }

  /** Returns a new buffered file output stream */
  public static OutputStream newFileOutputStream(
      AbsPath root, Path pathRelativeToProjectRoot, FileAttribute<?>... attrs) throws IOException {
    return newFileOutputStream(root, pathRelativeToProjectRoot, /* append */ false, attrs);
  }

  /** Returns a new buffered file output stream */
  public static OutputStream newFileOutputStream(
      AbsPath root, Path pathRelativeToProjectRoot, boolean append, FileAttribute<?>... attrs)
      throws IOException {
    return new BufferedOutputStream(
        newUnbufferedFileOutputStream(root, pathRelativeToProjectRoot, append, attrs));
  }

  /** Returns a new uUnbuffered file output stream */
  public static OutputStream newUnbufferedFileOutputStream(
      AbsPath root, Path pathRelativeToProjectRoot, boolean append, FileAttribute<?>... attrs)
      throws IOException {
    return Channels.newOutputStream(
        Files.newByteChannel(
            getPathForRelativePath(root, pathRelativeToProjectRoot),
            append
                ? ImmutableSet.of(StandardOpenOption.CREATE, StandardOpenOption.APPEND)
                : ImmutableSet.of(
                    StandardOpenOption.CREATE,
                    StandardOpenOption.TRUNCATE_EXISTING,
                    StandardOpenOption.WRITE),
            attrs));
  }

  /** Writes bytes to a path. */
  public static void writeBytesToPath(
      AbsPath root, byte[] bytes, Path pathRelativeToProjectRoot, FileAttribute<?>... attrs)
      throws IOException {
    // No need to buffer writes when writing a single piece of data.
    try (OutputStream outputStream =
        newUnbufferedFileOutputStream(root, pathRelativeToProjectRoot, /* append */ false, attrs)) {
      outputStream.write(bytes);
    }
  }

  /** Reads a file content if file exists. */
  public static Optional<String> readFileIfItExists(AbsPath root, Path pathRelativeToProjectRoot) {
    Path fileToRead = getPathForRelativePath(root, pathRelativeToProjectRoot);
    return readFileIfItExists(fileToRead, pathRelativeToProjectRoot.toString());
  }

  /** Reads a file content if file exists. */
  private static Optional<String> readFileIfItExists(
      Path fileToRead, String pathRelativeToProjectRoot) {
    if (Files.isRegularFile(fileToRead)) {
      String contents;
      try {
        contents = Files.readString(fileToRead);
      } catch (IOException e) {
        // Alternatively, we could return Optional.empty(), though something seems suspicious if we
        // have already verified that fileToRead is a file and then we cannot read it.
        throw new RuntimeException("Error reading " + pathRelativeToProjectRoot, e);
      }
      return Optional.of(contents);
    } else {
      return Optional.empty();
    }
  }

  /**
   * Converts a path string (or sequence of strings) to a Path with the same VFS as this instance.
   */
  public static Path getPath(AbsPath root, String first, String... rest) {
    return root.getFileSystem().getPath(first, rest);
  }

  /** Reads a file content as list of lines. */
  public static List<String> readLines(AbsPath root, Path pathRelativeToProjectRoot)
      throws IOException {
    Path file = getPathForRelativePath(root, pathRelativeToProjectRoot);
    return Files.readAllLines(file, StandardCharsets.UTF_8);
  }

  /** Copies a file. */
  public static void copy(AbsPath root, Path source, Path target, CopySourceMode sourceMode)
      throws IOException {
    switch (sourceMode) {
      case FILE:
        Files.copy(
            getPathForRelativePath(root, source),
            getPathForRelativePath(root, target),
            StandardCopyOption.REPLACE_EXISTING);
        break;
      case DIRECTORY_CONTENTS_ONLY:
        MostFiles.copyRecursively(
            getPathForRelativePath(root, source), getPathForRelativePath(root, target));
        break;
      case DIRECTORY_AND_CONTENTS:
        MostFiles.copyRecursively(
            getPathForRelativePath(root, source),
            getPathForRelativePath(root, target.resolve(source.getFileName())));
        break;
    }
  }

  /** Copies a file. */
  public static void copyFile(AbsPath root, Path source, Path target) throws IOException {
    copy(root, source, target, CopySourceMode.FILE);
  }

  /** Creates a symlink. */
  public static void createSymLink(AbsPath root, Path symLink, Path realFile, boolean force)
      throws IOException {
    symLink = getPathForRelativePath(root, symLink);
    if (force) {
      MostFiles.deleteRecursivelyIfExists(symLink);
    }
    MorePaths.createSymLink(MorePaths.WIN_FS_INSTANCE, symLink, realFile);
  }

  /**
   * Returns the set of POSIX file permissions, or the empty set if the underlying file system does
   * not support POSIX file attributes.
   */
  public static Set<PosixFilePermission> getPosixFilePermissions(
      AbsPath root, Path pathRelativeToProjectRoot) throws IOException {
    Path resolvedPath = getPathForRelativePath(root, pathRelativeToProjectRoot);
    if (Files.getFileAttributeView(resolvedPath, PosixFileAttributeView.class) != null) {
      return Files.getPosixFilePermissions(resolvedPath);
    } else {
      return ImmutableSet.of();
    }
  }

  /**
   * @param root Absolute path that defines a project root
   * @param buckOut Relative path that defines a buck out directory
   * @param path Absolute or relative path to the project root.
   * @return If {@code path} is relative, it is returned. If it is absolute and is inside the
   *     project root, it is relativized to the project root and returned. Otherwise an absent value
   *     is returned.
   */
  public static Optional<Path> getPathRelativeToProjectRoot(
      AbsPath root, RelPath buckOut, Path path) {
    Path normalizedPath = MorePaths.normalize(path);
    if (normalizedPath.isAbsolute()) {
      AbsPath pathAbs = AbsPath.of(normalizedPath);
      AbsPath configuredBuckOut = MorePaths.normalize(root.resolve(buckOut));
      // If the path is in the configured buck-out, it's also part of the filesystem.
      if (pathAbs.startsWith(configuredBuckOut) || pathAbs.startsWith(root)) {
        return Optional.of(MorePaths.relativize(root.getPath(), normalizedPath));
      } else {
        return Optional.empty();
      }
    } else {
      return Optional.of(normalizedPath);
    }
  }

  /** Walks a project-root relative file tree with a visitor and visit options. */
  public static void walkRelativeFileTree(
      AbsPath projectRoot,
      Path pathRelativeToProjectRoot,
      EnumSet<FileVisitOption> visitOptions,
      FileVisitor<Path> fileVisitor,
      DirectoryStream.Filter<? super Path> ignoreFilter)
      throws IOException {
    Path rootPath = getPathForRelativePath(projectRoot, pathRelativeToProjectRoot);
    Function<Path, Path> pathMapper = path -> relativize(projectRoot, path).getPath();
    FileVisitor<Path> pathMappingVisitor =
        new FileVisitor<Path>() {
          @Override
          public FileVisitResult preVisitDirectory(Path dir, BasicFileAttributes attrs)
              throws IOException {
            return fileVisitor.preVisitDirectory(pathMapper.apply(dir), attrs);
          }

          @Override
          public FileVisitResult visitFile(Path file, BasicFileAttributes attrs)
              throws IOException {
            return fileVisitor.visitFile(pathMapper.apply(file), attrs);
          }

          @Override
          public FileVisitResult visitFileFailed(Path file, IOException exc) throws IOException {
            return fileVisitor.visitFileFailed(pathMapper.apply(file), exc);
          }

          @Override
          public FileVisitResult postVisitDirectory(Path dir, @Nullable IOException exc)
              throws IOException {
            return fileVisitor.postVisitDirectory(pathMapper.apply(dir), exc);
          }
        };
    walkFileTree(projectRoot, rootPath, visitOptions, pathMappingVisitor, ignoreFilter);
  }

  /** Returns ignore filter. */
  public static DirectoryStream.Filter<? super Path> getIgnoreFilter(
      @Nullable AbsPath projectRoot,
      boolean skipIgnored,
      ImmutableCollection<PathMatcher> ignores) {
    return skipIgnored
        ? input -> !isIgnored(relativize(Objects.requireNonNull(projectRoot), input), ignores)
        : input -> true;
  }

  public static DirectoryStream.Filter<? super Path> getEmptyIgnoreFilter() {
    return getIgnoreFilter(null, false, ImmutableSet.of());
  }

  public static EnumSet<FileVisitOption> getDefaultVisitOptions() {
    return EnumSet.of(FileVisitOption.FOLLOW_LINKS);
  }

  public static void walkFileTree(
      AbsPath root,
      Path path,
      Set<FileVisitOption> options,
      FileVisitor<Path> fileVisitor,
      DirectoryStream.Filter<? super Path> ignoreFilter)
      throws IOException {
    new FileTreeWalker(getPathForRelativePath(root, path), options, fileVisitor, ignoreFilter)
        .walk();
  }

  /** Returns relative paths for all files under the given path */
  public static ImmutableSet<Path> getFilesUnderPath(
      AbsPath root,
      Path pathRelativeToProjectRoot,
      EnumSet<FileVisitOption> visitOptions,
      DirectoryStream.Filter<? super Path> ignoresFilter)
      throws IOException {
    return getFilesUnderPath(
        root, pathRelativeToProjectRoot, x -> true, visitOptions, ignoresFilter);
  }

  /** Returns relative paths for all files under the given path */
  public static ImmutableSet<Path> getFilesUnderPath(
      AbsPath root,
      Path pathRelativeToProjectRoot,
      Predicate<Path> predicate,
      EnumSet<FileVisitOption> visitOptions,
      DirectoryStream.Filter<? super Path> ignoresFilter)
      throws IOException {
    ImmutableSet.Builder<Path> paths = ImmutableSet.builder();
    walkRelativeFileTree(
        root,
        pathRelativeToProjectRoot,
        visitOptions,
        new SimpleFileVisitor<Path>() {
          @Override
          public FileVisitResult visitFile(Path path, BasicFileAttributes attributes) {
            if (predicate.test(path)) {
              paths.add(path);
            }
            return FileVisitResult.CONTINUE;
          }
        },
        ignoresFilter);

    return paths.build();
  }

  /**
   * FileTreeWalker is used to walk files similar to Files.walkFileTree.
   *
   * <p>It has two major differences from walkFileTree.
   *
   * <ol>
   *   <li>It ignores files and directories ignored by this ProjectFilesystem.
   *   <li>The walk is in a deterministic order.
   * </ol>
   *
   * <p>And it has three minor differences.
   *
   * <ol>
   *   <li>It doesn't accept a depth limit.
   *   <li>It doesn't handle the presence of a security manager the same way.
   *   <li>It skips symlinks which create cycles instead of calling visitFileFailed method with the
   *       FileSystemLoopException.
   * </ol>
   */
  private static class FileTreeWalker {
    private final FileVisitor<Path> visitor;
    private final Path root;
    private final boolean followLinks;
    private final ArrayDeque<DirWalkState> state;
    private final DirectoryStream.Filter<? super Path> ignoreFilter;

    FileTreeWalker(
        Path root,
        Set<FileVisitOption> options,
        FileVisitor<Path> pathFileVisitor,
        DirectoryStream.Filter<? super Path> ignoreFilter) {
      this.followLinks = options.contains(FileVisitOption.FOLLOW_LINKS);
      this.visitor = pathFileVisitor;
      this.root = root;
      this.state = new ArrayDeque<>();
      this.ignoreFilter = ignoreFilter;
    }

    private ImmutableList<Path> getContents(Path root) throws IOException {
      try (DirectoryStream<Path> stream = Files.newDirectoryStream(root, ignoreFilter)) {
        return FluentIterable.from(stream).toSortedList(Comparator.naturalOrder());
      }
    }

    private class DirWalkState {
      private final Path dir;
      private final BasicFileAttributes attrs;
      private final boolean isRootSentinel;
      private UnmodifiableIterator<Path> iter;
      private @Nullable IOException ioe = null;

      DirWalkState(Path directory, BasicFileAttributes attributes, boolean isRootSentinel) {
        this.dir = directory;
        this.attrs = attributes;
        this.isRootSentinel = isRootSentinel;

        if (isRootSentinel) {
          this.iter = ImmutableList.of(root).iterator();
        } else {
          try {
            this.iter = getContents(directory).iterator();
          } catch (IOException e) {
            this.iter = ImmutableList.<Path>of().iterator();
            this.ioe = e;
          }
        }
      }
    }

    private void walk() throws IOException {
      state.add(new DirWalkState(root, getAttributes(root), true));

      while (true) {
        FileVisitResult result;
        if (state.getLast().iter.hasNext()) {
          result = visitPath(state.getLast().iter.next());
        } else {
          DirWalkState dirState = state.removeLast();
          if (dirState.isRootSentinel) {
            return;
          }
          result = visitor.postVisitDirectory(dirState.dir, dirState.ioe);
        }
        Objects.requireNonNull(result, "FileVisitor returned a null FileVisitResult.");
        if (result == FileVisitResult.SKIP_SIBLINGS) {
          state.getLast().iter = ImmutableList.<Path>of().iterator();
        } else if (result == FileVisitResult.TERMINATE) {
          return;
        }
      }
    }

    private FileVisitResult visitPath(Path p) throws IOException {
      BasicFileAttributes attrs;
      try {
        attrs = getAttributes(p);
        ensureNoLoops(p, attrs);
      } catch (FileSystemLoopException ex) {
        // Skip loop if found
        return FileVisitResult.CONTINUE;
      } catch (IOException ioe) {
        return visitor.visitFileFailed(p, ioe);
      }

      if (attrs.isDirectory()) {
        FileVisitResult result = visitor.preVisitDirectory(p, attrs);
        if (result == FileVisitResult.CONTINUE) {
          state.add(new DirWalkState(p, attrs, false));
        }
        return result;
      } else {
        return visitor.visitFile(p, attrs);
      }
    }

    private void ensureNoLoops(Path p, BasicFileAttributes attrs) throws FileSystemLoopException {
      if (!followLinks) {
        return;
      }
      if (!attrs.isDirectory()) {
        return;
      }
      if (willLoop(p, attrs)) {
        throw new FileSystemLoopException(p.toString());
      }
    }

    private boolean willLoop(Path p, BasicFileAttributes attrs) {
      try {
        Object thisKey = attrs.fileKey();
        for (DirWalkState s : state) {
          if (s.isRootSentinel) {
            continue;
          }
          Object thatKey = s.attrs.fileKey();
          if (thisKey != null && thatKey != null) {
            if (thisKey.equals(thatKey)) {
              return true;
            }
          } else if (Files.isSameFile(p, s.dir)) {
            return true;
          }
        }
      } catch (IOException e) {
        return true;
      }
      return false;
    }

    private BasicFileAttributes getAttributes(Path root) throws IOException {
      if (!followLinks) {
        return Files.readAttributes(root, BasicFileAttributes.class, LinkOption.NOFOLLOW_LINKS);
      }
      try {
        return Files.readAttributes(root, BasicFileAttributes.class);
      } catch (IOException e) {
        return Files.readAttributes(root, BasicFileAttributes.class, LinkOption.NOFOLLOW_LINKS);
      }
    }
  }
}
