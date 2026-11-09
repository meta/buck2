/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.abi;

import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.when;

import com.facebook.buck.jvm.java.ReducedClasspathSourceOnlyAbiRuleInfoFactory;
import com.facebook.buck.jvm.java.abi.source.api.SourceOnlyAbiRuleInfoFactory;
import com.facebook.buck.jvm.java.abi.source.api.SourceOnlyAbiRuleInfoFactory.SourceOnlyAbiRuleInfo;
import com.facebook.buck.jvm.java.plugin.api.BuckJavacTaskProxy;
import com.facebook.buck.jvm.java.plugin.api.PluginClassLoader;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import javax.tools.Diagnostic;
import javax.tools.JavaFileManager;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class AbiValidationInputTest {
  @Rule public TemporaryFolder temporaryFolder = new TemporaryFolder();

  @Test
  public void missingValidatorClassReportsUnsupportedCompiler() {
    RuntimeException failure =
        assertThrows(
            RuntimeException.class,
            () ->
                SourceBasedAbiStubber.newInheritedMemberTypeValidatingTaskListener(
                    mock(PluginClassLoader.class),
                    mock(BuckJavacTaskProxy.class),
                    mock(SourceOnlyAbiRuleInfo.class),
                    () -> false,
                    Diagnostic.Kind.ERROR));
    assertTrue(failure.getCause() instanceof ClassNotFoundException);
    String message = failure.getMessage();
    assertTrue(
        message != null && message.startsWith("Could not load source-generated ABI validator."));
  }

  @Test
  public void unreadableReducedClasspathJarFailsClosed() throws IOException {
    Path jar = temporaryFolder.newFile("invalid.jar").toPath();
    Files.writeString(jar, "not a jar");
    JavaFileManager fileManager = mock(JavaFileManager.class);
    SourceOnlyAbiRuleInfoFactory delegate = mock(SourceOnlyAbiRuleInfoFactory.class);
    when(delegate.create(fileManager)).thenReturn(mock(SourceOnlyAbiRuleInfo.class));
    ReducedClasspathSourceOnlyAbiRuleInfoFactory factory =
        new ReducedClasspathSourceOnlyAbiRuleInfoFactory(delegate, List.of(jar), List.of());
    UncheckedIOException failure =
        assertThrows(UncheckedIOException.class, () -> factory.create(fileManager));
    String message = failure.getMessage();
    assertTrue(message != null && message.contains(jar.toString()));
  }
}
