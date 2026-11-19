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

import com.facebook.buck.util.zip.ZipScrubber;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableSet;
import com.google.common.io.ByteStreams;
import java.io.BufferedOutputStream;
import java.io.ByteArrayInputStream;
import java.io.DataInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.io.UncheckedIOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.TreeMap;
import java.util.TreeSet;
import java.util.concurrent.ConcurrentHashMap;
import java.util.stream.Collectors;
import java.util.zip.Deflater;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;
import java.util.zip.ZipOutputStream;
import org.jetbrains.annotations.Nullable;
import org.objectweb.asm.AnnotationVisitor;
import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassVisitor;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.FieldVisitor;
import org.objectweb.asm.MethodVisitor;
import org.objectweb.asm.Opcodes;

/**
 * Writes the part of a desugar classpath that D8 can read when it desugars one program jar at a min
 * SDK of 24 or higher.
 *
 * <p>D8 loads a classpath class only when it looks up that type, and it parses the class without
 * code or debug information (D8's {@code JarClassFileReader} reads classpath classes with {@code
 * SKIP_CODE} and {@code SKIP_DEBUG}, unless the class is in a nest). It also records the string
 * constant at constant pool index 2 of every class it reads, as a possible marker. At a min SDK of
 * 24 or higher, D8 does not desugar interface methods, and the desugarings it runs look up the
 * types that the program references and their supertypes, and look up members of those types by
 * name.
 *
 * <p>So the view holds, for every type that the program jar references (in a class constant or in a
 * descriptor or signature) and for their supertypes and nest members, transitively: the class from
 * the first classpath jar that defines it, without code, debug information and {@code
 * kotlin.Metadata}, and with only the fields and methods whose name occurs in the program jar's
 * constant pools or in {@link #IMPLICIT_MEMBER_NAMES}. The string constant at index 2 stays at
 * index 2. A class in a nest, and a record, keeps all of its bytes. A second definition of a type
 * goes to the duplicates jar, so that D8 fails with "Classpath type already present" for the same
 * types as with the full classpath.
 *
 * <p>D8 also looks up types that the program does not reference: when it names a synthetic class
 * {@code P$$ExternalSynthetic...} for a program class {@code P}, it skips names that the classpath
 * already defines, and it looks up its global synthetics under {@code com/android/tools/r8/}. The
 * view keeps every classpath class with such a name, whole. D8 also lists the fields of every class
 * it loads whose descriptor contains {@code /R$} as referenced resources, so the view keeps all
 * fields of such a class.
 *
 * <p>The view does not change when the classpath gains a class that the program does not reference,
 * or a member whose name the program does not contain.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public final class DesugarClasspathView {

  /**
   * Member names that D8 looks up on classpath classes although the program may not contain them:
   * {@code close} for AutoCloseable dispatch, constructors and class initializers, and the methods
   * of {@code java.lang.Object}.
   */
  static final ImmutableSet<String> IMPLICIT_MEMBER_NAMES =
      ImmutableSet.of(
          "<clinit>",
          "<init>",
          "clone",
          "close",
          "equals",
          "finalize",
          "getClass",
          "hashCode",
          "notify",
          "notifyAll",
          "toString",
          "wait");

  /** D8's {@code CfApplicationClassWriter.MARKER_STRING_CONSTANT_POOL_INDEX}. */
  static final int MARKER_CONSTANT_POOL_INDEX = 2;

  /** D8's {@code SyntheticNaming.SYNTHETIC_CLASS_SEPARATOR}. */
  static final String SYNTHETIC_CLASS_SEPARATOR = "$$";

  /** The package of D8's global synthetic types, such as {@code com/android/tools/r8/RecordTag}. */
  static final String GLOBAL_SYNTHETIC_PACKAGE = "com/android/tools/r8/";

  /** What D8's {@code DexItemFactory.computeReferencedResources} looks for in a descriptor. */
  static final String RESOURCE_CLASS_MARKER = "/R$";

  private static final String CLASS_SUFFIX = ".class";
  private static final String KOTLIN_METADATA_DESCRIPTOR = "Lkotlin/Metadata;";

  private DesugarClasspathView() {}

  /** The names and types that a program jar refers to. */
  static final class ProgramReferences {
    final Set<String> classes = Objects.requireNonNull(ConcurrentHashMap.newKeySet());
    final Set<String> memberNames = Objects.requireNonNull(ConcurrentHashMap.newKeySet());
    final Set<String> types = Objects.requireNonNull(ConcurrentHashMap.newKeySet());
  }

  /**
   * Writes the view of {@code classpath} (in D8's classpath order) for {@code programJar} to {@code
   * output}, and the second definitions of duplicated types to {@code duplicatesOutput}.
   */
  public static void write(
      Path programJar, List<Path> classpath, Path output, Path duplicatesOutput)
      throws IOException {
    ProgramReferences references = readProgramReferences(programJar);
    Set<String> memberNames = new HashSet<>(references.memberNames);
    memberNames.addAll(IMPLICIT_MEMBER_NAMES);

    List<ZipFile> jars = new ArrayList<>(classpath.size());
    try {
      for (Path path : classpath) {
        jars.add(new ZipFile(path.toFile()));
      }
      List<List<String>> classEntries =
          jars.parallelStream()
              .map(
                  jar ->
                      jar.stream()
                          .map(ZipEntry::getName)
                          .filter(DesugarClasspathView::isClassFile)
                          .collect(Collectors.toList()))
              .collect(Collectors.toList());
      Map<String, List<Integer>> definingJars = new HashMap<>();
      for (int index = 0; index < classEntries.size(); index++) {
        for (String name : classEntries.get(index)) {
          String internalName = name.substring(0, name.length() - CLASS_SUFFIX.length());
          List<Integer> defining =
              definingJars.computeIfAbsent(internalName, unused -> new ArrayList<>(1));
          if (defining.isEmpty() || defining.get(defining.size() - 1) != index) {
            defining.add(index);
          }
        }
      }

      Set<String> keepWhole = classesD8LooksUpByName(references.classes, definingJars.keySet());
      Map<String, byte[]> view = new ConcurrentHashMap<>();
      Map<String, byte[]> duplicates = new ConcurrentHashMap<>();
      Set<String> seen = new HashSet<>(references.types);
      seen.addAll(keepWhole);
      List<String> frontier = new ArrayList<>(seen);
      while (!frontier.isEmpty()) {
        List<String> related =
            frontier.parallelStream()
                .filter(definingJars::containsKey)
                .flatMap(
                    type -> {
                      List<Integer> defining = Objects.requireNonNull(definingJars.get(type));
                      String entryName = type + CLASS_SUFFIX;
                      List<String> next = new ArrayList<>();
                      for (int i = 0; i < Math.min(2, defining.size()); i++) {
                        byte[] bytes = readEntryUnchecked(jars.get(defining.get(i)), entryName);
                        ClassReader reader = new ClassReader(bytes);
                        HeaderCollector header = new HeaderCollector();
                        reader.accept(
                            header,
                            ClassReader.SKIP_CODE
                                | ClassReader.SKIP_DEBUG
                                | ClassReader.SKIP_FRAMES);
                        next.addAll(header.relatedTypes);
                        byte[] projected =
                            header.keepWholeClass || keepWhole.contains(type)
                                ? bytes
                                : project(reader, memberNames);
                        (i == 0 ? view : duplicates).put(entryName, projected);
                      }
                      return next.stream();
                    })
                .collect(Collectors.toList());
        frontier = new ArrayList<>();
        for (String type : related) {
          if (seen.add(type)) {
            frontier.add(type);
          }
        }
      }

      writeJar(output, new TreeMap<>(view));
      writeJar(duplicatesOutput, new TreeMap<>(duplicates));
    } catch (UncheckedIOException e) {
      throw e.getCause();
    } finally {
      for (ZipFile jar : jars) {
        jar.close();
      }
    }
  }

  /**
   * Returns the classpath types that D8 may look up although the program does not reference them:
   * possible names of synthetic classes for the program's classes, and D8's global synthetic types.
   */
  static Set<String> classesD8LooksUpByName(
      Set<String> programClasses, Set<String> classpathTypes) {
    TreeSet<String> sorted = new TreeSet<>(classpathTypes);
    Set<String> found = new HashSet<>(withPrefix(sorted, GLOBAL_SYNTHETIC_PACKAGE));
    for (String programClass : programClasses) {
      found.addAll(withPrefix(sorted, programClass + SYNTHETIC_CLASS_SEPARATOR));
    }
    return found;
  }

  private static Set<String> withPrefix(TreeSet<String> sorted, String prefix) {
    return Objects.requireNonNull(sorted.subSet(prefix, prefix + Character.MAX_VALUE));
  }

  /** Mirrors D8's {@code ZipUtils.isClassFile}, which decides the entries a classpath jar holds. */
  static boolean isClassFile(String entryName) {
    if (entryName.endsWith("module-info.class")) {
      return false;
    }
    if (entryName.startsWith("META-INF") || entryName.startsWith("/META-INF")) {
      return false;
    }
    return entryName.endsWith(CLASS_SUFFIX);
  }

  /** Collects every UTF-8 constant and every referenced type of the program jar's classes. */
  static ProgramReferences readProgramReferences(Path programJar) throws IOException {
    ProgramReferences references = new ProgramReferences();
    try (ZipFile jar = new ZipFile(programJar.toFile())) {
      jar.stream()
          .map(ZipEntry::getName)
          .filter(DesugarClasspathView::isClassFile)
          .collect(Collectors.toList())
          .parallelStream()
          .forEach(
              name -> {
                references.classes.add(name.substring(0, name.length() - CLASS_SUFFIX.length()));
                try {
                  addConstantPoolReferences(readEntry(jar, name), references);
                } catch (IOException e) {
                  throw new UncheckedIOException(e);
                }
              });
    } catch (UncheckedIOException e) {
      throw e.getCause();
    }
    return references;
  }

  /**
   * Adds the UTF-8 constants of a class file to {@code memberNames}, and the types named by its
   * class constants and by its descriptors and signatures to {@code types}.
   */
  static void addConstantPoolReferences(byte[] classFile, ProgramReferences references)
      throws IOException {
    int count = readUnsignedShort(classFile, 8);
    String[] utf8 = new String[count];
    List<Integer> classNameIndexes = new ArrayList<>();
    int offset = 10;
    for (int i = 1; i < count; i++) {
      int tag = classFile[offset] & 0xff;
      switch (tag) {
        case 1: // Utf8
          int length = readUnsignedShort(classFile, offset + 1);
          utf8[i] =
              isAscii(classFile, offset + 3, length)
                  ? new String(classFile, offset + 3, length, StandardCharsets.ISO_8859_1)
                  : new DataInputStream(new ByteArrayInputStream(classFile, offset + 1, length + 2))
                      .readUTF();
          offset += 3 + length;
          break;
        case 7: // Class
          classNameIndexes.add(readUnsignedShort(classFile, offset + 1));
          offset += 3;
          break;
        case 8: // String
        case 16: // MethodType
        case 19: // Module
        case 20: // Package
          offset += 3;
          break;
        case 15: // MethodHandle
          offset += 4;
          break;
        case 3: // Integer
        case 4: // Float
        case 9: // Fieldref
        case 10: // Methodref
        case 11: // InterfaceMethodref
        case 12: // NameAndType
        case 17: // Dynamic
        case 18: // InvokeDynamic
          offset += 5;
          break;
        case 5: // Long
        case 6: // Double
          offset += 9;
          i++;
          break;
        default:
          throw new IOException("Unknown constant pool tag " + tag + " at offset " + offset);
      }
    }
    for (String value : utf8) {
      if (value != null) {
        references.memberNames.add(value);
        addTypesFromSignature(value, references.types);
      }
    }
    for (int nameIndex : classNameIndexes) {
      String name = utf8[nameIndex];
      if (name != null && !name.startsWith("[")) {
        references.types.add(name);
      }
    }
  }

  /**
   * Adds the internal names of the class types in a field or method descriptor, or in a generic
   * signature, to {@code types}. Other strings add nothing, or names that match no class. Type
   * variables and the names of type parameters add nothing.
   */
  static void addTypesFromSignature(String value, Set<String> types) {
    int length = value.length();
    if (length == 0) {
      return;
    }
    char first = value.charAt(0);
    if (first != '(' && first != 'L' && first != '[' && first != '<') {
      return;
    }
    boolean typeStart = true;
    for (int i = 0; i < length; i++) {
      char c = value.charAt(i);
      if ((c == 'L' || c == 'T') && typeStart) {
        int end = i + 1;
        while (end < length && ";<.:".indexOf(value.charAt(end)) < 0) {
          end++;
        }
        boolean typeParameterName = end < length && value.charAt(end) == ':';
        if (c == 'L' && !typeParameterName && end > i + 1) {
          types.add(value.substring(i + 1, end));
        }
        i = end - 1;
        typeStart = false;
      } else {
        typeStart = "();<>[+-:^*BCDFIJSZV".indexOf(c) >= 0;
      }
    }
  }

  /**
   * Returns the class without code, debug information, {@code kotlin.Metadata} and the members
   * whose name is not in {@code memberNames}, except that a class whose name contains {@code /R$}
   * keeps all of its fields. A string constant at index 2 stays at index 2.
   */
  static byte[] project(ClassReader reader, Set<String> memberNames) {
    boolean keepAllFields =
        Objects.requireNonNull(reader.getClassName()).contains(RESOURCE_CLASS_MARKER);
    ClassWriter writer = new ClassWriter(0);
    String marker = readMarker(reader);
    if (marker != null) {
      // The first constant ASM adds is the marker's UTF-8 entry (index 1), then its string (index
      // 2).
      writer.newConst(marker);
    }
    reader.accept(
        new ClassVisitor(Opcodes.ASM9, writer) {
          @Override
          public @Nullable AnnotationVisitor visitAnnotation(String descriptor, boolean visible) {
            return KOTLIN_METADATA_DESCRIPTOR.equals(descriptor)
                ? null
                : super.visitAnnotation(descriptor, visible);
          }

          @Override
          public @Nullable FieldVisitor visitField(
              int access,
              String name,
              String descriptor,
              @Nullable String signature,
              @Nullable Object value) {
            return keepAllFields || memberNames.contains(name)
                ? super.visitField(access, name, descriptor, signature, value)
                : null;
          }

          @Override
          public @Nullable MethodVisitor visitMethod(
              int access,
              String name,
              String descriptor,
              @Nullable String signature,
              String @Nullable [] exceptions) {
            return memberNames.contains(name)
                ? super.visitMethod(access, name, descriptor, signature, exceptions)
                : null;
          }
        },
        ClassReader.SKIP_CODE | ClassReader.SKIP_DEBUG | ClassReader.SKIP_FRAMES);
    return writer.toByteArray();
  }

  /** Returns the string constant that D8 reads as a possible marker, as D8 reads it. */
  static @Nullable String readMarker(ClassReader reader) {
    if (reader.getItemCount() <= MARKER_CONSTANT_POOL_INDEX
        || reader.getItem(MARKER_CONSTANT_POOL_INDEX) <= 0) {
      return null;
    }
    try {
      Object constant =
          reader.readConst(MARKER_CONSTANT_POOL_INDEX, new char[reader.getMaxStringLength()]);
      return constant instanceof String ? (String) constant : null;
    } catch (IllegalArgumentException e) {
      return null;
    }
  }

  /** Reads a classpath class's supertypes and nest, and whether the view keeps the whole class. */
  private static final class HeaderCollector extends ClassVisitor {
    final List<String> relatedTypes = new ArrayList<>();
    boolean keepWholeClass = false;

    HeaderCollector() {
      super(Opcodes.ASM9);
    }

    @Override
    public void visit(
        int version,
        int access,
        String name,
        @Nullable String signature,
        @Nullable String superName,
        String @Nullable [] interfaces) {
      if (superName != null) {
        relatedTypes.add(superName);
      }
      if (interfaces != null) {
        for (String implemented : interfaces) {
          relatedTypes.add(implemented);
        }
      }
      if ((access & Opcodes.ACC_RECORD) != 0) {
        keepWholeClass = true;
      }
    }

    @Override
    public void visitNestHost(String nestHost) {
      relatedTypes.add(nestHost);
      keepWholeClass = true;
    }

    @Override
    public void visitNestMember(String nestMember) {
      relatedTypes.add(nestMember);
      keepWholeClass = true;
    }
  }

  private static byte[] readEntry(ZipFile jar, String entryName) throws IOException {
    ZipEntry entry = jar.getEntry(entryName);
    if (entry == null) {
      throw new IOException("Missing " + entryName + " in " + jar.getName());
    }
    try (InputStream input = Objects.requireNonNull(jar.getInputStream(entry))) {
      return ByteStreams.toByteArray(input);
    }
  }

  private static void writeJar(Path path, Map<String, byte[]> entries) throws IOException {
    try (OutputStream file = new BufferedOutputStream(Files.newOutputStream(path));
        ZipOutputStream zip = new ZipOutputStream(file)) {
      zip.setLevel(Deflater.BEST_SPEED);
      for (Map.Entry<String, byte[]> entry : entries.entrySet()) {
        zip.putNextEntry(new ZipEntry(entry.getKey()));
        zip.write(entry.getValue());
        zip.closeEntry();
      }
    }
    ZipScrubber.scrubZip(path);
  }

  /** Whether the modified UTF-8 bytes are ASCII, which decodes the same as ISO-8859-1. */
  private static boolean isAscii(byte[] bytes, int offset, int length) {
    for (int i = offset; i < offset + length; i++) {
      if (bytes[i] < 0) {
        return false;
      }
    }
    return true;
  }

  private static byte[] readEntryUnchecked(ZipFile jar, String entryName) {
    try {
      return readEntry(jar, entryName);
    } catch (IOException e) {
      throw new UncheckedIOException(e);
    }
  }

  private static int readUnsignedShort(byte[] bytes, int offset) {
    return ((bytes[offset] & 0xff) << 8) | (bytes[offset + 1] & 0xff);
  }
}
