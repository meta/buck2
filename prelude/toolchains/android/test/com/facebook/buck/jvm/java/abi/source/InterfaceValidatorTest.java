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

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import com.facebook.buck.jvm.java.testutil.compiler.CompilerTreeApiTest;
import com.google.common.base.Joiner;
import com.google.common.collect.ImmutableMap;
import com.sun.source.tree.CompilationUnitTree;
import java.io.IOException;
import java.util.Map;
import org.junit.Before;
import org.junit.Test;

public class InterfaceValidatorTest extends CompilerTreeApiTest {
  final ImmutableMap<String, String> CLASSPATH_WITH_COMPLEX_MEMBER_TYPES =
      ImmutableMap.of(
          "com/facebook/bar/Bar.java",
          Joiner.on('\n')
              .join(
                  "package com.facebook.bar;",
                  "import com.facebook.baz.Baz;",
                  "public class Bar extends Baz {",
                  "  public static class StaticMember extends SuperStaticMember{",
                  "    public static class StaticMemberer extends SuperStaticMemberer {",
                  "    }",
                  "  }",
                  "  public class Inner extends Baz.Inner { }",
                  "}"),
          "com/facebook/baz/Baz.java",
          Joiner.on('\n')
              .join(
                  "package com.facebook.baz;",
                  "public class Baz {",
                  "  public static class SuperStaticMember {",
                  "    public static class SuperStaticMemberer {",
                  "      public static class StaticMemberest {",
                  "      }",
                  "    }",
                  "  }",
                  "  public class Inner {",
                  "    public class Innerer { }",
                  "  }",
                  "}"));

  private ValidatingTaskListenerFactory taskListenerFactory;

  @Before
  public void setUp() {
    taskListenerFactory = new ValidatingTaskListenerFactory("//:rule");
  }

  @Test
  public void testSimpleClassPasses() throws IOException {
    compileWithValidation("public class Foo { }");

    assertNoErrors();
  }

  @Test
  public void testImplicitAnnotationSuperclassSucceeds() throws IOException {
    taskListenerFactory.setRuleIsRequiredForSourceAbi(true);
    compileWithValidation("@interface Foo { };");

    assertNoErrors();
  }

  @Test
  public void testAnnotationInNonRequiredRuleFails() throws IOException {
    testCompiler.setAllowCompilationErrors(true);
    compileWithValidation(ImmutableMap.of("Foo.java", "@interface Foo { @interface Inner { } };"));

    assertErrors(
        "Foo.java:1: error: Annotation definitions must be in rules with"
            + " required_for_source_only_abi = True.\n"
            + "@interface Foo { @interface Inner { } };\n"
            + " ^\n"
            + "  For a quick fix, add required_for_source_only_abi = True to //:rule.\n"
            + "  A better fix is to move Foo to a new rule that contains only\n"
            + "  annotations, and mark that rule required_for_source_only_abi.",
        "Foo.java:1: error: Annotation definitions must be in rules with"
            + " required_for_source_only_abi = True.\n"
            + "@interface Foo { @interface Inner { } };\n"
            + "                  ^\n"
            + "  For a quick fix, add required_for_source_only_abi = True to //:rule.\n"
            + "  A better fix is to move Inner to a new rule that contains only\n"
            + "  annotations, and mark that rule required_for_source_only_abi.");
  }

  @Test
  public void testMissingGrandSuperFailsButMissingSuperDoesNot() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.bar;",
                    "import com.facebook.baz.Baz;",
                    "import com.facebook.iface2.Interface2;",
                    "public class Bar extends Baz implements Interface2 { }"),
            "com/facebook/baz/Baz.java",
            Joiner.on('\n').join("package com.facebook.baz;", "public class Baz { }"),
            "com/facebook/iface/Interface.java",
            Joiner.on('\n').join("package com.facebook.iface;", "public interface Interface { }"),
            "com/facebook/iface2/Interface2.java",
            Joiner.on('\n')
                .join("package com.facebook.iface2;", "public interface Interface2 { }")));
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/bar:bar");
    testCompiler.setAllowCompilationErrors(true);
    compileWithValidation(
        ImmutableMap.of(
            "com/facebook/foo/Foo.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.foo;",
                    "import com.facebook.bar.Bar;",
                    "import com.facebook.iface.Interface;",
                    "public class Foo extends Bar implements Interface { }")));

    assertErrors(
        "Foo.java:4: error: Source-only ABI generation requires that this type be unavailable, or"
            + " that all of its superclasses/interfaces be available.\n"
            + "public class Foo extends Bar implements Interface { }\n"
            + "                         ^\n"
            + "  To fix, add the following rules to source_only_abi_deps in //:rule:"
            + " //com/facebook/baz:baz, //com/facebook/iface2:iface2");
  }

  @Test
  public void testMissingSuperSucceeds() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.bar;",
                    "import com.facebook.baz.Baz;",
                    "public class Bar extends Baz { }"),
            "com/facebook/baz/Baz.java",
            Joiner.on('\n').join("package com.facebook.baz;", "public class Baz { }"),
            "com/facebook/iface/Interface.java",
            Joiner.on('\n').join("package com.facebook.iface;", "public interface Interface { }")));
    compileWithValidation(
        ImmutableMap.of(
            "com/facebook/foo/Foo.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.foo;",
                    "import com.facebook.bar.Bar;",
                    "import com.facebook.iface.Interface;",
                    "public class Foo extends Bar implements Interface { }")));

    assertNoErrors();
  }

  @Test
  public void testFullyQualifiedNameFromBootClasspathSucceeds() throws IOException {
    compileWithValidation("abstract class Foo implements java.util.List { }");

    assertNoErrors();
  }

  @Test
  public void testFullyQualifiedNameFromClasspathSucceeds() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            Joiner.on('\n').join("package com.facebook.bar;", "public class Bar { }")));
    compileWithValidation("class Foo extends com.facebook.bar.Bar { }");

    assertNoErrors();
  }

  @Test
  public void testSelfSucceeds() throws IOException {
    compileWithValidation(Joiner.on('\n').join("class Foo {", "  Foo f;", "}"));

    assertNoErrors();
  }

  @Test
  public void testSiblingTypeSucceeds() throws IOException {
    compileWithValidation(
        Joiner.on('\n')
            .join("class Foo {", "  class Bar {", "    Baz b;", "  }", "  class Baz { }", "}"));

    assertNoErrors();
  }

  @Test
  public void testSiblingOfParentTypeSucceeds() throws IOException {
    compileWithValidation(
        Joiner.on('\n')
            .join(
                "class Foo {",
                "  class Bar {",
                "    class Baz {",
                "      Waz w;",
                "    }",
                "  }",
                "  class Waz { }",
                "}"));

    assertNoErrors();
  }

  @Test
  public void testPackageReferenceFromInnerTypeSucceeds() throws IOException {
    compileWithValidation(
        ImmutableMap.of(
            "Foo.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.foo;",
                    "class Foo {",
                    "  class Inner {",
                    "    Bar b;",
                    "  }",
                    "}"),
            "Bar.java",
            Joiner.on('\n').join("package com.facebook.foo;", "class Bar { }")));

    assertNoErrors();
  }

  @Test
  public void testImportedPackageAnnotationSucceeds() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/anno/Anno.java",
            Joiner.on('\n').join("package com.facebook.anno;", "public @interface Anno { }")));

    compileWithValidation(
        ImmutableMap.of(
            "com/facebook/foo/package-info.java",
            Joiner.on('\n')
                .join("@Anno", "package com.facebook.foo;", "import com.facebook.anno.Anno;")));

    assertNoErrors();
  }

  @Test
  public void testBadCaseImportFromClasspathSucceeds() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/someclass.java",
            Joiner.on('\n').join("package com.facebook.bar;", "public class someclass { }")));

    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/bar:bar");
    compileWithValidation(
        Joiner.on('\n').join("import com.facebook.bar.someclass;", "public class Foo { }"));

    assertNoErrors();
  }

  @Test
  public void testBadCaseImportFails() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/someclass.java",
            Joiner.on('\n').join("package com.facebook.bar;", "public class someclass { }")));

    testCompiler.setAllowCompilationErrors(true);
    compileWithValidation(
        Joiner.on('\n').join("import com.facebook.bar.someclass;", "public class Foo { }"));

    assertErrors(
        Joiner.on('\n')
            .join(
                "Foo.java:1: error: Source-only ABI generation requires top-level class names to"
                    + " start with a capital letter.",
                "import com.facebook.bar.someclass;",
                "                       ^",
                "  To fix: ",
                "  Rename \"someclass\" to \"Someclass\"."));
  }

  @Test
  public void testStarImportedTypeFromClasspathFails() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            Joiner.on('\n').join("package com.facebook.bar;", "public class Bar { }")));
    testCompiler.setAllowCompilationErrors(true);
    compileWithValidation(
        Joiner.on('\n')
            .join("import com.facebook.bar.*;", "public abstract class Foo extends Bar { }"));

    assertErrors(
        Joiner.on('\n')
            .join(
                "Foo.java:2: error: Source-only ABI generation requires that this type be"
                    + " explicitly imported (star imports are not accepted).",
                "public abstract class Foo extends Bar { }",
                "                                  ^",
                "  To fix: ",
                "  Add an import for \"com.facebook.bar.Bar\""));
  }

  @Test
  public void testStarImportedTypeCompiledTogetherSucceeds() throws IOException {
    compileWithValidation(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            Joiner.on('\n').join("package com.facebook.bar;", "public class Bar { }"),
            "Foo.java",
            Joiner.on('\n')
                .join("import com.facebook.bar.*;", "public abstract class Foo extends Bar { }")));

    assertNoErrors();
  }

  @Test
  public void testStarImportedTypeFromBootClasspathSucceeds() throws IOException {
    this.compileWithValidation(
        Joiner.on('\n')
            .join("import java.util.*;", "public abstract class Foo implements List<String> { }"));

    assertNoErrors();
  }

  @Test
  public void testStaticImportedMemberWithMissingSuperclassFails() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/base/Base.java",
            Joiner.on('\n').join("package com.facebook.base;", "public class Base {", "}")));

    testCompiler.setAllowCompilationErrors(true);
    this.compileWithValidation(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.bar;",
                    "public class Bar extends com.facebook.base.Base {",
                    "  public static void foo() { }",
                    "}"),
            "com/facebook/foo/Foo.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.foo;",
                    "import static com.facebook.bar.Bar.foo;",
                    "public class Foo {",
                    "}")));

    assertError(
        "Foo.java:2: error: Source-only ABI generation requires that this type be unavailable, or"
            + " that all of its superclasses/interfaces be available.\n"
            + "import static com.facebook.bar.Bar.foo;\n"
            + "                              ^\n"
            + "  To fix, add the following rules to source_only_abi_deps in //:rule:"
            + " //com/facebook/base:base");
  }

  @Test
  public void testCanonicallyStaticImportedTypeFails() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.bar;",
                    "import com.facebook.baz.Baz;",
                    "public class Bar extends Baz { }"),
            "com/facebook/baz/Baz.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.baz;",
                    "public class Baz { public static class Inner { } }")));

    testCompiler.setAllowCompilationErrors(true);
    this.compileWithValidation(
        Joiner.on('\n')
            .join(
                "package com.facebook.foo;",
                "import static com.facebook.baz.Baz.Inner;",
                "public class Foo {",
                "  Inner i;",
                "}"));

    assertErrors(
        "Foo.java:4: error: Source-only ABI generation requires that this type be unavailable, or"
            + " that all of its superclasses/interfaces be available.\n"
            + "  Inner i;\n"
            + "  ^\n"
            + "  To fix, add the following rules to source_only_abi_deps in //:rule:"
            + " //com/facebook/baz:baz");
  }

  @Test
  public void testNonCanonicallyStaticImportedTypeFails() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.bar;",
                    "import com.facebook.baz.Baz;",
                    "public class Bar extends Baz { }"),
            "com/facebook/baz/Baz.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.baz;",
                    "public class Baz { public static class Inner { } }")));
    testCompiler.setAllowCompilationErrors(true);
    this.compileWithValidation(
        Joiner.on('\n')
            .join(
                "package com.facebook.foo;",
                "import static com.facebook.bar.Bar.Inner;",
                "public class Foo {",
                "  Inner i;",
                "}"));

    assertErrors(
        "Foo.java:4: error: Source-only ABI generation requires that this type be unavailable, or"
            + " that all of its superclasses/interfaces be available.\n"
            + "  Inner i;\n"
            + "  ^\n"
            + "  To fix, add the following rules to source_only_abi_deps in //:rule:"
            + " //com/facebook/baz:baz");
  }

  @Test
  public void testOwnMemberTypeSucceeds() throws IOException {
    compileWithValidation(
        Joiner.on('\n')
            .join(
                "package com.facebook.foo;",
                "public class Foo {",
                "  Inner i;",
                "  class Inner { }",
                "}"));

    assertNoErrors();
  }

  @Test
  public void testSuperclassMemberTypeFromClasspathFails() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.bar;",
                    "import com.facebook.foo.Baz;",
                    "public class Bar implements Baz { }"),
            "com/facebook/foo/Baz.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.foo;", "public interface Baz { interface Inner { } }")));
    testCompiler.setAllowCompilationErrors(true);
    compileWithValidation(
        ImmutableMap.of(
            "Foo.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.foo;",
                    "import com.facebook.bar.Bar;",
                    "class Foo extends Bar {",
                    "  Inner i;",
                    "}")));

    assertError(
        Joiner.on('\n')
            .join(
                "Foo.java:4: error: Source-only ABI generation requires that this member type"
                    + " reference be more explicit.",
                "  Inner i;",
                "  ^",
                "  To fix: ",
                "  Add an import for \"com.facebook.foo.Baz\"",
                "  Use \"Baz.Inner\" here instead of \"Inner\"."));
  }

  @Test
  public void testBootClasspathMemberThroughMissingInterfaceFails() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.bar;",
                    "public interface Bar extends java.util.Map<String, String> { }")));
    testCompiler.setAllowCompilationErrors(true);
    compileWithValidation(
        ImmutableMap.of(
            "Foo.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.foo;",
                    "import com.facebook.bar.Bar;",
                    "abstract class Foo implements Bar {",
                    "  Entry<String, String> entry;",
                    "}")));

    assertError(
        Joiner.on('\n')
            .join(
                "Foo.java:4: error: Source-only ABI generation requires that this member type"
                    + " reference be more explicit.",
                "  Entry<String, String> entry;",
                "  ^",
                "  To fix: ",
                "  Add an import for \"java.util.Map\"",
                "  Use \"Map.Entry\" here instead of \"Entry\"."));
  }

  @Test
  public void testStaticImportThroughMissingSuperclassReportsProvider() throws IOException {
    taskListenerFactory.setInheritedMemberTypesOnly(true);
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public class Bar extends java.lang.Thread { }"));
    testCompiler.setAllowCompilationErrors(true);
    compileWithValidation(
        Joiner.on('\n')
            .join(
                "package com.facebook.foo;",
                "import static com.facebook.bar.Bar.*;",
                "class Foo extends com.facebook.bar.Bar {",
                "  State state;",
                "}"));
    assertError(
        Joiner.on('\n')
            .join(
                "Foo.java:4: error: Inherited member type java.lang.Thread.State requires missing"
                    + " supertypes during source-only ABI generation.",
                "  State state;",
                "  ^",
                "  Use the fully qualified type or add the providers of the following classes to"
                    + " source_only_abi_deps in //:rule.",
                "  KOSABI_JASABI_MISSING_SUPERTYPES: com.facebook.bar.Bar"));
  }

  @Test
  public void testStaticImportCannotSupplyHiddenMember() throws IOException {
    taskListenerFactory.setInheritedMemberTypesOnly(true);
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public class Bar extends java.lang.Thread { }",
            "com/facebook/shadow/Shadow.java",
            "package com.facebook.shadow; public class Shadow extends java.lang.Thread { public"
                + " static class State {} }"));
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/shadow:shadow");
    testCompiler.setAllowCompilationErrors(true);
    compileWithValidation(
        Joiner.on('\n')
            .join(
                "package com.facebook.foo;",
                "import static com.facebook.shadow.Shadow.*;",
                "class Foo extends com.facebook.bar.Bar {",
                "  State state;",
                "}"));
    assertError(
        Joiner.on('\n')
            .join(
                "Foo.java:4: error: Inherited member type java.lang.Thread.State requires missing"
                    + " supertypes during source-only ABI generation.",
                "  State state;",
                "  ^",
                "  Use the fully qualified type or add the providers of the following classes to"
                    + " source_only_abi_deps in //:rule.",
                "  KOSABI_JASABI_MISSING_SUPERTYPES: com.facebook.bar.Bar"));
  }

  @Test
  public void testStaticImportCannotSupplyInaccessibleMember() throws IOException {
    taskListenerFactory.setInheritedMemberTypesOnly(true);
    withClasspath(
        ImmutableMap.of(
            "com/facebook/base/Base.java",
            "package com.facebook.base; public class Base { protected static class Inner {} }",
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public class Bar extends com.facebook.base.Base {}",
            "com/facebook/visible/Visible.java",
            "package com.facebook.visible; public class Visible extends com.facebook.base.Base"
                + " {}"));
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/base:base");
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/visible:visible");
    testCompiler.setAllowCompilationErrors(true);
    compileWithValidation(
        Joiner.on('\n')
            .join(
                "package com.facebook.foo;",
                "import static com.facebook.visible.Visible.*;",
                "class Foo extends com.facebook.bar.Bar {",
                "  Inner inner;",
                "}"));
    assertError(
        Joiner.on('\n')
            .join(
                "Foo.java:4: error: Inherited member type com.facebook.base.Base.Inner requires"
                    + " missing supertypes during source-only ABI generation.",
                "  Inner inner;",
                "  ^",
                "  Use the fully qualified type or add the providers of the following classes to"
                    + " source_only_abi_deps in //:rule.",
                "  KOSABI_JASABI_MISSING_SUPERTYPES: com.facebook.bar.Bar"));
  }

  @Test
  public void testLexicalOuterCannotSupplyHiddenMember() throws IOException {
    taskListenerFactory.setInheritedMemberTypesOnly(true);
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public class Bar extends java.lang.Thread {}"));
    testCompiler.setAllowCompilationErrors(true);
    compileWithValidation(
        ImmutableMap.of(
            "Outer.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.foo;",
                    "class Outer extends Thread {",
                    "  static class State {}",
                    "  class Child extends com.facebook.bar.Bar {",
                    "    State state;",
                    "  }",
                    "}")));
    assertSameType(
        elements.getTypeElement("java.lang.Thread.State").asType(),
        findField("state", elements.getTypeElement("com.facebook.foo.Outer.Child")).asType());
    assertError(
        Joiner.on('\n')
            .join(
                "Outer.java:5: error: Inherited member type java.lang.Thread.State requires missing"
                    + " supertypes during source-only ABI generation.",
                "    State state;",
                "    ^",
                "  Use the fully qualified type or add the providers of the following classes to"
                    + " source_only_abi_deps in //:rule.",
                "  KOSABI_JASABI_MISSING_SUPERTYPES: com.facebook.bar.Bar"));
  }

  @Test
  public void testLexicalOuterCanSupplySameInheritedMember() throws IOException {
    taskListenerFactory.setInheritedMemberTypesOnly(true);
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public class Bar extends java.lang.Thread {}"));
    compileWithValidation(
        ImmutableMap.of(
            "Outer.java",
            "package com.facebook.foo; class Outer extends Thread { class Child extends"
                + " com.facebook.bar.Bar { State state; } }"));
    assertSameType(
        elements.getTypeElement("java.lang.Thread.State").asType(),
        findField("state", elements.getTypeElement("com.facebook.foo.Outer.Child")).asType());
    assertNoErrors();
  }

  @Test
  public void testLexicalOuterCanSupplyPrivateMember() throws IOException {
    taskListenerFactory.setInheritedMemberTypesOnly(true);
    compileWithValidation(
        "class Foo { private static class Value {} class Child { public Value value; } }");
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberQualifiedRootRequiresProvider() throws IOException {
    withQualifiedMemberHierarchy();
    testCompiler.setAllowCompilationErrors(true);
    compileQualifiedMember("Outer.Inner");
    assertMissingInheritedBar();
  }

  @Test
  public void testInheritedMemberQualifiedRootWithProviderSucceeds() throws IOException {
    withQualifiedMemberHierarchy();
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/bar:bar");
    compileQualifiedMember("Outer.Inner");
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberQualifiedCanonicalOwnerSucceeds() throws IOException {
    withQualifiedMemberHierarchy();
    compileQualifiedMember("com.facebook.base.Base.Outer.Inner");
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberQualifiedImportedRootSucceeds() throws IOException {
    withQualifiedMemberHierarchy();
    compileWithValidation(
        "package com.facebook.foo; import com.facebook.base.Base.Outer;"
            + " public class Foo { public Outer.Inner field; }");
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberQualifiedContainerRequiresProvider() throws IOException {
    withQualifiedMemberHierarchy();
    testCompiler.setAllowCompilationErrors(true);
    compileQualifiedMember("com.facebook.bar.Bar.Outer.Inner");
    assertMissingInheritedBar();
  }

  @Test
  public void testInheritedMemberQualifiedContainerWithProviderSucceeds() throws IOException {
    withQualifiedMemberHierarchy();
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/bar:bar");
    compileQualifiedMember("com.facebook.bar.Bar.Outer.Inner");
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberQualifiedUnavailableDeclaringOwnerSucceeds() throws IOException {
    taskListenerFactory.setInheritedMemberTypesOnly(true);
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public class Bar { public static class Inner {} }"));
    compileWithValidation(
        "package com.facebook.foo; public class Foo { public com.facebook.bar.Bar.Inner field; }");
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberShadowedLexicalFallbackRequiresProvider() throws IOException {
    withLexicalMemberHierarchy();
    testCompiler.setAllowCompilationErrors(true);
    compileShadowedLexicalMember("State");
    assertMissingInheritedBar();
  }

  @Test
  public void testInheritedMemberShadowedLexicalFallbackWithProviderSucceeds() throws IOException {
    withLexicalMemberHierarchy();
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/bar:bar");
    compileShadowedLexicalMember("State");
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberShadowedLexicalFallbackExplicitTypeSucceeds() throws IOException {
    withLexicalMemberHierarchy();
    compileShadowedLexicalMember("java.lang.Thread.State");
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberUnavailableShadowPreservesOuterFallback() throws IOException {
    withLexicalMemberHierarchy();
    compileInheritedShadowMember();
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberAvailableShadowBlocksOuterFallback() throws IOException {
    withLexicalMemberHierarchy();
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/shadow:shadow");
    testCompiler.setAllowCompilationErrors(true);
    compileInheritedShadowMember();
    assertMissingInheritedBar();
  }

  @Test
  public void testInheritedMemberTypeParameterBlocksOuterFallback() throws IOException {
    withLexicalMemberHierarchy();
    testCompiler.setAllowCompilationErrors(true);
    compileTypeParameterShadowMember("", "State");
    assertMissingInheritedBar();
  }

  @Test
  public void testInheritedMemberTypeParameterWithProviderSucceeds() throws IOException {
    withLexicalMemberHierarchy();
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/bar:bar");
    compileTypeParameterShadowMember("", "State");
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberTypeParameterExplicitTypeSucceeds() throws IOException {
    withLexicalMemberHierarchy();
    compileTypeParameterShadowMember("", "java.lang.Thread.State");
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberTypeParameterBlocksSameScopeCanonicalPath() throws IOException {
    withLexicalMemberHierarchy();
    testCompiler.setAllowCompilationErrors(true);
    compileTypeParameterShadowMember("extends Thread", "State");
    assertMissingInheritedBar();
  }

  @Test
  public void testInheritedMemberSingleTypeImportDoesNotBypassLexicalShadow() throws IOException {
    withLexicalMemberHierarchy();
    testCompiler.setAllowCompilationErrors(true);
    compileImportedShadowedMember("import java.lang.Thread.State;");
    assertMissingInheritedBar();
  }

  @Test
  public void testInheritedMemberOnDemandImportDoesNotBypassLexicalShadow() throws IOException {
    withLexicalMemberHierarchy();
    testCompiler.setAllowCompilationErrors(true);
    compileImportedShadowedMember("import java.lang.Thread.*;");
    assertMissingInheritedBar();
  }

  @Test
  public void testInheritedMemberImportedLexicalShadowWithProviderSucceeds() throws IOException {
    withLexicalMemberHierarchy();
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/bar:bar");
    compileImportedShadowedMember("import java.lang.Thread.State;");
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberShadowBehindMissingCanonicalPathRequiresProvider()
      throws IOException {
    withCompetingMemberHierarchy();
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/shadow:shadow");
    testCompiler.setAllowCompilationErrors(true);
    compileCompetingInheritedMember("State");
    assertMissingInheritedBar();
  }

  @Test
  public void testInheritedMemberUnavailableCompetingShadowPreservesOuterFallback()
      throws IOException {
    withCompetingMemberHierarchy();
    compileCompetingInheritedMember("State");
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberCompetingShadowWithProviderSucceeds() throws IOException {
    withCompetingMemberHierarchy();
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/shadow:shadow");
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/bar:bar");
    compileCompetingInheritedMember("State");
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberCompetingShadowExplicitTypeSucceeds() throws IOException {
    withCompetingMemberHierarchy();
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/shadow:shadow");
    compileCompetingInheritedMember("java.lang.Thread.State");
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberSamePackageTypeBlocksStaticOnDemandFallback() throws IOException {
    withImportedMemberHierarchy();
    testCompiler.setAllowCompilationErrors(true);
    compileSamePackageShadowMember("");
    assertMissingInheritedBar();
  }

  @Test
  public void testInheritedMemberSamePackageShadowWithProviderSucceeds() throws IOException {
    withImportedMemberHierarchy();
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/bar:bar");
    compileSamePackageShadowMember("");
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberSingleTypeImportBlocksStaticOnDemandFallback() throws IOException {
    withImportedMemberHierarchy();
    testCompiler.setAllowCompilationErrors(true);
    compileImportedMember("import com.facebook.other.State;");
    assertMissingInheritedBar();
  }

  @Test
  public void testInheritedMemberSingleStaticImportBlocksStaticOnDemandFallback()
      throws IOException {
    withImportedMemberHierarchy();
    testCompiler.setAllowCompilationErrors(true);
    compileImportedMember("import static com.facebook.shadow.Shadow.State;");
    assertMissingInheritedBar();
  }

  @Test
  public void testInheritedMemberSingleStaticShadowWithProviderSucceeds() throws IOException {
    withImportedMemberHierarchy();
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/bar:bar");
    compileImportedMember("import static com.facebook.shadow.Shadow.State;");
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberCanonicalImportPrecedesSamePackageShadow() throws IOException {
    withImportedMemberHierarchy();
    compileSamePackageShadowMember("import java.lang.Thread.State;");
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberCanonicalStaticImportPrecedesSamePackageShadow()
      throws IOException {
    withImportedMemberHierarchy();
    compileSamePackageShadowMember("import static com.facebook.visible.Visible.State;");
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberUnavailablePackageShadowPreservesStaticFallback()
      throws IOException {
    withImportedMemberHierarchy();
    compileImportedMember("");
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberAvailablePackageShadowBlocksStaticFallback() throws IOException {
    withImportedMemberHierarchy();
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/foo:foo");
    testCompiler.setAllowCompilationErrors(true);
    compileImportedMember("");
    assertMissingInheritedBar();
  }

  @Test
  public void testInheritedMemberSingleStaticFieldDoesNotBlockTypeFallback() throws IOException {
    withImportedMemberHierarchy();
    compileImportedMember("import static com.facebook.visible.Visible.StateField.State;");
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberNamedTypeImportSurvivesLaterFieldImport() throws IOException {
    withImportedMemberHierarchy();
    compileSamePackageShadowMember(
        "import static com.facebook.visible.Visible.State;"
            + " import static com.facebook.visible.Visible.StateField.State;");
    assertNoErrors();
  }

  private void compileTypeParameterShadowMember(String middleSuperclass, String fieldType)
      throws IOException {
    compileWithValidation(
        "package com.facebook.foo; class Outer extends Thread { static class Middle<State> "
            + middleSuperclass
            + " { class Child extends com.facebook.bar.Bar { public "
            + fieldType
            + " state; } } }");
    assertThreadStateField();
  }

  private void compileImportedShadowedMember(String typeImport) throws IOException {
    compileWithValidation(
        "package com.facebook.foo; "
            + typeImport
            + " class Outer extends Thread { static class Middle { static class State {}"
            + " class Child extends com.facebook.bar.Bar { public State state; } } }");
    assertThreadStateField();
  }

  private void withCompetingMemberHierarchy() throws IOException {
    taskListenerFactory.setInheritedMemberTypesOnly(true);
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public class Bar extends java.lang.Thread {}",
            "com/facebook/shadow/ShadowIface.java",
            "package com.facebook.shadow; public interface ShadowIface { class State {} }"));
  }

  private void compileCompetingInheritedMember(String fieldType) throws IOException {
    compileWithValidation(
        "package com.facebook.foo; class Outer extends Thread {"
            + " static class Middle extends com.facebook.bar.Bar"
            + " implements com.facebook.shadow.ShadowIface {"
            + " class Child extends com.facebook.bar.Bar { public "
            + fieldType
            + " state; } } }");
    assertThreadStateField();
  }

  private void withImportedMemberHierarchy() throws IOException {
    taskListenerFactory.setInheritedMemberTypesOnly(true);
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public class Bar extends java.lang.Thread {}",
            "com/facebook/visible/Visible.java",
            "package com.facebook.visible; public class Visible<State> extends java.lang.Thread {"
                + " public static class StateField { public static int State; } }",
            "com/facebook/other/State.java",
            "package com.facebook.other; public class State {}",
            "com/facebook/shadow/Shadow.java",
            "package com.facebook.shadow; public class Shadow { public static class State {} }",
            "com/facebook/foo/State.java",
            "package com.facebook.foo; public class State {}"));
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/visible:visible");
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/other:other");
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/shadow:shadow");
  }

  private void compileImportedMember(String typeImport) throws IOException {
    compileWithValidation(importedMemberSource(typeImport));
    assertImportedThreadStateField();
  }

  private void compileSamePackageShadowMember(String typeImport) throws IOException {
    compileWithValidation(
        ImmutableMap.of(
            "Foo.java",
            importedMemberSource(typeImport),
            "State.java",
            "package com.facebook.foo; public class State {}"));
    assertImportedThreadStateField();
  }

  private String importedMemberSource(String typeImport) {
    return "package com.facebook.foo; import static com.facebook.visible.Visible.*; "
        + typeImport
        + " public class Foo extends com.facebook.bar.Bar { public State state; }";
  }

  private void assertImportedThreadStateField() {
    assertSameType(
        elements.getTypeElement("java.lang.Thread.State").asType(),
        findField("state", elements.getTypeElement("com.facebook.foo.Foo")).asType());
  }

  @Test
  public void testIntactLexicalPathMustNotBypassAvailableCompetingType() throws IOException {
    withIntactMatchingBranchHierarchy(true, false);
    testCompiler.setAllowCompilationErrors(true);
    compileIntactLexicalMatchingBranch();
    assertMissingInheritedBar();
  }

  @Test
  public void testIntactLexicalPathWithoutCompetingProviderSucceeds() throws IOException {
    withIntactMatchingBranchHierarchy(false, false);
    compileIntactLexicalMatchingBranch();
    assertNoErrors();
  }

  @Test
  public void testInnerProviderPrecedesAmbiguousLexicalFallback() throws IOException {
    withIntactMatchingBranchHierarchy(true, true);
    compileIntactLexicalMatchingBranch();
    assertNoErrors();
  }

  @Test
  public void testStaticStarFallbackMustNotIgnoreAvailableCompetingType() throws IOException {
    withIntactMatchingBranchHierarchy(true, false);
    testCompiler.setAllowCompilationErrors(true);
    compileIntactStarMatchingBranch(false, false);
    assertMissingInheritedBar();
  }

  @Test
  public void testStaticStarFallbackWithoutCompetingProviderSucceeds() throws IOException {
    withIntactMatchingBranchHierarchy(false, false);
    compileIntactStarMatchingBranch(false, false);
    assertNoErrors();
  }

  @Test
  public void testInnerProviderPrecedesAmbiguousStaticStarFallback() throws IOException {
    withIntactMatchingBranchHierarchy(true, true);
    compileIntactStarMatchingBranch(false, false);
    assertNoErrors();
  }

  @Test
  public void testMixedStarFallbackMustNotIgnoreAvailableCompetingType() throws IOException {
    withIntactMatchingBranchHierarchy(true, false);
    testCompiler.setAllowCompilationErrors(true);
    compileIntactStarMatchingBranch(true, false);
    assertMissingInheritedBar();
  }

  @Test
  public void testMixedStarFallbackWithoutCompetingProviderSucceeds() throws IOException {
    withIntactMatchingBranchHierarchy(false, false);
    compileIntactStarMatchingBranch(true, false);
    assertNoErrors();
  }

  @Test
  public void testInnerProviderPrecedesAmbiguousMixedStarFallback() throws IOException {
    withIntactMatchingBranchHierarchy(true, true);
    compileIntactStarMatchingBranch(true, false);
    assertNoErrors();
  }

  @Test
  public void testOrdinaryStarFallbackMustNotIgnoreAvailableCompetingType() throws IOException {
    withIntactMatchingBranchHierarchy(true, false);
    testCompiler.setAllowCompilationErrors(true);
    compileIntactStarMatchingBranch(true, true);
    assertMissingInheritedBar();
  }

  @Test
  public void testOrdinaryStarFallbackWithoutCompetingProviderSucceeds() throws IOException {
    withIntactMatchingBranchHierarchy(false, false);
    compileIntactStarMatchingBranch(true, true);
    assertNoErrors();
  }

  @Test
  public void testInnerProviderPrecedesAmbiguousOrdinaryStarFallback() throws IOException {
    withIntactMatchingBranchHierarchy(true, true);
    compileIntactStarMatchingBranch(true, true);
    assertNoErrors();
  }

  @Test
  public void testPackageStarFallbackMustNotIgnoreAvailableCompetingType() throws IOException {
    withIntactMatchingBranchHierarchy(true, false);
    testCompiler.setAllowCompilationErrors(true);
    compilePackageStarMatchingBranch();
    assertMissingInheritedBar();
  }

  @Test
  public void testPackageStarFallbackWithoutCompetingProviderSucceeds() throws IOException {
    withIntactMatchingBranchHierarchy(false, false);
    compilePackageStarMatchingBranch();
    assertNoErrors();
  }

  @Test
  public void testInnerProviderPrecedesAmbiguousPackageStarFallback() throws IOException {
    withIntactMatchingBranchHierarchy(true, true);
    compilePackageStarMatchingBranch();
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberHeaderReferenceUsesEnclosingScopePath() throws IOException {
    taskListenerFactory.setInheritedMemberTypesOnly(true);
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public class Bar extends java.lang.Thread {}",
            "com/facebook/baz/Baz.java",
            "package com.facebook.baz; public class Baz extends java.lang.Thread {}"));
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/bar:bar");
    testCompiler.setAllowCompilationErrors(true);
    compileWithValidation(
        "package com.facebook.foo; class Outer extends com.facebook.baz.Baz {"
            + " static abstract class Child extends com.facebook.bar.Bar"
            + " implements Comparable<State> {} }");
    javax.lang.model.element.TypeElement child =
        elements.getTypeElement("com.facebook.foo.Outer.Child");
    assertSameType(
        elements.getTypeElement("java.lang.Thread.State").asType(),
        ((javax.lang.model.type.DeclaredType) child.getInterfaces().get(0))
            .getTypeArguments()
            .get(0));
    assertMissingInheritedProviders("com.facebook.baz.Baz");
  }

  @Test
  public void testInheritedMemberHeaderReferenceIgnoresOwnCompetingMember() throws IOException {
    withCompetingMemberHierarchy();
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/shadow:shadow");
    compileWithValidation(
        "package com.facebook.foo; class Outer extends Thread {"
            + " static abstract class Child extends com.facebook.bar.Bar"
            + " implements com.facebook.shadow.ShadowIface, Comparable<State> {} }");
    javax.lang.model.element.TypeElement child =
        elements.getTypeElement("com.facebook.foo.Outer.Child");
    assertSameType(
        elements.getTypeElement("java.lang.Thread.State").asType(),
        ((javax.lang.model.type.DeclaredType) child.getInterfaces().get(1))
            .getTypeArguments()
            .get(0));
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberCompetingScopeDoesNotOfferItsOwnRepair() throws IOException {
    assertCompetingScopeRepair(true);
  }

  @Test
  public void testInheritedMemberCompetingScopeDoesNotOfferShorterMissingRepair()
      throws IOException {
    assertCompetingScopeRepair(false);
  }

  private void assertCompetingScopeRepair(boolean middleProviderAvailable) throws IOException {
    taskListenerFactory.setInheritedMemberTypesOnly(true);
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public class Bar extends java.lang.Thread {}",
            "com/facebook/bar2/Bar2.java",
            "package com.facebook.bar2; public class Bar2 extends java.lang.Thread {}",
            "com/facebook/bar3/Bar3.java",
            "package com.facebook.bar3; public class Bar3 extends com.facebook.bar2.Bar2 {}",
            "com/facebook/shadow/ShadowIface.java",
            "package com.facebook.shadow; public interface ShadowIface { class State {} }"));
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/shadow:shadow");
    if (middleProviderAvailable) {
      taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/bar:bar");
    }
    testCompiler.setAllowCompilationErrors(true);
    compileWithValidation(
        "package com.facebook.foo; class Outer extends Thread {"
            + " static class Middle extends com.facebook.bar.Bar"
            + " implements com.facebook.shadow.ShadowIface {"
            + " class Child extends com.facebook.bar3.Bar3 { public State state; } } }");
    assertThreadStateField();
    assertMissingInheritedProviders("com.facebook.bar2.Bar2, com.facebook.bar3.Bar3");
  }

  @Test
  public void testNamedStaticFallbackIgnoresUnavailableCompetingType() throws IOException {
    withIntactMatchingBranchHierarchy(false, false);
    compileNamedStaticMatchingBranch();
    assertNoErrors();
  }

  @Test
  public void testNamedStaticFallbackRejectsAvailableCompetingType() throws IOException {
    withIntactMatchingBranchHierarchy(true, false);
    testCompiler.setAllowCompilationErrors(true);
    compileNamedStaticMatchingBranch();
    assertMissingInheritedBar();
  }

  @Test
  public void testInnerProviderPrecedesAmbiguousNamedStaticFallback() throws IOException {
    withIntactMatchingBranchHierarchy(true, true);
    compileNamedStaticMatchingBranch();
    assertNoErrors();
  }

  private void withIntactMatchingBranchHierarchy(boolean shadowAvailable, boolean barAvailable)
      throws IOException {
    taskListenerFactory.setInheritedMemberTypesOnly(true);
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public class Bar extends java.lang.Thread {}",
            "com/facebook/visible/Visible.java",
            "package com.facebook.visible; public class Visible extends java.lang.Thread {}",
            "com/facebook/visible/Mixed.java",
            "package com.facebook.visible; public class Mixed extends java.lang.Thread"
                + " implements com.facebook.shadow.ShadowIface {}",
            "com/facebook/shadow/ShadowIface.java",
            "package com.facebook.shadow; public interface ShadowIface { class State {} }",
            "com/facebook/shadow/State.java",
            "package com.facebook.shadow; public class State {}"));
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/visible:visible");
    if (shadowAvailable) {
      taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/shadow:shadow");
    }
    if (barAvailable) {
      taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/bar:bar");
    }
  }

  private void compileIntactLexicalMatchingBranch() throws IOException {
    compileWithValidation(
        "package com.facebook.foo; class Outer extends Thread {"
            + " static class Middle extends com.facebook.visible.Visible"
            + " implements com.facebook.shadow.ShadowIface {"
            + " class Child extends com.facebook.bar.Bar { public State state; } } }");
    assertThreadStateField();
  }

  private void compileIntactStarMatchingBranch(
      boolean ordinaryMatchingImport, boolean ordinaryShadowImport) throws IOException {
    String matchingImport =
        ordinaryMatchingImport ? "import java.lang.Thread.*;" : "import static java.lang.Thread.*;";
    String shadowImport =
        ordinaryShadowImport
            ? "import com.facebook.shadow.ShadowIface.*;"
            : "import static com.facebook.shadow.ShadowIface.*;";
    compileWithValidation(
        "package com.facebook.foo; "
            + matchingImport
            + shadowImport
            + " public class Foo extends com.facebook.bar.Bar { public State state; }");
    assertImportedThreadStateField();
  }

  private void compilePackageStarMatchingBranch() throws IOException {
    compileWithValidation(
        "package com.facebook.foo; import static java.lang.Thread.*;"
            + " import com.facebook.shadow.*;"
            + " public class Foo extends com.facebook.bar.Bar { public State state; }");
    assertImportedThreadStateField();
  }

  private void compileNamedStaticMatchingBranch() throws IOException {
    compileWithValidation(
        "package com.facebook.foo; import static com.facebook.visible.Mixed.State;"
            + " public class Foo extends com.facebook.bar.Bar { public State state; }");
    assertImportedThreadStateField();
  }

  private void withQualifiedMemberHierarchy() throws IOException {
    taskListenerFactory.setInheritedMemberTypesOnly(true);
    withClasspath(
        ImmutableMap.of(
            "com/facebook/base/Base.java",
            "package com.facebook.base; public interface Base {"
                + " interface Outer { interface Inner {} } }",
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public interface Bar extends com.facebook.base.Base {}"));
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/base:base");
  }

  private void compileQualifiedMember(String fieldType) throws IOException {
    compileWithValidation(
        "package com.facebook.foo; import com.facebook.bar.Bar;"
            + " public class Foo implements Bar { public "
            + fieldType
            + " field; }");
    assertSameType(
        elements.getTypeElement("com.facebook.base.Base.Outer.Inner").asType(),
        findField("field", elements.getTypeElement("com.facebook.foo.Foo")).asType());
  }

  private void withLexicalMemberHierarchy() throws IOException {
    taskListenerFactory.setInheritedMemberTypesOnly(true);
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public class Bar extends java.lang.Thread {}",
            "com/facebook/shadow/Shadow.java",
            "package com.facebook.shadow; public class Shadow { public static class State {} }"));
  }

  private void compileShadowedLexicalMember(String fieldType) throws IOException {
    compileWithValidation(
        "package com.facebook.foo; class Outer extends Thread { static class Middle {"
            + " static class State {} class Child extends com.facebook.bar.Bar { public "
            + fieldType
            + " state; } } }");
    assertThreadStateField();
  }

  private void compileInheritedShadowMember() throws IOException {
    compileWithValidation(
        "package com.facebook.foo; class Outer extends Thread {"
            + " static class Middle extends com.facebook.shadow.Shadow {"
            + " class Child extends com.facebook.bar.Bar { public State state; } } }");
    assertThreadStateField();
  }

  private void assertThreadStateField() {
    assertSameType(
        elements.getTypeElement("java.lang.Thread.State").asType(),
        findField("state", elements.getTypeElement("com.facebook.foo.Outer.Middle.Child"))
            .asType());
  }

  private void assertMissingInheritedBar() {
    assertMissingInheritedProviders("com.facebook.bar.Bar");
  }

  private void assertMissingInheritedProviders(String expected) {
    assertEquals(1, testCompiler.getErrorMessages().size());
    String error = testCompiler.getErrorMessages().get(0);
    String marker = "KOSABI_JASABI_MISSING_SUPERTYPES: ";
    assertTrue(error, error.contains(marker));
    assertEquals(expected, error.substring(error.indexOf(marker) + marker.length()).trim());
  }

  @Test
  public void testInheritedMemberCheckNamesMissingProvider() throws IOException {
    taskListenerFactory.setInheritedMemberTypesOnly(true);
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public interface Bar extends java.util.Map<String, String> {"
                + " }"));
    testCompiler.setAllowCompilationErrors(true);
    compileWithValidation(
        Joiner.on('\n')
            .join(
                "package com.facebook.foo;",
                "import com.facebook.bar.Bar;",
                "abstract class Foo implements Bar {",
                "  Entry<String, String> entry;",
                "}"));

    assertError(
        Joiner.on('\n')
            .join(
                "Foo.java:4: error: Inherited member type java.util.Map.Entry requires missing"
                    + " supertypes during source-only ABI generation.",
                "  Entry<String, String> entry;",
                "  ^",
                "  Use the fully qualified type or add the providers of the following classes to"
                    + " source_only_abi_deps in //:rule.",
                "  KOSABI_JASABI_MISSING_SUPERTYPES: com.facebook.bar.Bar"));
  }

  @Test
  public void testInheritedMemberCheckPreservesAvailableDiamondPath() throws IOException {
    taskListenerFactory.setInheritedMemberTypesOnly(true);
    testBootClasspathMemberThroughAvailableDiamondBranchSucceeds();
  }

  @Test
  public void testInheritedMemberCheckPreservesQualifiedType() throws IOException {
    taskListenerFactory.setInheritedMemberTypesOnly(true);
    testQualifiedBootClasspathMemberThroughMissingInterfaceSucceeds();
  }

  @Test
  public void testInheritedMemberCheckPreservesImportedType() throws IOException {
    taskListenerFactory.setInheritedMemberTypesOnly(true);
    testImportedBootClasspathMemberThroughMissingInterfaceSucceeds();
  }

  @Test
  public void testInheritedMemberCheckSkipsPrivateImplementationClass() throws IOException {
    taskListenerFactory.setInheritedMemberTypesOnly(true);
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public interface Bar extends java.util.Map<String, String> {"
                + " }"));
    compileWithValidation(
        "import com.facebook.bar.Bar; class Foo { private abstract class Impl implements Bar {"
            + " public Entry<String, String> entry; } }");
    assertNoErrors();
  }

  @Test
  public void testInheritedMemberCheckDoesNotEnableLegacyAnnotationOrConstantChecks()
      throws IOException {
    taskListenerFactory.setInheritedMemberTypesOnly(true);
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public class Bar { public static final int VALUE = 1; }"));
    compileWithValidation(
        "import com.facebook.bar.Bar; @interface Foo { int value() default Bar.VALUE; }");
    assertNoErrors();
  }

  @Test
  public void testInheritedAnnotationSourceMethodIsDiscarded() throws IOException {
    withInheritedAnnotations();
    compileWithValidation(
        "import com.facebook.bar.Bar; class Foo implements Bar {"
            + " public @Source int number() { return 1; } }");
    assertNoErrors();
  }

  @Test
  public void testInheritedAnnotationSourceFieldAndParameterAreDiscarded() throws IOException {
    withInheritedAnnotations();
    compileWithValidation(
        "import com.facebook.bar.Bar; class Foo implements Bar {"
            + " @Source public int field; public void accept(@Source String value) {} }");
    assertNoErrors();
  }

  @Test
  public void testInheritedAnnotationSourceTypeUseIsDiscarded() throws IOException {
    withInheritedAnnotations();
    compileWithValidation(
        "import com.facebook.bar.Bar; class Foo implements Bar {"
            + " public java.util.List<@SourceType String> values; }");
    assertNoErrors();
  }

  @Test
  public void testInheritedAnnotationSourceTypeParameterIsDiscarded() throws IOException {
    withInheritedAnnotations();
    compileWithValidation(
        "import com.facebook.bar.Bar; class Foo implements Bar {"
            + " public <@SourceParameter T> void accept(T value) {} }");
    assertNoErrors();
  }

  @Test
  public void testInheritedAnnotationSourceContainerDiscardsNestedValue() throws IOException {
    withInheritedAnnotations();
    compileWithValidation(
        "import com.facebook.bar.Bar; class Foo implements Bar {"
            + " @Bar.SourceContainer(@Retained) public int number() { return 1; } }");
    assertNoErrors();
  }

  @Test
  public void testInheritedAnnotationClassRetentionRemainsChecked() throws IOException {
    assertInheritedAnnotationRejected("@Retained public int number() { return 1; }", "Retained");
  }

  @Test
  public void testInheritedAnnotationRuntimeRetentionRemainsChecked() throws IOException {
    assertInheritedAnnotationRejected("@Runtime public int number() { return 1; }", "Runtime");
  }

  @Test
  public void testInheritedAnnotationDefaultRetentionRemainsChecked() throws IOException {
    assertInheritedAnnotationRejected("@Default public int number() { return 1; }", "Default");
  }

  @Test
  public void testInheritedAnnotationSourceDescriptorRemainsChecked() throws IOException {
    assertInheritedAnnotationRejected("public Source value;", "Source");
  }

  @Test
  public void testInheritedAnnotationSourceMethodStillChecksReturnType() throws IOException {
    assertInheritedAnnotationRejected(
        "@Bar.Source public Source value() { return null; }", "Source");
  }

  @Test
  public void testInheritedAnnotationRetainedContainerKeepsSourceValue() throws IOException {
    assertInheritedAnnotationRejected(
        "@Bar.RetainedContainer(@Source) public int number() { return 1; }", "Source");
  }

  @Test
  public void testInheritedAnnotationDefaultKeepsSourceValue() throws IOException {
    assertInheritedAnnotationRejected(
        "public @interface Defaults { Bar.Source value() default @Source; }", "Source");
  }

  private void withInheritedAnnotations() throws IOException {
    taskListenerFactory.setInheritedMemberTypesOnly(true);
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.bar;",
                    "import java.lang.annotation.*;",
                    "public interface Bar {",
                    "  @Retention(RetentionPolicy.SOURCE) @interface Source {}",
                    "  @Retention(RetentionPolicy.CLASS) @interface Retained {}",
                    "  @Retention(RetentionPolicy.RUNTIME) @interface Runtime {}",
                    "  @interface Default {}",
                    "  @Retention(RetentionPolicy.SOURCE) @Target(ElementType.TYPE_USE)",
                    "  @interface SourceType {}",
                    "  @Retention(RetentionPolicy.SOURCE) @Target(ElementType.TYPE_PARAMETER)",
                    "  @interface SourceParameter {}",
                    "  @Retention(RetentionPolicy.SOURCE)",
                    "  @interface SourceContainer { Retained value(); }",
                    "  @Retention(RetentionPolicy.RUNTIME)",
                    "  @interface RetainedContainer { Source value(); }",
                    "}")));
  }

  private void assertInheritedAnnotationRejected(String declaration, String annotation)
      throws IOException {
    withInheritedAnnotations();
    testCompiler.setAllowCompilationErrors(true);
    compileWithValidation(
        "import com.facebook.bar.Bar; class Foo implements Bar { " + declaration + " }");
    assertEquals(1, testCompiler.getErrorMessages().size());
    String error = testCompiler.getErrorMessages().get(0);
    assertTrue(error, error.contains("Inherited member type com.facebook.bar.Bar." + annotation));
    assertTrue(error, error.contains("KOSABI_JASABI_MISSING_SUPERTYPES: com.facebook.bar.Bar"));
  }

  @Test
  public void testBootClasspathMemberThroughAvailableInterfaceSucceeds() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public interface Bar extends java.util.Map<String, String> {"
                + " }"));
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/bar:bar");
    compileWithValidation(
        "package com.facebook.foo; import com.facebook.bar.Bar;"
            + " abstract class Foo implements Bar { Entry<String, String> entry; }");

    assertNoErrors();
  }

  @Test
  public void testImportedBootClasspathMemberThroughMissingInterfaceSucceeds() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public interface Bar extends java.util.Map<String, String> {"
                + " }"));
    compileWithValidation(
        "package com.facebook.foo; import com.facebook.bar.Bar; import java.util.Map.Entry;"
            + " abstract class Foo implements Bar { Entry<String, String> entry; }");

    assertNoErrors();
  }

  @Test
  public void testQualifiedBootClasspathMemberThroughMissingInterfaceSucceeds() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public interface Bar extends java.util.Map<String, String> {"
                + " }"));
    compileWithValidation(
        "package com.facebook.foo; import com.facebook.bar.Bar;"
            + " abstract class Foo implements Bar { java.util.Map.Entry<String, String> entry; }");

    assertNoErrors();
  }

  @Test
  public void testStaticallyImportedBootClasspathMemberThroughMissingInterfaceSucceeds()
      throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public interface Bar extends java.util.Map<String, String> {"
                + " }"));
    compileWithValidation(
        "package com.facebook.foo; import com.facebook.bar.Bar; import static java.util.Map.Entry;"
            + " abstract class Foo implements Bar { Entry<String, String> entry; }");

    assertNoErrors();
  }

  @Test
  public void testBootClasspathMemberThroughAvailableDiamondBranchSucceeds() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public interface Bar extends java.util.Map<String, String> {"
                + " }",
            "com/facebook/baz/Baz.java",
            "package com.facebook.baz; public interface Baz extends java.util.Map<String, String> {"
                + " }"));
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/baz:baz");
    compileWithValidation(
        "package com.facebook.foo; import com.facebook.bar.Bar; import com.facebook.baz.Baz;"
            + " abstract class Foo implements Bar, Baz { Entry<String, String> entry; }");

    assertNoErrors();
  }

  @Test
  public void testBootClasspathMemberThroughMissingSuperclassFails() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            "package com.facebook.bar; public abstract class Bar implements java.util.Map<String,"
                + " String> { }"));
    testCompiler.setAllowCompilationErrors(true);
    compileWithValidation(
        Joiner.on('\n')
            .join(
                "package com.facebook.foo;",
                "import com.facebook.bar.Bar;",
                "abstract class Foo extends Bar {",
                "  Entry<String, String> entry;",
                "}"));

    assertError(
        Joiner.on('\n')
            .join(
                "Foo.java:4: error: Source-only ABI generation requires that this member type"
                    + " reference be more explicit.",
                "  Entry<String, String> entry;",
                "  ^",
                "  To fix: ",
                "  Add an import for \"java.util.Map\"",
                "  Use \"Map.Entry\" here instead of \"Entry\"."));
  }

  @Test
  public void testImportedSuperclassMemberTypeFromClasspathSucceeds() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.bar;",
                    "import com.facebook.foo.Baz;",
                    "public class Bar implements Baz { }"),
            "com/facebook/foo/Baz.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.foo;", "public interface Baz { interface Inner { } }")));
    compileWithValidation(
        ImmutableMap.of(
            "Foo.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.foo;",
                    "import com.facebook.bar.Bar;",
                    "import com.facebook.foo.Baz.Inner;",
                    "class Foo extends Bar {",
                    "  Inner i;",
                    "}")));

    assertNoErrors();
  }

  @Test
  public void testPackageMemberTypeCompiledTogetherSucceeds() throws IOException {
    compileWithValidation(
        ImmutableMap.of(
            "Foo.java",
            Joiner.on('\n').join("package com.facebook.foo;", "class Foo extends Bar { }"),
            "Bar.java",
            Joiner.on('\n').join("package com.facebook.foo;", "class Bar { }")));

    assertNoErrors();
  }

  @Test
  public void testPackageMemberTypeFromClasspathSucceeds() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/foo/Bar.java",
            Joiner.on('\n').join("package com.facebook.foo;", "class Bar { }")));
    compileWithValidation(
        ImmutableMap.of(
            "Foo.java",
            Joiner.on('\n').join("package com.facebook.foo;", "class Foo extends Bar { }")));

    assertNoErrors();
  }

  @Test
  public void testQualifiedPackageMemberInnerTypeFromClasspathSucceeds() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/foo/Bar.java",
            Joiner.on('\n')
                .join("package com.facebook.foo;", "class Bar { static class Inner { } }")));
    compileWithValidation(
        ImmutableMap.of(
            "Foo.java",
            Joiner.on('\n').join("package com.facebook.foo;", "class Foo extends Bar.Inner { }")));

    assertNoErrors();
  }

  @Test
  public void testUnqualifiedPackageMemberInnerTypeFromClasspathFails() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/foo/Bar.java",
            Joiner.on('\n')
                .join("package com.facebook.foo;", "class Bar { static class Inner { } }")));
    testCompiler.setAllowCompilationErrors(true);
    compileWithValidation(
        ImmutableMap.of(
            "Foo.java",
            Joiner.on('\n')
                .join("package com.facebook.foo;", "class Foo extends Bar {", "  Inner i;", "}")));

    assertError(
        Joiner.on('\n')
            .join(
                "Foo.java:3: error: Source-only ABI generation requires that this member type"
                    + " reference be more explicit.",
                "  Inner i;",
                "  ^",
                "  To fix: ",
                "  Add an import for \"com.facebook.foo.Bar\"",
                "  Use \"Bar.Inner\" here instead of \"Inner\"."));
  }

  @Test
  public void testNonCanonicalUnqualifiedPackageMemberInnerTypeFromClasspathFails()
      throws IOException {
    withClasspath(CLASSPATH_WITH_COMPLEX_MEMBER_TYPES);
    testCompiler.setAllowCompilationErrors(true);
    compileWithValidation(
        ImmutableMap.of(
            "FooBar.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.baz;",
                    "import com.facebook.bar.Bar;",
                    "class FooBar extends Bar {",
                    "  StaticMember.StaticMemberer.StaticMemberest i;",
                    "}")));

    assertErrors(
        Joiner.on('\n')
            .join(
                "FooBar.java:4: error: Source-only ABI generation requires that this type be"
                    + " referred to by its canonical name.",
                "  StaticMember.StaticMemberer.StaticMemberest i;",
                "  ^",
                "  To fix: ",
                "  Add an import for \"com.facebook.baz.Baz.SuperStaticMember\"",
                "  Use \"SuperStaticMember\" here instead of \"StaticMember\"."),
        Joiner.on('\n')
            .join(
                "FooBar.java:4: error: Source-only ABI generation requires that this type be"
                    + " referred to by its canonical name.",
                "  StaticMember.StaticMemberer.StaticMemberest i;",
                "              ^",
                "  To fix: ",
                "  Use \"SuperStaticMemberer\" here instead of \"StaticMemberer\"."));
  }

  @Test
  public void testNonCanonicalInnerTypeErrorMessageWhenCanonicalOuterIsImported()
      throws IOException {
    withClasspath(CLASSPATH_WITH_COMPLEX_MEMBER_TYPES);
    testCompiler.setAllowCompilationErrors(true);
    compileWithValidation(
        ImmutableMap.of(
            "Foo.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.foo;",
                    "import com.facebook.bar.Bar;",
                    "import com.facebook.baz.Baz;",
                    "public class Foo {",
                    "  Baz b;",
                    "  Bar.SuperStaticMember m;",
                    "}")));

    assertError(
        "Foo.java:6: error: Source-only ABI generation requires that this type be referred to by"
            + " its canonical name.\n"
            + "  Bar.SuperStaticMember m;\n"
            + "  ^\n"
            + "  To fix: \n"
            + "  Add an import for \"com.facebook.baz.Baz\"\n"
            + "  Use \"Baz\" here instead of \"Bar\".");
  }

  @Test
  public void testQualifiedNestedGenericSucceeds() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.bar;",
                    "public class Bar<T> {",
                    "  public class Inner<U> { }",
                    "}")));

    compileWithValidation(
        ImmutableMap.of(
            "Foo.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.foo;",
                    "public class Foo {",
                    "  com.facebook.bar.Bar<Integer>.Inner<String> i;",
                    "}")));

    assertNoErrors();
  }

  @Test
  public void testQualifiedAnnotatedTypeSucceeds() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/facebook/bar/Bar.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.bar;",
                    "public class Bar {",
                    "  public class Inner { }",
                    "}")));

    taskListenerFactory.setRuleIsRequiredForSourceAbi(true);
    compileWithValidation(
        ImmutableMap.of(
            "Foo.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.foo;",
                    "import java.lang.annotation.*;",
                    "public class Foo {",
                    "  com.facebook.bar.@Anno Bar.Inner i;",
                    "}",
                    "@Target(ElementType.TYPE_USE)",
                    "@interface Anno { }")));

    assertNoErrors();
  }

  @Test
  public void testConstantCompiledTogetherSucceeds() throws IOException {
    compileWithValidation(
        ImmutableMap.of(
            "Foo.java",
            Joiner.on('\n')
                .join(
                    "class Foo {",
                    "  public static final int CONSTANT = Constants.CONSTANT + 1 +"
                        + " Constants.CONSTANT2;",
                    "}"),
            "Constants.java",
            Joiner.on('\n')
                .join(
                    "class Constants {",
                    "  public static final int CONSTANT = 3 + 5;",
                    "  public static final int CONSTANT2 = 3;",
                    "}")));

    assertNoErrors();
  }

  @Test
  public void testConstantFromUnavailableTargetFails() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "Constants.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.constants;",
                    "public class Constants {",
                    "  public static final int CONSTANT = 3 + 5;",
                    "  public static final int CONST2 = 3;",
                    "}")));
    testCompiler.setAllowCompilationErrors(true);
    compileWithValidation(
        ImmutableMap.of(
            "Foo.java",
            Joiner.on('\n')
                .join(
                    "import com.facebook.constants.Constants;",
                    "class Foo {",
                    "  public static final int CONSTANT = Constants.CONSTANT + 1 +"
                        + " Constants.CONST2;",
                    "}")));

    assertErrors(
        Joiner.on('\n')
            .join(
                "Foo.java:3: error: This constant will not be available during source-only ABI"
                    + " generation.",
                "  public static final int CONSTANT = Constants.CONSTANT + 1 + Constants.CONST2;",
                "                                              ^",
                "  For a quick fix, add required_for_source_only_abi = True to"
                    + " //com/facebook/constants:constants.",
                "  A better fix is to move Constants to a new rule that contains only",
                "  constants, and mark that rule required_for_source_only_abi."),
        Joiner.on('\n')
            .join(
                "Foo.java:3: error: This constant will not be available during source-only ABI"
                    + " generation.",
                "  public static final int CONSTANT = Constants.CONSTANT + 1 + Constants.CONST2;",
                "                                                                       ^",
                "  For a quick fix, add required_for_source_only_abi = True to"
                    + " //com/facebook/constants:constants.",
                "  A better fix is to move Constants to a new rule that contains only",
                "  constants, and mark that rule required_for_source_only_abi."));
  }

  @Test
  public void testConstantFromAvailableTargetFails() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "Constants.java",
            Joiner.on('\n')
                .join(
                    "package com.facebook.constants;",
                    "public class Constants {",
                    "  public static final int CONSTANT = 3 + 5;",
                    "  public static final int CONST2 = 3;",
                    "}")));
    taskListenerFactory.addTargetAvailableForSourceOnlyAbi("//com/facebook/constants:constants");
    compileWithValidation(
        ImmutableMap.of(
            "Foo.java",
            Joiner.on('\n')
                .join(
                    "import com.facebook.constants.Constants;",
                    "class Foo {",
                    "  public static final int CONSTANT = Constants.CONSTANT + 1 +"
                        + " Constants.CONST2;",
                    "}")));

    assertNoErrors();
  }

  protected void compileWithValidation(String source) throws IOException {
    compileWithValidation(ImmutableMap.of("Foo.java", source));
  }

  protected Iterable<? extends CompilationUnitTree> compileWithValidation(
      Map<String, String> sources) throws IOException {
    return compile(sources, taskListenerFactory);
  }
}
