/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java;

import com.facebook.infer.annotation.Nullsafe;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.io.Reader;
import java.io.Writer;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.Iterator;
import java.util.List;
import java.util.Objects;
import java.util.Set;
import javax.annotation.Nullable;
import javax.annotation.concurrent.NotThreadSafe;
import javax.tools.FileObject;
import javax.tools.ForwardingJavaFileObject;
import javax.tools.JavaFileObject;
import javax.tools.StandardJavaFileManager;
import javax.tools.StandardLocation;

@NotThreadSafe
@Nullsafe(Nullsafe.Mode.LOCAL)
class ListenableFileManager extends ForwardingStandardJavaFileManager {

  private final FileObjectWrapper fileTracker;
  private final List<FileManagerListener> listeners = new ArrayList<>();
  @Nullable private final CompilerClassToSourceMap classToSourceMap;
  private final Path sourceRoot;

  public ListenableFileManager(StandardJavaFileManager fileManager) {
    this(fileManager, null, Paths.get(""));
  }

  public ListenableFileManager(
      StandardJavaFileManager fileManager,
      @Nullable CompilerClassToSourceMap classToSourceMap,
      Path sourceRoot) {
    super(fileManager);
    fileTracker = new FileObjectWrapper();
    this.classToSourceMap = classToSourceMap;
    this.sourceRoot = sourceRoot;
  }

  public void addListener(FileManagerListener listener) {
    listeners.add(listener);
  }

  public void removeListener(FileManagerListener listener) {
    listeners.remove(listener);
  }

  @Override
  @Nullable
  public String inferBinaryName(Location location, JavaFileObject file) {
    // javac does not play nice with wrapped file objects in this method; so we unwrap
    return super.inferBinaryName(location, unwrap(file));
  }

  @Override
  public boolean isSameFile(FileObject a, FileObject b) {
    // javac does not play nice with wrapped file objects in this method; so we unwrap
    return super.isSameFile(unwrap(a), unwrap(b));
  }

  private JavaFileObject unwrap(JavaFileObject file) {
    if (file instanceof ListenableJavaFileObject) {
      return ((ListenableJavaFileObject) file).getJavaFileObject();
    }
    return file;
  }

  private FileObject unwrap(FileObject file) {
    if (file instanceof JavaFileObject) {
      return unwrap((JavaFileObject) file);
    }

    return file;
  }

  @Override
  public Iterable<? extends JavaFileObject> getJavaFileObjectsFromFiles(
      Iterable<? extends File> files) {
    return fileManager.getJavaFileObjectsFromFiles(files);
  }

  @Override
  public Iterable<? extends JavaFileObject> getJavaFileObjects(File... files) {
    return fileManager.getJavaFileObjects(files);
  }

  @Override
  public Iterable<? extends JavaFileObject> getJavaFileObjectsFromStrings(Iterable<String> names) {
    return fileManager.getJavaFileObjectsFromStrings(names);
  }

  @Override
  public Iterable<? extends JavaFileObject> getJavaFileObjects(String... names) {
    return fileManager.getJavaFileObjects(names);
  }

  @Override
  public Iterable<JavaFileObject> list(
      Location location, String packageName, Set<JavaFileObject.Kind> kinds, boolean recurse)
      throws IOException {
    Iterable<JavaFileObject> listIterator = super.list(location, packageName, kinds, recurse);
    if (location == StandardLocation.ANNOTATION_PROCESSOR_PATH) {
      return listIterator;
    } else {
      return new TrackingIterable(listIterator);
    }
  }

  @Override
  @Nullable
  public JavaFileObject getJavaFileForInput(
      Location location, String className, JavaFileObject.Kind kind) throws IOException {
    JavaFileObject javaFileObject = super.getJavaFileForInput(location, className, kind);
    if (javaFileObject == null) {
      return null;
    }
    if (location == StandardLocation.ANNOTATION_PROCESSOR_PATH) {
      return javaFileObject;
    } else {
      return fileTracker.wrap(javaFileObject);
    }
  }

  @Override
  public JavaFileObject getJavaFileForOutput(
      Location location, String className, JavaFileObject.Kind kind, @Nullable FileObject sibling)
      throws IOException {
    JavaFileObject javaFileObject = super.getJavaFileForOutput(location, className, kind, sibling);
    if (classToSourceMap != null
        && location == StandardLocation.CLASS_OUTPUT
        && kind == JavaFileObject.Kind.CLASS) {
      List<Path> sources =
          sibling != null && "file".equals(sibling.toUri().getScheme())
              ? List.of(Paths.get(sibling.toUri()))
              : List.of();
      classToSourceMap.put(className.replace('.', '/') + kind.extension, sources, sourceRoot);
    }
    if (location == StandardLocation.ANNOTATION_PROCESSOR_PATH) {
      return javaFileObject;
    } else {
      return fileTracker.wrap(javaFileObject);
    }
  }

  @Override
  @Nullable
  public FileObject getFileForInput(Location location, String packageName, String relativeName)
      throws IOException {
    FileObject fileObject = super.getFileForInput(location, packageName, relativeName);
    if (location == StandardLocation.ANNOTATION_PROCESSOR_PATH || fileObject == null) {
      return fileObject;
    } else {
      return fileTracker.wrap(fileObject);
    }
  }

  @Override
  public FileObject getFileForOutput(
      Location location, String packageName, String relativeName, @Nullable FileObject sibling)
      throws IOException {
    FileObject fileObject = super.getFileForOutput(location, packageName, relativeName, sibling);
    if (location == StandardLocation.ANNOTATION_PROCESSOR_PATH) {
      return fileObject;
    } else {
      return fileTracker.wrap(fileObject);
    }
  }

  private class TrackingIterable implements Iterable<JavaFileObject> {
    private final Iterable<? extends JavaFileObject> inner;

    public TrackingIterable(Iterable<? extends JavaFileObject> inner) {
      this.inner = inner;
    }

    @Override
    public Iterator<JavaFileObject> iterator() {
      return new TrackingIterator(inner.iterator());
    }
  }

  private class TrackingIterator implements Iterator<JavaFileObject> {

    private final Iterator<? extends JavaFileObject> inner;

    public TrackingIterator(Iterator<? extends JavaFileObject> inner) {
      this.inner = inner;
    }

    @Override
    public boolean hasNext() {
      return inner.hasNext();
    }

    @Override
    public JavaFileObject next() {
      return fileTracker.wrap(inner.next());
    }

    @Override
    public void remove() {
      inner.remove();
    }
  }

  private class FileObjectWrapper {
    public FileObject wrap(FileObject inner) {
      if (inner instanceof JavaFileObject) {
        return wrap((JavaFileObject) inner);
      }
      return inner;
    }

    public JavaFileObject wrap(JavaFileObject inner) {
      return new ListenableJavaFileObject(Objects.requireNonNull(inner));
    }
  }

  private class ListenableJavaFileObject extends ForwardingJavaFileObject<JavaFileObject> {
    public ListenableJavaFileObject(JavaFileObject fileObject) {
      super(fileObject);
    }

    @Override
    public String toString() {
      return fileObject.getName();
    }

    public JavaFileObject getJavaFileObject() {
      return fileObject;
    }

    @Override
    public InputStream openInputStream() throws IOException {
      listeners.forEach(it -> it.onFileRead(fileObject));
      return super.openInputStream();
    }

    @Override
    public Reader openReader(boolean ignoreEncodingErrors) throws IOException {
      listeners.forEach(it -> it.onFileRead(fileObject));
      return super.openReader(ignoreEncodingErrors);
    }

    @Override
    @Nullable
    public CharSequence getCharContent(boolean ignoreEncodingErrors) throws IOException {
      listeners.forEach(it -> it.onFileRead(fileObject));
      return super.getCharContent(ignoreEncodingErrors);
    }

    @Override
    public OutputStream openOutputStream() throws IOException {
      listeners.forEach(it -> it.onFileWritten(fileObject));
      return super.openOutputStream();
    }

    @Override
    public Writer openWriter() throws IOException {
      listeners.forEach(it -> it.onFileWritten(fileObject));
      return super.openWriter();
    }
  }
}
