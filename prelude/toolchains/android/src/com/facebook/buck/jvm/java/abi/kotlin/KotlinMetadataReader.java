/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.abi.kotlin;

import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import java.util.Arrays;
import java.util.Collection;
import java.util.Iterator;
import java.util.List;
import java.util.Objects;
import java.util.Set;
import java.util.stream.Stream;
import kotlin.Metadata;
import kotlin.metadata.Attributes;
import kotlin.metadata.KmClass;
import kotlin.metadata.KmDeclarationContainer;
import kotlin.metadata.KmFunction;
import kotlin.metadata.KmProperty;
import kotlin.metadata.Visibility;
import kotlin.metadata.jvm.JvmExtensionsKt;
import kotlin.metadata.jvm.JvmFieldSignature;
import kotlin.metadata.jvm.JvmMetadataUtil;
import kotlin.metadata.jvm.JvmMethodSignature;
import kotlin.metadata.jvm.KotlinClassMetadata;
import org.objectweb.asm.tree.AnnotationNode;

/** Utilities to read Kotlin class metadata. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class KotlinMetadataReader {

  /**
   * Opaque handle to parsed Kotlin metadata. Callers should obtain this via {@link #readMetadata}
   * and pass it to {@link #getInlineFunctions} and {@link #isFilePrivateClass} to avoid redundant
   * parsing.
   */
  public static final class ParsedMetadata {
    final KotlinClassMetadata metadata;

    ParsedMetadata(KotlinClassMetadata metadata) {
      this.metadata = metadata;
    }
  }

  /**
   * Parses the @kotlin.Metadata annotation into a reusable handle. Call once and pass the result to
   * both {@link #getInlineFunctions} and {@link #isFilePrivateClass}.
   *
   * <p>The strict read only understands versions up to the bundled kotlin-metadata-jvm (e.g. 2.2.0
   * reads up to 2.3.0), so a newer version (e.g. 2.4.0) falls back to a lenient read. Metadata the
   * lenient reader cannot parse fails the action.
   */
  public static ParsedMetadata readMetadata(AnnotationNode annotationNode) {
    Metadata classHeader = createHeader(annotationNode);
    KotlinClassMetadata metadata;
    try {
      metadata = KotlinClassMetadata.readStrict(classHeader);
      if (metadata != null) {
        return new ParsedMetadata(metadata);
      }
    } catch (IllegalArgumentException strictFailure) {
      // Fall through to the lenient read below.
    }
    try {
      metadata = KotlinClassMetadata.readLenient(classHeader);
    } catch (IllegalArgumentException lenientFailure) {
      throw new IllegalArgumentException(
          "Unparsable Kotlin metadata: kind="
              + classHeader.k()
              + " version="
              + Arrays.toString(classHeader.mv())
              + "; strict and lenient reads failed",
          lenientFailure);
    }
    if (metadata == null) {
      throw new IllegalArgumentException(
          "Unparsable Kotlin metadata: kind="
              + classHeader.k()
              + " version="
              + Arrays.toString(classHeader.mv())
              + "; strict and lenient reads failed");
    }
    return new ParsedMetadata(metadata);
  }

  /**
   * Finds the inline functions of a Kotlin class from its parsed metadata.
   *
   * @param parsed the result of {@link #readMetadata}
   */
  public static ImmutableList<String> getInlineFunctions(ParsedMetadata parsed) {
    KotlinClassMetadata metadata = parsed.metadata;
    KmDeclarationContainer container;
    if (metadata instanceof KotlinClassMetadata.Class) {
      container = ((KotlinClassMetadata.Class) metadata).getKmClass();
    } else if (metadata instanceof KotlinClassMetadata.FileFacade) {
      container = ((KotlinClassMetadata.FileFacade) metadata).getKmPackage();
    } else if (metadata instanceof KotlinClassMetadata.MultiFileClassPart) {
      container = ((KotlinClassMetadata.MultiFileClassPart) metadata).getKmPackage();
    } else {
      return ImmutableList.of();
    }

    ImmutableList<String> inlineFunctions =
        container.getFunctions().stream()
            .filter(Attributes::isInline)
            .map(JvmExtensionsKt::getSignature)
            .filter(Objects::nonNull)
            .map(JvmMethodSignature::getName)
            .collect(ImmutableList.toImmutableList());

    ImmutableList<String> inlineProperties =
        container.getProperties().stream()
            .filter(
                it ->
                    Attributes.isInline(it.getGetter())
                        || it.getSetter() != null && Attributes.isInline(it.getSetter()))
            .map(KmProperty::getName)
            .collect(ImmutableList.toImmutableList());

    ImmutableList<String> inlineGetters =
        container.getProperties().stream()
            .filter(it -> Attributes.isInline(it.getGetter()))
            .map(JvmExtensionsKt::getGetterSignature)
            .filter(Objects::nonNull)
            .map(JvmMethodSignature::getName)
            .collect(ImmutableList.toImmutableList());

    ImmutableList<String> inlineSetters =
        container.getProperties().stream()
            .filter(it -> it.getSetter() != null && Attributes.isInline(it.getSetter()))
            .map(JvmExtensionsKt::getSetterSignature)
            .filter(Objects::nonNull)
            .map(JvmMethodSignature::getName)
            .collect(ImmutableList.toImmutableList());

    return Stream.of(inlineFunctions, inlineProperties, inlineGetters, inlineSetters)
        .flatMap(Collection::stream)
        .distinct()
        .sorted()
        .collect(ImmutableList.toImmutableList());
  }

  /**
   * Checks if a Kotlin class is file-private (declared with `private` visibility at the file
   * level). Such classes are compiled to package-private in bytecode, so they pass the standard
   * ACC_PRIVATE check, but they should NOT be included in class-abi since they are not part of the
   * module's public API. Source-only-abi correctly excludes them; this method enables class-abi to
   * match that behavior.
   *
   * @param parsed the result of {@link #readMetadata}
   */
  public static boolean isFilePrivateClass(ParsedMetadata parsed) {
    KotlinClassMetadata metadata = parsed.metadata;
    if (metadata instanceof KotlinClassMetadata.Class) {
      KmClass kmClass = ((KotlinClassMetadata.Class) metadata).getKmClass();
      if (Attributes.getVisibility(kmClass) != Visibility.PRIVATE) {
        return false;
      }
      // Distinguish file-private top-level classes from private inner/nested classes.
      // In Kotlin metadata, inner classes use '.' as a separator (e.g. "com/example/Outer.Inner"),
      // while top-level classes have no '.' after the last '/' (e.g. "com/example/TopLevel").
      String className = kmClass.getName();
      int lastSlash = className.lastIndexOf('/');
      String simpleName = lastSlash >= 0 ? className.substring(lastSlash + 1) : className;
      return !simpleName.contains(".");
    }
    return false;
  }

  /**
   * Checks if a Kotlin class is a private nested class (a member class whose own Kotlin visibility
   * is private). Its members are unreachable: Kotlin consumers see the metadata-private owner, and
   * Java consumers see a non-public nested class. Callers keep the class shell (its file must exist
   * for InnerClasses/constant-pool/@Metadata references) but drop all of its members.
   *
   * @param parsed the result of {@link #readMetadata}
   */
  public static boolean isPrivateNestedClass(ParsedMetadata parsed) {
    KotlinClassMetadata metadata = parsed.metadata;
    if (metadata instanceof KotlinClassMetadata.Class) {
      KmClass kmClass = ((KotlinClassMetadata.Class) metadata).getKmClass();
      if (Attributes.getVisibility(kmClass) != Visibility.PRIVATE) {
        return false;
      }
      String className = kmClass.getName();
      int lastSlash = className.lastIndexOf('/');
      String simpleName = lastSlash >= 0 ? className.substring(lastSlash + 1) : className;
      return simpleName.contains(".");
    }
    return false;
  }

  /**
   * Whether a private nested class should have its members stripped: only plain classes, interfaces
   * and objects (including companions). Value, enum and annotation classes keep the old behavior,
   * matching source-only-abi, whose backend lowerings read their members.
   *
   * @param parsed the result of {@link #readMetadata}
   */
  public static boolean shouldStripNestedMembers(ParsedMetadata parsed) {
    KotlinClassMetadata metadata = parsed.metadata;
    if (!(metadata instanceof KotlinClassMetadata.Class)) {
      return false;
    }
    if (!isPrivateNestedClass(parsed)) {
      return false;
    }
    KmClass kmClass = ((KotlinClassMetadata.Class) metadata).getKmClass();
    switch (Attributes.getKind(kmClass)) {
      case CLASS:
      case INTERFACE:
      case OBJECT:
      case COMPANION_OBJECT:
        return !Attributes.isValue(kmClass);
      default:
        return false;
    }
  }

  /**
   * Rewrites a stub class's @kotlin.Metadata in place to drop functions and properties whose JVM
   * members were stripped from the stub, keeping the Kotlin-level ABI aligned with the bytecode.
   * Only the d1/d2 payload is replaced; kind, version and extras are preserved, and constructors
   * are untouched (member stripping keeps them).
   *
   * <p>Strict-parse only: metadata the bundled reader cannot parse strictly, or that fails to
   * re-serialize, is left untouched, failing closed to the historical pass-through behavior.
   * Members without a JVM signature are kept. When nothing would be dropped the original bytes are
   * kept, so semantically-identical stubs stay byte-identical.
   *
   * @param annotationNode the @kotlin.Metadata annotation on the stub class
   * @param keptMemberKeys JVM name+descriptor keys retained in the stub
   * @return true if the annotation was rewritten
   */
  public static boolean rewriteMetadataForStrippedMembers(
      AnnotationNode annotationNode, Set<String> keptMemberKeys) {
    try {
      KotlinClassMetadata metadata = KotlinClassMetadata.readStrict(createHeader(annotationNode));
      if (!(metadata instanceof KotlinClassMetadata.Class)) {
        return false;
      }
      KotlinClassMetadata.Class classMetadata = (KotlinClassMetadata.Class) metadata;
      KmClass kmClass = classMetadata.getKmClass();
      int functionCount = kmClass.getFunctions().size();
      int propertyCount = kmClass.getProperties().size();
      kmClass.getFunctions().removeIf(f -> !isKeptFunction(f, keptMemberKeys));
      kmClass.getProperties().removeIf(p -> !isKeptProperty(p, keptMemberKeys));
      if (kmClass.getFunctions().size() == functionCount
          && kmClass.getProperties().size() == propertyCount) {
        return false;
      }
      Metadata rewritten =
          new KotlinClassMetadata.Class(
                  kmClass, classMetadata.getVersion(), classMetadata.getFlags())
              .write();
      replacePayload(annotationNode, rewritten);
      return true;
    } catch (RuntimeException e) {
      return false;
    }
  }

  private static boolean isKeptFunction(KmFunction function, Set<String> keptMemberKeys) {
    JvmMethodSignature signature = JvmExtensionsKt.getSignature(function);
    if (signature == null) {
      return true;
    }
    if (isPrivateOrLocal(Attributes.getVisibility(function))) {
      return false;
    }
    return keptMemberKeys.contains(signature.getName() + signature.getDescriptor());
  }

  private static boolean isKeptProperty(KmProperty property, Set<String> keptMemberKeys) {
    JvmFieldSignature field = JvmExtensionsKt.getFieldSignature(property);
    JvmMethodSignature getter = JvmExtensionsKt.getGetterSignature(property);
    JvmMethodSignature setter = JvmExtensionsKt.getSetterSignature(property);
    if (field == null && getter == null && setter == null) {
      return true;
    }
    if (isPrivateOrLocal(Attributes.getVisibility(property))) {
      return false;
    }
    if ((field != null && keptMemberKeys.contains(field.getName() + field.getDescriptor()))
        || (getter != null && keptMemberKeys.contains(getter.getName() + getter.getDescriptor()))
        || (setter != null && keptMemberKeys.contains(setter.getName() + setter.getDescriptor()))) {
      return true;
    }
    // @JvmField/const members live on the outer class as statics, so no companion member key
    // matches them; they are exactly the field-only properties. Private ones feed no reachable
    // static and stay stripped, matching the bytecode rule.
    return (field != null && getter == null && setter == null) || Attributes.isConst(property);
  }

  private static boolean isPrivateOrLocal(Visibility visibility) {
    return visibility == Visibility.PRIVATE
        || visibility == Visibility.PRIVATE_TO_THIS
        || visibility == Visibility.LOCAL;
  }

  private static void replacePayload(AnnotationNode annotationNode, Metadata rewritten) {
    List<String> d1 = Arrays.asList(rewritten.d1());
    List<String> d2 = Arrays.asList(rewritten.d2());
    List<Object> values = Objects.requireNonNull(annotationNode.values);
    for (int i = 0; i + 1 < values.size(); i += 2) {
      if ("d1".equals(values.get(i))) {
        values.set(i + 1, d1);
      } else if ("d2".equals(values.get(i))) {
        values.set(i + 1, d2);
      }
    }
  }

  /**
   * Finds the function names of a Kotlin class from its parsed metadata.
   *
   * @param parsed the result of {@link #readMetadata}
   */
  public static ImmutableList<String> getFunctionNames(ParsedMetadata parsed) {
    KotlinClassMetadata metadata = parsed.metadata;
    if (!(metadata instanceof KotlinClassMetadata.Class)) {
      return ImmutableList.of();
    }
    return ((KotlinClassMetadata.Class) metadata)
        .getKmClass().getFunctions().stream()
            .map(KmFunction::getName)
            .collect(ImmutableList.toImmutableList());
  }

  /**
   * Finds the property names of a Kotlin class from its parsed metadata.
   *
   * @param parsed the result of {@link #readMetadata}
   */
  public static ImmutableList<String> getPropertyNames(ParsedMetadata parsed) {
    KotlinClassMetadata metadata = parsed.metadata;
    if (!(metadata instanceof KotlinClassMetadata.Class)) {
      return ImmutableList.of();
    }
    return ((KotlinClassMetadata.Class) metadata)
        .getKmClass().getProperties().stream()
            .map(KmProperty::getName)
            .collect(ImmutableList.toImmutableList());
  }

  /**
   * Converts the given AnnotationNode representing the @kotlin.Metadata annotation into
   * KotlinClassHeader, to be able to use it in KotlinClassMetadata.read.
   */
  private static Metadata createHeader(AnnotationNode node) {
    Integer kind = null;
    int[] metadataVersion = null;
    String[] data1 = null;
    String[] data2 = null;
    String extraString = null;
    String packageName = null;
    Integer extraInt = null;

    Iterator<Object> it = Objects.requireNonNull(node.values).iterator();
    while (it.hasNext()) {
      String name = (String) it.next();
      Object value = it.next();

      switch (name) {
        case "k":
          kind = (Integer) value;
          break;
        case "mv":
          metadataVersion = listToIntArray(value);
          break;
        case "d1":
          data1 = listToStringArray(value);
          break;
        case "d2":
          data2 = listToStringArray(value);
          break;
        case "xs":
          extraString = (String) value;
          break;
        case "pn":
          packageName = (String) value;
          break;
        case "xi":
          extraInt = (Integer) value;
          break;
      }
    }

    return JvmMetadataUtil.Metadata(
        kind, metadataVersion, data1, data2, extraString, packageName, extraInt);
  }

  @SuppressWarnings("unchecked")
  private static int[] listToIntArray(Object list) {
    return ((List<Integer>) list).stream().mapToInt(i -> i).toArray();
  }

  @SuppressWarnings("unchecked")
  private static String[] listToStringArray(Object list) {
    return ((List<String>) list).toArray(new String[0]);
  }
}
