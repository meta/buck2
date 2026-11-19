/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.cli.bootstrapper;

import com.facebook.infer.annotation.Nullsafe;
import java.util.Arrays;

/**
 * This class sets up a separate ClassLoader for most of Buck's implementation, leaving only the
 * bare minimum bootstrapping classes (and a few classes for compatibility with library code that is
 * not ClassLoader-aware) in the system ClassLoader. This is done so that annotation processors do
 * not have their classpaths polluted with Buck's dependencies when Buck is compiling Java code
 * in-process.
 *
 * <p>Under JSR-199, when the Java compiler is run in-process it uses a ClassLoader that is a child
 * of the system ClassLoader. In order for annotation processors to access the Compiler Tree API
 * (which lives in tools.jar with the compiler itself), they must be loaded with a ClassLoader
 * descended from the compiler's. If Buck used the system ClassLoader as a normal Java application
 * would, this would result in annotation processors getting Buck's versions of Guava, Jackson, etc.
 * instead of their own.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public final class ClassLoaderBootstrapper {

  static {
    // On macOS, this will suppress the Dock icon if any libraries try to use AWT somewhere.
    System.setProperty("apple.awt.UIElement", "true");
  }

  private static final ClassLoader classLoader = ClassLoaderFactory.withEnv().create();

  private ClassLoaderBootstrapper() {}

  /** Main method */
  public static void main(String[] args) {
    // Some things (notably Jetty) use the context class loader to load stuff
    Thread.currentThread().setContextClassLoader(classLoader);

    String mainClassName = args[0];
    String[] remainingArgs = Arrays.copyOfRange(args, 1, args.length);
    ClassLoaderBootstrapperUtils.invokeMainMethod(classLoader, mainClassName, remainingArgs);
  }

  public static Class<?> loadClass(String name) {
    try {
      return classLoader.loadClass(name);
    } catch (ClassNotFoundException e) {
      throw new NoClassDefFoundError(name);
    }
  }
}
