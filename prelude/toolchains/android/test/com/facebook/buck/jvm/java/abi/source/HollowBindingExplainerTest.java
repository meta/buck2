/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.abi.source;

import static org.junit.Assert.assertTrue;

import com.facebook.buck.jvm.java.testutil.compiler.CompilerTreeApiTest;
import com.google.common.base.Joiner;
import com.google.common.collect.ImmutableMap;
import com.sun.source.tree.ClassTree;
import com.sun.source.tree.CompilationUnitTree;
import com.sun.source.tree.Tree;
import com.sun.source.tree.VariableTree;
import com.sun.source.util.TaskEvent;
import com.sun.source.util.TaskListener;
import com.sun.source.util.TreePath;
import java.io.IOException;
import org.junit.Test;

/**
 * Tests for {@link HollowBindingExplainer}. The arity crash itself needs production classpath shape
 * to trigger, so these tests call the explainer directly on attributions with truly-missing
 * supertypes and assert the reported names and machine line.
 */
public class HollowBindingExplainerTest extends CompilerTreeApiTest {
  private String explanation;

  @Test
  public void testMissingSupertypesNamed() throws IOException {
    withClasspath(ImmutableMap.of());
    initCompiler(
        ImmutableMap.of(
            "User.java",
            Joiner.on('\n')
                .join(
                    "package com.foo;",
                    "import com.dep.MissingIface;",
                    "import com.dep.Outer;",
                    "public class User implements MissingIface, SamePkgMissing, Outer.Inner {",
                    "  String field;",
                    "}")));
    testCompiler.setAllowCompilationErrors(true);
    captureExplanation("field");
    testCompiler.compile();

    assertTrue(explanation, explanation.contains("com.foo.User"));
    assertTrue(explanation, explanation.contains("com.dep.MissingIface"));
    assertTrue(explanation, explanation.contains("com.foo.SamePkgMissing"));
    assertTrue(explanation, explanation.contains("com.dep.Outer.Inner"));
    assertTrue(
        explanation,
        explanation.contains(
            "KOSABI_JASABI_MISSING_SUPERTYPES: "
                + "com.dep.MissingIface, com.dep.Outer.Inner, com.foo.SamePkgMissing"));
  }

  @Test
  public void testStarImportIsApproximate() throws IOException {
    withClasspath(ImmutableMap.of());
    initCompiler(
        ImmutableMap.of(
            "User.java",
            Joiner.on('\n')
                .join(
                    "package com.foo;",
                    "import com.dep.*;",
                    "public class User implements StarMissing {",
                    "  String field;",
                    "}")));
    testCompiler.setAllowCompilationErrors(true);
    captureExplanation("field");
    testCompiler.compile();

    assertTrue(explanation, explanation.contains("StarMissing"));
    assertTrue(explanation, explanation.endsWith("KOSABI_JASABI_MISSING_SUPERTYPES: "));
  }

  @Test
  public void testCompleteHierarchyFallsBack() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/dep/Base.java",
            Joiner.on('\n').join("package com.dep;", "public class Base { }")));
    initCompiler(
        ImmutableMap.of(
            "User.java",
            Joiner.on('\n')
                .join(
                    "package com.foo;",
                    "import com.dep.Base;",
                    "public class User extends Base {",
                    "  String field;",
                    "}")));
    testCompiler.setAllowCompilationErrors(true);
    captureExplanation("field");
    testCompiler.compile();

    assertTrue(explanation, explanation.contains("no direct supertype link"));
  }

  private void captureExplanation(String fieldName) {
    testCompiler.setTaskListener(
        new TaskListener() {
          @Override
          public void started(TaskEvent e) {}

          @Override
          public void finished(TaskEvent e) {
            if (e.getKind() != TaskEvent.Kind.ANALYZE) {
              return;
            }
            CompilationUnitTree unit = e.getCompilationUnit();
            for (Tree decl : unit.getTypeDecls()) {
              if (!(decl instanceof ClassTree)) {
                continue;
              }
              for (Tree member : ((ClassTree) decl).getMembers()) {
                if (member instanceof VariableTree
                    && ((VariableTree) member).getName().contentEquals(fieldName)) {
                  TreePath path =
                      new TreePath(
                          new TreePath(new TreePath(unit), decl),
                          ((VariableTree) member).getType());
                  explanation =
                      HollowBindingExplainer.explain(
                              trees, path, "Lazy", new IllegalArgumentException("test"))
                          .getMessage();
                }
              }
            }
          }
        });
  }
}
