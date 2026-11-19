/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.dex.desugarclasspath;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import com.google.common.io.ByteStreams;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Enumeration;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeMap;
import java.util.TreeSet;
import java.util.concurrent.TimeUnit;
import java.util.function.Consumer;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;
import java.util.zip.ZipOutputStream;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassVisitor;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.Handle;
import org.objectweb.asm.MethodVisitor;
import org.objectweb.asm.Opcodes;
import org.objectweb.asm.Type;

public class DesugarClasspathViewTest {

  @Rule public TemporaryFolder tmp = new TemporaryFolder();

  private static final int PUBLIC = Opcodes.ACC_PUBLIC;
  private static final int PUBLIC_INTERFACE =
      Opcodes.ACC_PUBLIC | Opcodes.ACC_INTERFACE | Opcodes.ACC_ABSTRACT;

  // ---- The view itself ----

  @Test
  public void viewHoldsReferencedTypesAndTheirSupertypes() throws Exception {
    Path classpath =
        jar(
            "cp.jar",
            ImmutableMap.of(
                "p/A.class", cls("p/A", "java/lang/Object", PUBLIC, c -> {}),
                "p/I.class", cls("p/I", "java/lang/Object", PUBLIC_INTERFACE, c -> {}),
                "p/B.class", cls("p/B", "p/A", PUBLIC, c -> method(c, PUBLIC, "m", "()V"), "p/I"),
                "p/Unreferenced.class",
                    cls("p/Unreferenced", "java/lang/Object", PUBLIC, c -> {})));
    Path program = program(c -> callVirtual(c, "p/B", "m", "()V"));

    View view = writeView(program, classpath);

    assertEquals(ImmutableSet.of("p/A.class", "p/B.class", "p/I.class"), view.entries.keySet());
    assertTrue(view.duplicates.isEmpty());
  }

  @Test
  public void viewKeepsOnlyMembersWhoseNameTheProgramContains() throws Exception {
    Path classpath =
        jar(
            "cp.jar",
            ImmutableMap.of(
                "p/B.class",
                cls(
                    "p/B",
                    "java/lang/Object",
                    PUBLIC,
                    c -> {
                      method(c, PUBLIC, "<init>", "()V");
                      method(c, PUBLIC, "m", "()V");
                      method(c, PUBLIC, "m", "(I)V");
                      method(c, PUBLIC, "notReferenced", "()V");
                      method(c, PUBLIC, "close", "()V");
                      c.visitField(PUBLIC, "f", "I", null, null).visitEnd();
                      c.visitField(PUBLIC, "g", "I", null, null).visitEnd();
                    })));
    Path program =
        program(
            c -> {
              callVirtual(c, "p/B", "m", "()V");
              getField(c, "p/B", "f", "I");
            });

    View view = writeView(program, classpath);

    assertEquals(
        ImmutableSet.of("<init>()V", "m()V", "m(I)V", "close()V", "f:I"),
        members(view.entries.get("p/B.class")));
  }

  @Test
  public void viewIgnoresMembersAndClassesTheProgramDoesNotReferTo() throws Exception {
    Path program = program(c -> callVirtual(c, "p/B", "m", "()V"));
    byte[] before =
        writeView(program, jar("before.jar", ImmutableMap.of("p/B.class", classB(false, false))))
            .bytes;
    byte[] withNewMemberAndClass =
        writeView(
                program,
                jar(
                    "after.jar",
                    ImmutableMap.of(
                        "p/B.class",
                        classB(true, false),
                        "p/New.class",
                        cls("p/New", "java/lang/Object", PUBLIC, c -> {}))))
            .bytes;
    byte[] withStaticM =
        writeView(program, jar("static.jar", ImmutableMap.of("p/B.class", classB(false, true))))
            .bytes;

    assertArrayEquals(before, withNewMemberAndClass);
    assertFalse(Arrays.equals(before, withStaticM));
  }

  @Test
  public void viewKeepsTheStringAtConstantPoolIndexTwo() throws Exception {
    String marker = "~~R8{\"backend\":\"dex\",\"compilation-mode\":\"release\"}";
    ClassWriter withMarker = new ClassWriter(0);
    withMarker.newConst(marker);
    withMarker.visit(Opcodes.V1_8, PUBLIC, "p/B", null, "java/lang/Object", null);
    method(withMarker, PUBLIC, "unused", "()V");
    withMarker.visitEnd();
    Path classpath = jar("cp.jar", ImmutableMap.of("p/B.class", withMarker.toByteArray()));

    byte[] viewClass =
        writeView(program(c -> callVirtual(c, "p/B", "m", "()V")), classpath)
            .entries
            .get("p/B.class");

    assertEquals(marker, DesugarClasspathView.readMarker(new ClassReader(viewClass)));
    assertNull(DesugarClasspathView.readMarker(new ClassReader(classB(false, false))));
  }

  @Test
  public void secondDefinitionGoesToTheDuplicatesJar() throws Exception {
    byte[] first = classB(false, false);
    byte[] second = classB(true, false);
    Path program = program(c -> callVirtual(c, "p/B", "m", "()V"));

    View view =
        writeView(
            program,
            jar("first.jar", ImmutableMap.of("p/B.class", first)),
            jar("second.jar", ImmutableMap.of("p/B.class", second)));

    assertEquals(ImmutableSet.of("p/B.class"), view.entries.keySet());
    assertEquals(ImmutableSet.of("p/B.class"), view.duplicates.keySet());
    assertEquals(ImmutableSet.of("<init>()V", "m()V"), members(view.entries.get("p/B.class")));
  }

  @Test
  public void nestMembersAndRecordsKeepAllOfTheirBytes() throws Exception {
    ClassWriter host = new ClassWriter(0);
    host.visit(Opcodes.V11, PUBLIC, "p/Host", null, "java/lang/Object", null);
    host.visitNestMember("p/Host$Member");
    method(host, PUBLIC, "notReferenced", "()V");
    host.visitEnd();
    ClassWriter member = new ClassWriter(0);
    member.visit(Opcodes.V11, PUBLIC, "p/Host$Member", null, "java/lang/Object", null);
    member.visitNestHost("p/Host");
    member.visitEnd();
    byte[] record =
        cls(
            "p/R",
            "java/lang/Record",
            PUBLIC | Opcodes.ACC_FINAL | Opcodes.ACC_RECORD,
            c -> method(c, PUBLIC, "notReferenced", "()V"));
    Map<String, byte[]> classes =
        ImmutableMap.of(
            "p/Host.class",
            host.toByteArray(),
            "p/Host$Member.class",
            member.toByteArray(),
            "p/R.class",
            record);
    Path program =
        program(
            c -> {
              callVirtual(c, "p/Host", "m", "()V");
              callVirtual(c, "p/R", "m", "()V");
            });

    View view = writeView(program, jar("cp.jar", classes));

    assertEquals(classes.keySet(), view.entries.keySet());
    for (Map.Entry<String, byte[]> entry : classes.entrySet()) {
      assertArrayEquals(entry.getKey(), entry.getValue(), view.entries.get(entry.getKey()));
    }
  }

  @Test
  public void viewKeepsClassesD8LooksUpByNameWhole() throws Exception {
    byte[] synthetic =
        cls(
            "q/User$$ExternalSyntheticLambda0",
            "java/lang/Object",
            PUBLIC,
            c -> method(c, PUBLIC, "notReferenced", "()V"));
    byte[] global =
        cls(
            "com/android/tools/r8/RecordTag",
            "java/lang/Object",
            PUBLIC,
            c -> method(c, PUBLIC, "notReferenced", "()V"));
    Path classpath =
        jar(
            "cp.jar",
            ImmutableMap.of(
                "q/User$$ExternalSyntheticLambda0.class",
                synthetic,
                "com/android/tools/r8/RecordTag.class",
                global,
                "q/UserOther$$ExternalSyntheticLambda0.class",
                cls("q/UserOther$$ExternalSyntheticLambda0", "java/lang/Object", PUBLIC, c -> {}),
                "q/Unreferenced.class",
                cls("q/Unreferenced", "java/lang/Object", PUBLIC, c -> {})));

    View view = writeView(program(c -> {}), classpath);

    assertEquals(
        ImmutableSet.of(
            "com/android/tools/r8/RecordTag.class", "q/User$$ExternalSyntheticLambda0.class"),
        view.entries.keySet());
    assertArrayEquals(synthetic, view.entries.get("q/User$$ExternalSyntheticLambda0.class"));
    assertArrayEquals(global, view.entries.get("com/android/tools/r8/RecordTag.class"));
  }

  @Test
  public void viewKeepsAllFieldsOfResourceClasses() throws Exception {
    Path classpath = jar("cp.jar", ImmutableMap.of("p/R$string.class", resourceClass()));

    View view =
        writeView(program("p/R$string", c -> getStatic(c, "p/R$string", "foo", "I")), classpath);

    assertEquals(
        ImmutableSet.of("<init>()V", "bar:I", "baz:I", "foo:I"),
        members(view.entries.get("p/R$string.class")));
  }

  @Test
  public void viewBytesAreTheSameOnEveryRun() throws Exception {
    Map<String, byte[]> classes = new TreeMap<>();
    for (int i = 0; i < 300; i++) {
      String name = "p/C" + i;
      classes.put(
          name + ".class",
          cls(
              name,
              i == 0 ? "java/lang/Object" : "p/C" + (i - 1),
              PUBLIC,
              c -> method(c, PUBLIC, "m", "()V")));
    }
    Path classpath = jar("cp.jar", classes);
    Path program =
        program(
            c -> {
              for (int i = 0; i < 300; i += 7) {
                callVirtual(c, "p/C" + i, "m", "()V");
              }
            });

    byte[] first = writeView(program, classpath).bytes;
    for (int run = 0; run < 3; run++) {
      assertArrayEquals(first, writeView(program, classpath).bytes);
    }
  }

  @Test
  public void classFileEntriesMatchD8() {
    assertTrue(DesugarClasspathView.isClassFile("p/B.class"));
    assertFalse(DesugarClasspathView.isClassFile("module-info.class"));
    assertFalse(DesugarClasspathView.isClassFile("META-INF/versions/9/p/B.class"));
    assertFalse(DesugarClasspathView.isClassFile("p/B.txt"));
  }

  @Test
  public void typesFromDescriptorsAndSignatures() {
    Set<String> types = new TreeSet<>();
    DesugarClasspathView.addTypesFromSignature("(ILp/A;[Lp/B;J)Lp/C;", types);
    DesugarClasspathView.addTypesFromSignature("Ljava/util/List<+Lp/D;>;", types);
    DesugarClasspathView.addTypesFromSignature("<T:Lp/E;>(TT;)V", types);
    DesugarClasspathView.addTypesFromSignature("<LT:Lp/F;U::Lp/G<TLT;>;>(TU;TLFoo;)V", types);
    DesugarClasspathView.addTypesFromSignature("p/NotADescriptor", types);
    assertEquals(
        ImmutableSet.of("java/util/List", "p/A", "p/B", "p/C", "p/D", "p/E", "p/F", "p/G"), types);
  }

  // ---- D8 with the view against D8 with the whole classpath ----

  @Test
  public void autoCloseableDispatchIsTheSameAsWithTheWholeClasspath() throws Exception {
    Path classpath =
        jar(
            "cp.jar",
            ImmutableMap.of(
                "p/MyExecutor.class",
                cls(
                    "p/MyExecutor",
                    "java/util/concurrent/AbstractExecutorService",
                    PUBLIC | Opcodes.ACC_ABSTRACT,
                    c -> method(c, PUBLIC, "notReferenced", "()V")),
                "p/Unrelated.class",
                cls("p/Unrelated", "java/lang/Object", PUBLIC, c -> {})));
    Path program = program(c -> callVirtual(c, "p/MyExecutor", "close", "()V"));

    D8Result whole = d8(program, classpath);
    D8Result view = d8WithView(program, classpath);
    D8Result empty = d8(program);

    assertEquals(whole.stderr, 0, whole.exitCode);
    assertFalse("D8 must write the dex units", whole.dexUnits.isEmpty());
    assertTrue(
        whole.syntheticContexts,
        whole.syntheticContexts.contains("ExternalSyntheticAutoCloseable"));
    assertArrayEquals(whole.dex, view.dex);
    assertEquals(whole.dexUnits, view.dexUnits);
    assertEquals(whole.syntheticContexts, view.syntheticContexts);
    assertFalse(
        "The classpath must matter for this case",
        empty.syntheticContexts.contains("ExternalSyntheticAutoCloseable"));
  }

  @Test
  public void classpathOverrideOfCloseIsTheSameAsWithTheWholeClasspath() throws Exception {
    Path withOverride = jar("override.jar", ImmutableMap.of("p/MyExecutor.class", executor(true)));
    Path withoutOverride =
        jar("no_override.jar", ImmutableMap.of("p/MyExecutor.class", executor(false)));
    Path program = program(c -> callVirtual(c, "p/MyExecutor", "close", "()V"));

    D8Result whole = d8(program, withOverride);
    D8Result view = d8WithView(program, withOverride);
    D8Result inherited = d8(program, withoutOverride);

    assertEquals(whole.stderr, 0, whole.exitCode);
    assertArrayEquals(whole.dex, view.dex);
    assertEquals(whole.dexUnits, view.dexUnits);
    assertEquals(whole.syntheticContexts, view.syntheticContexts);
    assertFalse(
        "A classpath method must matter for this case", Arrays.equals(whole.dex, inherited.dex));
  }

  @Test
  public void syntheticClassNamesAreTheSameAsWithTheWholeClasspath() throws Exception {
    Path withClash =
        jar(
            "clash.jar",
            ImmutableMap.of(
                "q/User$$ExternalSyntheticLambda0.class",
                cls("q/User$$ExternalSyntheticLambda0", "java/lang/Object", PUBLIC, c -> {})));
    Path program = programWithLambda();

    D8Result whole = d8(program, withClash);
    D8Result view = d8WithView(program, withClash);
    D8Result noClash = d8(program);

    assertEquals(whole.stderr, 0, whole.exitCode);
    assertTrue(whole.classNames, whole.classNames.contains("q/User$$ExternalSyntheticLambda1"));
    assertArrayEquals(whole.dex, view.dex);
    assertEquals(whole.dexUnits, view.dexUnits);
    assertEquals(whole.classNames, view.classNames);
    assertEquals(whole.syntheticContexts, view.syntheticContexts);
    assertFalse("The classpath must matter for this case", Arrays.equals(whole.dex, noClash.dex));
  }

  @Test
  public void referencedResourcesAreTheSameAsWithTheWholeClasspath() throws Exception {
    Path classpath = jar("cp.jar", ImmutableMap.of("p/R$string.class", resourceClass()));
    Path program = program("p/R$string", c -> getStatic(c, "p/R$string", "foo", "I"));

    D8Result whole = d8(program, classpath);
    D8Result view = d8WithView(program, classpath);

    assertEquals(whole.stderr, 0, whole.exitCode);
    assertTrue(whole.referencedResources, whole.referencedResources.contains("p.bar"));
    assertArrayEquals(whole.dex, view.dex);
    assertEquals(whole.dexUnits, view.dexUnits);
    assertEquals(whole.referencedResources, view.referencedResources);
  }

  @Test
  public void duplicateClasspathTypeFailsAsWithTheWholeClasspath() throws Exception {
    Path first = jar("first.jar", ImmutableMap.of("p/MyExecutor.class", executor(false)));
    Path second = jar("second.jar", ImmutableMap.of("p/MyExecutor.class", executor(false)));
    Path program = program(c -> callVirtual(c, "p/MyExecutor", "close", "()V"));

    D8Result whole = d8(program, first, second);
    D8Result view = d8WithView(program, first, second);

    assertNotEquals(0, whole.exitCode);
    assertTrue(whole.stderr, whole.stderr.contains("Classpath type already present"));
    assertNotEquals(0, view.exitCode);
    assertTrue(view.stderr, view.stderr.contains("Classpath type already present"));
  }

  // ---- Helpers ----

  private static final class View {
    final Map<String, byte[]> entries;
    final Map<String, byte[]> duplicates;
    final byte[] bytes;

    View(Map<String, byte[]> entries, Map<String, byte[]> duplicates, byte[] bytes) {
      this.entries = entries;
      this.duplicates = duplicates;
      this.bytes = bytes;
    }
  }

  private static final class D8Result {
    final int exitCode;
    final String stderr;
    final byte[] dex;
    final String syntheticContexts;
    final String classNames;
    final String referencedResources;
    final String dexUnits;

    D8Result(
        int exitCode,
        String stderr,
        byte[] dex,
        String syntheticContexts,
        String classNames,
        String referencedResources,
        String dexUnits) {
      this.exitCode = exitCode;
      this.stderr = stderr;
      this.dex = dex;
      this.syntheticContexts = syntheticContexts;
      this.classNames = classNames;
      this.referencedResources = referencedResources;
      this.dexUnits = dexUnits;
    }
  }

  private int counter = 0;

  private Path newPath(String name) {
    return tmp.getRoot().toPath().resolve((counter++) + "_" + name);
  }

  private View writeView(Path program, Path... classpath) throws IOException {
    Path view = newPath("view.jar");
    Path duplicates = newPath("duplicates.jar");
    DesugarClasspathView.write(program, Arrays.asList(classpath), view, duplicates);
    return new View(readJar(view), readJar(duplicates), Files.readAllBytes(view));
  }

  private D8Result d8WithView(Path program, Path... classpath) throws Exception {
    Path view = newPath("view.jar");
    Path duplicates = newPath("duplicates.jar");
    DesugarClasspathView.write(program, Arrays.asList(classpath), view, duplicates);
    return d8(program, view, duplicates);
  }

  /** Runs the pre-dex command line of {@code java/dex.bzl} at min SDK 33 in a child JVM. */
  private D8Result d8(Path program, Path... classpath) throws Exception {
    Path classpathFile = newPath("classpath.txt");
    List<String> lines = new ArrayList<>();
    for (Path path : classpath) {
      lines.add(path.toString());
    }
    Files.write(classpathFile, lines);
    Path output = newPath("out.dex.jar");
    Path syntheticContexts = newPath("synthetic_contexts.txt");
    Path classNames = newPath("class_names.txt");
    Path referencedResources = newPath("referenced_resources.txt");
    Path dexUnits = newPath("dex_units.txt");
    Path stderr = newPath("stderr.txt");
    Path scratch = Files.createDirectories(newPath("scratch"));
    List<String> command =
        ImmutableList.of(
            System.getProperty("java.home") + File.separator + "bin" + File.separator + "java",
            "-cp",
            System.getProperty("java.class.path"),
            "com.facebook.buck.android.dex.D8ExecutableMain",
            "--output-dex-file",
            output.toString(),
            "--file-to-dex",
            program.toString(),
            "--android-jar",
            androidJar().toString(),
            "--intermediate",
            "--file-per-class-file",
            "--no-optimize",
            "--force-jumbo",
            "--classpath-files",
            classpathFile.toString(),
            "--synthetic-contexts-path",
            syntheticContexts.toString(),
            "--class-names-path",
            classNames.toString(),
            "--referenced-resources-path",
            referencedResources.toString(),
            "--dex-units-path",
            dexUnits.toString(),
            "--min-sdk-version",
            "33");
    ProcessBuilder builder = new ProcessBuilder(command);
    builder.environment().put("BUCK_SCRATCH_PATH", scratch.toString());
    builder.redirectErrorStream(true);
    builder.redirectOutput(stderr.toFile());
    Process process = builder.start();
    assertTrue("D8 timed out", process.waitFor(5, TimeUnit.MINUTES));
    int exitCode = process.exitValue();
    String log = new String(Files.readAllBytes(stderr), StandardCharsets.UTF_8);
    if (exitCode != 0) {
      return new D8Result(exitCode, log, new byte[0], "", "", "", "");
    }
    return new D8Result(
        exitCode,
        log,
        Files.readAllBytes(output),
        read(syntheticContexts),
        read(classNames),
        read(referencedResources),
        read(dexUnits));
  }

  private static String read(Path path) throws IOException {
    return new String(Files.readAllBytes(path), StandardCharsets.UTF_8);
  }

  private Path androidJar = null;

  /** A library with the classes these tests need from {@code android.jar}. */
  private Path androidJar() throws IOException {
    if (androidJar != null) {
      return androidJar;
    }
    Map<String, byte[]> classes = new TreeMap<>();
    classes.put(
        "java/lang/Object.class",
        cls(
            "java/lang/Object",
            null,
            PUBLIC,
            c -> {
              method(c, PUBLIC, "<init>", "()V");
              method(c, PUBLIC, "equals", "(Ljava/lang/Object;)Z");
              method(c, PUBLIC, "hashCode", "()I");
              method(c, PUBLIC, "toString", "()Ljava/lang/String;");
            }));
    classes.put(
        "java/lang/String.class", cls("java/lang/String", "java/lang/Object", PUBLIC, c -> {}));
    classes.put(
        "java/lang/Record.class",
        cls("java/lang/Record", "java/lang/Object", PUBLIC | Opcodes.ACC_ABSTRACT, c -> {}));
    classes.put(
        "java/lang/Runnable.class",
        cls(
            "java/lang/Runnable",
            "java/lang/Object",
            PUBLIC_INTERFACE,
            c -> method(c, PUBLIC | Opcodes.ACC_ABSTRACT, "run", "()V")));
    classes.put(
        "java/lang/AutoCloseable.class",
        cls(
            "java/lang/AutoCloseable",
            "java/lang/Object",
            PUBLIC_INTERFACE,
            c -> method(c, PUBLIC | Opcodes.ACC_ABSTRACT, "close", "()V")));
    classes.put(
        "java/util/concurrent/Executor.class",
        cls(
            "java/util/concurrent/Executor",
            "java/lang/Object",
            PUBLIC_INTERFACE,
            c -> method(c, PUBLIC | Opcodes.ACC_ABSTRACT, "execute", "(Ljava/lang/Runnable;)V")));
    classes.put(
        "java/util/concurrent/ExecutorService.class",
        cls(
            "java/util/concurrent/ExecutorService",
            "java/lang/Object",
            PUBLIC_INTERFACE,
            c -> {
              method(c, PUBLIC | Opcodes.ACC_ABSTRACT, "shutdown", "()V");
              method(c, PUBLIC, "close", "()V");
            },
            "java/util/concurrent/Executor",
            "java/lang/AutoCloseable"));
    classes.put(
        "java/util/concurrent/AbstractExecutorService.class",
        cls(
            "java/util/concurrent/AbstractExecutorService",
            "java/lang/Object",
            PUBLIC | Opcodes.ACC_ABSTRACT,
            c -> method(c, PUBLIC, "<init>", "()V"),
            "java/util/concurrent/ExecutorService"));
    androidJar = jar("android.jar", classes);
    return androidJar;
  }

  /** An executor on the classpath, optionally with its own close(). */
  private static byte[] executor(boolean overridesClose) {
    return cls(
        "p/MyExecutor",
        "java/util/concurrent/AbstractExecutorService",
        PUBLIC | Opcodes.ACC_ABSTRACT,
        c -> {
          if (overridesClose) {
            method(c, PUBLIC, "close", "()V");
          }
        });
  }

  /** A resource class with three static int fields. */
  private static byte[] resourceClass() {
    return cls(
        "p/R$string",
        "java/lang/Object",
        PUBLIC,
        c -> {
          method(c, PUBLIC, "<init>", "()V");
          for (String field : ImmutableList.of("foo", "bar", "baz")) {
            c.visitField(PUBLIC | Opcodes.ACC_STATIC, field, "I", null, null).visitEnd();
          }
        });
  }

  /** p/B with m(), optionally an extra method and a static m(). Carries kotlin.Metadata. */
  private static byte[] classB(boolean withExtraMember, boolean staticM) {
    return cls(
        "p/B",
        "java/lang/Object",
        PUBLIC,
        c -> {
          c.visitAnnotation("Lkotlin/Metadata;", true)
              .visit("d2", withExtraMember ? "m,added" : "m");
          method(c, PUBLIC, "<init>", "()V");
          method(c, PUBLIC | (staticM ? Opcodes.ACC_STATIC : 0), "m", "()V");
          if (withExtraMember) {
            method(c, PUBLIC, "added", "()V");
          }
        });
  }

  private Path program(Consumer<MethodVisitor> body) throws IOException {
    return program("java/lang/Object", body);
  }

  /** q/User, with a static run(Object) whose body is {@code body}. */
  private Path program(String superName, Consumer<MethodVisitor> body) throws IOException {
    ClassWriter writer = new ClassWriter(ClassWriter.COMPUTE_MAXS);
    writer.visit(Opcodes.V1_8, PUBLIC, "q/User", null, superName, null);
    MethodVisitor init = writer.visitMethod(PUBLIC, "<init>", "()V", null, null);
    init.visitCode();
    init.visitVarInsn(Opcodes.ALOAD, 0);
    init.visitMethodInsn(Opcodes.INVOKESPECIAL, superName, "<init>", "()V", false);
    init.visitInsn(Opcodes.RETURN);
    init.visitMaxs(0, 0);
    init.visitEnd();
    MethodVisitor run =
        writer.visitMethod(PUBLIC | Opcodes.ACC_STATIC, "run", "(Ljava/lang/Object;)V", null, null);
    run.visitCode();
    body.accept(run);
    run.visitInsn(Opcodes.RETURN);
    run.visitMaxs(0, 0);
    run.visitEnd();
    writer.visitEnd();
    return jar("program.jar", ImmutableMap.of("q/User.class", writer.toByteArray()));
  }

  /** q/User, whose run() returns a lambda for its static lambda$run$0(). */
  private Path programWithLambda() throws IOException {
    ClassWriter writer = new ClassWriter(ClassWriter.COMPUTE_MAXS);
    writer.visit(Opcodes.V1_8, PUBLIC, "q/User", null, "java/lang/Object", null);
    MethodVisitor lambda =
        writer.visitMethod(
            Opcodes.ACC_PRIVATE | Opcodes.ACC_STATIC | Opcodes.ACC_SYNTHETIC,
            "lambda$run$0",
            "()V",
            null,
            null);
    lambda.visitCode();
    lambda.visitInsn(Opcodes.RETURN);
    lambda.visitMaxs(0, 0);
    lambda.visitEnd();
    MethodVisitor run =
        writer.visitMethod(
            PUBLIC | Opcodes.ACC_STATIC, "run", "()Ljava/lang/Runnable;", null, null);
    run.visitCode();
    run.visitInvokeDynamicInsn(
        "run",
        "()Ljava/lang/Runnable;",
        new Handle(
            Opcodes.H_INVOKESTATIC,
            "java/lang/invoke/LambdaMetafactory",
            "metafactory",
            "(Ljava/lang/invoke/MethodHandles$Lookup;Ljava/lang/String;Ljava/lang/invoke/MethodType;"
                + "Ljava/lang/invoke/MethodType;Ljava/lang/invoke/MethodHandle;"
                + "Ljava/lang/invoke/MethodType;)Ljava/lang/invoke/CallSite;",
            false),
        Type.getType("()V"),
        new Handle(Opcodes.H_INVOKESTATIC, "q/User", "lambda$run$0", "()V", false),
        Type.getType("()V"));
    run.visitInsn(Opcodes.ARETURN);
    run.visitMaxs(0, 0);
    run.visitEnd();
    writer.visitEnd();
    return jar("program.jar", ImmutableMap.of("q/User.class", writer.toByteArray()));
  }

  private static void getStatic(MethodVisitor m, String owner, String name, String descriptor) {
    m.visitFieldInsn(Opcodes.GETSTATIC, owner, name, descriptor);
    m.visitInsn(Opcodes.POP);
  }

  private static void callVirtual(MethodVisitor m, String owner, String name, String descriptor) {
    m.visitVarInsn(Opcodes.ALOAD, 0);
    m.visitTypeInsn(Opcodes.CHECKCAST, owner);
    m.visitMethodInsn(Opcodes.INVOKEVIRTUAL, owner, name, descriptor, false);
  }

  private static void getField(MethodVisitor m, String owner, String name, String descriptor) {
    m.visitVarInsn(Opcodes.ALOAD, 0);
    m.visitTypeInsn(Opcodes.CHECKCAST, owner);
    m.visitFieldInsn(Opcodes.GETFIELD, owner, name, descriptor);
    m.visitInsn(Opcodes.POP);
  }

  private static byte[] cls(
      String name,
      String superName,
      int access,
      Consumer<ClassWriter> members,
      String... interfaces) {
    ClassWriter writer = new ClassWriter(0);
    writer.visit(Opcodes.V1_8, access, name, null, superName, interfaces);
    members.accept(writer);
    writer.visitEnd();
    return writer.toByteArray();
  }

  private static void method(ClassWriter c, int access, String name, String descriptor) {
    c.visitMethod(access, name, descriptor, null, null).visitEnd();
  }

  private static Set<String> members(byte[] classFile) {
    Set<String> members = new HashSet<>();
    new ClassReader(classFile)
        .accept(
            new ClassVisitor(Opcodes.ASM9) {
              @Override
              public org.objectweb.asm.FieldVisitor visitField(
                  int access, String name, String descriptor, String signature, Object value) {
                members.add(name + ":" + descriptor);
                return null;
              }

              @Override
              public MethodVisitor visitMethod(
                  int access,
                  String name,
                  String descriptor,
                  String signature,
                  String[] exceptions) {
                members.add(name + descriptor);
                return null;
              }
            },
            0);
    return members;
  }

  private Path jar(String name, Map<String, byte[]> entries) throws IOException {
    Path path = newPath(name);
    try (OutputStream file = Files.newOutputStream(path);
        ZipOutputStream zip = new ZipOutputStream(file)) {
      for (Map.Entry<String, byte[]> entry : new TreeMap<>(entries).entrySet()) {
        zip.putNextEntry(new ZipEntry(entry.getKey()));
        zip.write(entry.getValue());
        zip.closeEntry();
      }
    }
    return path;
  }

  private static Map<String, byte[]> readJar(Path path) throws IOException {
    Map<String, byte[]> entries = new TreeMap<>();
    try (ZipFile zip = new ZipFile(path.toFile())) {
      Enumeration<? extends ZipEntry> all = zip.entries();
      while (all.hasMoreElements()) {
        ZipEntry entry = all.nextElement();
        try (InputStream input = zip.getInputStream(entry)) {
          entries.put(entry.getName(), ByteStreams.toByteArray(input));
        }
      }
    }
    return entries;
  }
}
