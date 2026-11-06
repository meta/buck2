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

import com.facebook.buck.util.zip.JarBuilder;
import com.facebook.infer.annotation.Nullsafe;
import java.net.URI;
import javax.annotation.Nullable;
import javax.lang.model.element.Modifier;
import javax.lang.model.element.NestingKind;
import javax.tools.FileObject;
import javax.tools.JavaFileObject;
import javax.tools.SimpleJavaFileObject;

/**
 * A {@link JavaFileObject} implementation that allows using jar URIs unlike {@link
 * SimpleJavaFileObject} that assumes the uri used is a file system uri. This implementation can be
 * used when the CLASS_OUTPUT is represented by a straight to jar write. The only content that it
 * stores is the full uri, the relative path within the jar and the kind of the {@link FileObject}.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public abstract class JarFileObject implements JavaFileObject {

  protected final URI uri;
  protected final String pathInJar;
  protected final Kind kind;

  public JarFileObject(URI uri, String pathInJar, Kind kind) {
    this.uri = uri;
    this.pathInJar = pathInJar;
    this.kind = kind;
  }

  @Override
  public URI toUri() {
    return uri;
  }

  @Override
  public String getName() {
    return pathInJar;
  }

  @Override
  public long getLastModified() {
    return 0L;
  }

  @Override
  public boolean delete() {
    return false;
  }

  @Override
  public Kind getKind() {
    return this.kind;
  }

  @Override
  public boolean isNameCompatible(String simpleName, Kind kind) {
    String baseName = simpleName + kind.extension;
    return kind.equals(getKind())
        && (baseName.equals(pathInJar) || pathInJar.endsWith("/" + baseName));
  }

  @Override
  @Nullable
  public NestingKind getNestingKind() {
    return null;
  }

  @Override
  @Nullable
  public Modifier getAccessLevel() {
    return null;
  }

  @Override
  public String toString() {
    return this.getClass().getName() + "[" + toUri() + "]";
  }

  public abstract void writeToJar(JarBuilder jarBuilder);
}
