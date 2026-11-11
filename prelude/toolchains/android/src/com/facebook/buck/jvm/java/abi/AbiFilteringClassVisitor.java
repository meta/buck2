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

import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.Preconditions;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Objects;
import java.util.Set;
import org.jspecify.annotations.Nullable;
import org.objectweb.asm.Attribute;
import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassVisitor;
import org.objectweb.asm.FieldVisitor;
import org.objectweb.asm.Handle;
import org.objectweb.asm.Label;
import org.objectweb.asm.MethodVisitor;
import org.objectweb.asm.Opcodes;
import org.objectweb.asm.Type;
import org.objectweb.asm.tree.AnnotationNode;
import org.objectweb.asm.tree.ClassNode;
import org.objectweb.asm.tree.FieldNode;
import org.objectweb.asm.tree.MethodNode;

/** A {@link ClassVisitor} that only passes to its delegate events for the class's ABI. */
@Nullsafe(Nullsafe.Mode.LOCAL)
class AbiFilteringClassVisitor extends ClassVisitor {

  private final List<String> methodsWithRetainedBody;
  @Nullable private final Set<String> referencedClassNames;

  @Nullable private String name;
  @Nullable private String outerName = null;
  private int classAccess;
  private int classVersion;
  private boolean hasVisibleConstructor = false;
  private final Set<String> includedInnerClasses = new HashSet<>();
  private final List<String> nestMembers = new ArrayList<>();
  private boolean keepSynthetic;
  private final Set<String> hiddenDeprecatedMethods;
  // When the visited class is a private Kotlin nested class of strippable kind, no member
  // is consumer-reachable: drop every field and non-constructor method, keeping only the
  // class shell (plus the synthesized default constructor from visitEnd) so
  // InnerClasses/constant-pool/@Metadata references stay valid. Constructors still flow
  // through the normal filter so a public constructor is kept exactly as source-only-abi
  // keeps it, as do @JvmStatic/@JvmField members (keptMemberKeys), whose outer-class
  // static copies callers genuinely reach.
  private final boolean stripAllMembers;
  // JVM name+descriptor keys exempt from stripAllMembers. Empty unless stripping.
  private final Set<String> keptMemberKeys;

  public AbiFilteringClassVisitor(
      ClassVisitor cv, List<String> methodsWithRetainedBody, boolean keepSynthetic) {
    this(cv, methodsWithRetainedBody, null, keepSynthetic, Collections.emptySet());
  }

  public AbiFilteringClassVisitor(
      ClassVisitor cv,
      List<String> methodsWithRetainedBody,
      @Nullable Set<String> referencedClassNames,
      boolean keepSynthetic) {
    this(cv, methodsWithRetainedBody, referencedClassNames, keepSynthetic, Collections.emptySet());
  }

  public AbiFilteringClassVisitor(
      ClassVisitor cv,
      List<String> methodsWithRetainedBody,
      @Nullable Set<String> referencedClassNames,
      boolean keepSynthetic,
      Set<String> hiddenDeprecatedMethods) {
    this(
        cv,
        methodsWithRetainedBody,
        referencedClassNames,
        keepSynthetic,
        hiddenDeprecatedMethods,
        false,
        Collections.emptySet());
  }

  public AbiFilteringClassVisitor(
      ClassVisitor cv,
      List<String> methodsWithRetainedBody,
      @Nullable Set<String> referencedClassNames,
      boolean keepSynthetic,
      Set<String> hiddenDeprecatedMethods,
      boolean stripAllMembers,
      Set<String> keptMemberKeys) {
    super(Opcodes.ASM9, cv);
    this.methodsWithRetainedBody = methodsWithRetainedBody;
    this.referencedClassNames = referencedClassNames;
    this.keepSynthetic = keepSynthetic;
    this.hiddenDeprecatedMethods = hiddenDeprecatedMethods;
    this.stripAllMembers = stripAllMembers;
    this.keptMemberKeys = keptMemberKeys;
  }

  /**
   * Finds synthetic methods annotated with {@code @kotlin.Deprecated(level = HIDDEN)}.
   *
   * <p>Kotlin compiles {@code DeprecationLevel.HIDDEN} members as {@code ACC_SYNTHETIC} but keeps
   * them in {@code @kotlin.Metadata}. kotlinc needs the JVM method (with its {@code @Deprecated}
   * marker) to hide the overload during resolution; dropping it makes calls ambiguous that Gradle
   * (full jars) resolves. Returns keys of the form {@code name + descriptor}.
   */
  static Set<String> findHiddenDeprecatedMethods(ClassNode classNode) {
    if (classNode.methods == null) {
      return Collections.emptySet();
    }
    Set<String> result = new HashSet<>();
    for (MethodNode method : classNode.methods) {
      if ((method.access & Opcodes.ACC_SYNTHETIC) == 0) {
        continue;
      }
      if (isHiddenDeprecated(method.visibleAnnotations)
          || isHiddenDeprecated(method.invisibleAnnotations)) {
        result.add(method.name + method.desc);
      }
    }
    return result;
  }

  /**
   * Finds members that kotlinc also emits as statics on the outer class ({@code @JvmStatic}/
   * {@code @JvmField}). Kotlin metadata omits these annotations, so read bytecode instead. A
   * synthetic {@code $annotations} carrier keeps both itself and the accessor it names. Returns
   * keys of the form {@code name + descriptor}.
   */
  static Set<String> findJvmStaticMemberKeys(ClassNode classNode) {
    Set<String> result = new HashSet<>();
    if (classNode.methods != null) {
      for (MethodNode method : classNode.methods) {
        if (!hasJvmStaticOrJvmField(method.visibleAnnotations)
            && !hasJvmStaticOrJvmField(method.invisibleAnnotations)) {
          continue;
        }
        String methodName = Objects.requireNonNull(method.name);
        result.add(methodName + method.desc);
        if (methodName.endsWith("$annotations")) {
          String accessorName =
              methodName.substring(0, methodName.length() - "$annotations".length());
          for (MethodNode candidate : classNode.methods) {
            if (Objects.requireNonNull(candidate.name).equals(accessorName)
                && isAccessorShape(accessorName, Objects.requireNonNull(candidate.desc))) {
              result.add(candidate.name + candidate.desc);
            }
          }
        }
      }
    }
    if (classNode.fields != null) {
      for (FieldNode field : classNode.fields) {
        if (hasJvmStaticOrJvmField(field.visibleAnnotations)
            || hasJvmStaticOrJvmField(field.invisibleAnnotations)) {
          result.add(field.name + field.desc);
        }
      }
    }
    return result;
  }

  /**
   * Whether a same-named method has the JVM shape of the Kotlin accessor a {@code $annotations}
   * carrier names: setters take one argument and return void, getters take none. A Kotlin accessor
   * cannot share name and shape with an unrelated overload (same JVM signature differing only in
   * return type is a platform clash), so this keeps the accessor and its bridges while dropping
   * unrelated overloads.
   */
  private static boolean isAccessorShape(String accessorName, String desc) {
    Type methodType = Type.getMethodType(desc);
    if (accessorName.startsWith("set")) {
      return methodType.getArgumentTypes().length == 1
          && methodType.getReturnType().equals(Type.VOID_TYPE);
    }
    return methodType.getArgumentTypes().length == 0;
  }

  private static boolean hasJvmStaticOrJvmField(@Nullable List<AnnotationNode> annotations) {
    if (annotations == null) {
      return false;
    }
    for (AnnotationNode annotation : annotations) {
      if ("Lkotlin/jvm/JvmStatic;".equals(annotation.desc)
          || "Lkotlin/jvm/JvmField;".equals(annotation.desc)) {
        return true;
      }
    }
    return false;
  }

  private static boolean isHiddenDeprecated(@Nullable List<AnnotationNode> annotations) {
    if (annotations == null) {
      return false;
    }
    for (AnnotationNode annotation : annotations) {
      if (!"Lkotlin/Deprecated;".equals(annotation.desc)) {
        continue;
      }
      if (annotation.values == null) {
        return false;
      }
      for (int i = 0; i + 1 < annotation.values.size(); i += 2) {
        if (!"level".equals(annotation.values.get(i))) {
          continue;
        }
        Object level = annotation.values.get(i + 1);
        if (level instanceof String[]) {
          String[] enumValue = (String[]) level;
          if (enumValue.length == 2
              && "Lkotlin/DeprecationLevel;".equals(enumValue[0])
              && "HIDDEN".equals(enumValue[1])) {
            return true;
          }
        }
      }
      return false;
    }
    return false;
  }

  @Override
  public void visit(
      int version,
      int access,
      String name,
      @Nullable String signature,
      @Nullable String superName,
      String @Nullable [] interfaces) {
    this.name = name;
    classAccess = access;
    classVersion = version;
    super.visit(version, access, name, signature, superName, interfaces);
  }

  @Override
  public void visitInnerClass(String name, String outerName, String innerName, int access) {
    if (name.equals(this.name)) {
      this.outerName = outerName;
    }

    if (!shouldIncludeInnerClass(access, name, outerName)) {
      return;
    }

    includedInnerClasses.add(name);
    super.visitInnerClass(name, outerName, innerName, access);
  }

  @Override
  public void visitNestMember(String nestMember) {
    Preconditions.checkState(classVersion >= Opcodes.V11);
    nestMembers.add(nestMember);
  }

  @Override
  @Nullable
  public FieldVisitor visitField(
      int access, String name, String desc, @Nullable String signature, @Nullable Object value) {
    if (stripAllMembers && !keptMemberKeys.contains(name + desc)) {
      return null;
    }
    if (!shouldInclude(access)) {
      return null;
    }
    return super.visitField(access, name, desc, signature, value);
  }

  @Override
  @Nullable
  public MethodVisitor visitMethod(
      int access,
      String name,
      String desc,
      @Nullable String signature,
      String @Nullable [] exceptions) {
    if (stripAllMembers && !name.equals("<init>") && !keptMemberKeys.contains(name + desc)) {
      return null;
    }
    if (methodsWithRetainedBody.contains(name)
        || (name.endsWith("$annotations"))
        || (name.endsWith("$default")
            && methodsWithRetainedBody.contains(name.substring(0, name.length() - 8)))
        || (name.endsWith("$$forInline")
            && methodsWithRetainedBody.contains(
                name.substring(0, name.length() - "$$forInline".length())))) {
      if (name.equals("<init>") && (access & (Opcodes.ACC_PRIVATE | Opcodes.ACC_SYNTHETIC)) == 0) {
        hasVisibleConstructor = true;
      }

      return super.visitMethod(access, name, desc, signature, exceptions);
    }

    // Per JVMS8 2.9, "Class and interface initialization methods are invoked
    // implicitly by the Java Virtual Machine; they are never invoked directly from any
    // Java Virtual Machine instruction, but are invoked only indirectly as part of the class
    // initialization process." Thus we don't need to emit a stub of <clinit>.
    boolean isHiddenDeprecatedMethod =
        (access & Opcodes.ACC_SYNTHETIC) != 0 && hiddenDeprecatedMethods.contains(name + desc);
    if ((!shouldInclude(access) && !isHiddenDeprecatedMethod)
        || (name.equals("<clinit>") && (access & Opcodes.ACC_STATIC) > 0)) {
      return null;
    }

    // We don't stub private constructors, but if stripping these constructors results in no
    // constructors at all, we want to include a default private constructor. This is because
    // removing all these private methods will make the class look like it has no constructors at
    // all, which is not possible. We track if this class has a public, non-synthetic constructor
    // and is not an interface or annotation to determine if a default private constructor is
    // generated when visitEnd() is called.
    if (name.equals("<init>") && (access & Opcodes.ACC_SYNTHETIC) == 0) {
      hasVisibleConstructor = true;
    }

    // Bridge methods are created by the compiler, and don't appear in source. It would be nice to
    // skip them, but they're used by the compiler to cover the fact that type erasure has occurred.
    // Normally the compiler adds these as public methods, but if you're compiling against a stub
    // produced using our ABI generator, we don't want people calling it accidentally. Oh well, I
    // guess it happens IRL too.
    //
    // Synthetic methods are also generated by the compiler, unless it's one of the methods named in
    // section 4.7.8 of the JVM spec, which are "<init>" and "Enum.valueOf()" and "Enum.values".
    // None of these are actually harmful to the ABI, so we allow synthetic methods through.
    // http://docs.oracle.com/javase/specs/jvms/se7/html/jvms-4.html#jvms-4.7.8
    return new SkipCodeMethodVisitor(
        Opcodes.ASM9, super.visitMethod(access, name, desc, signature, exceptions));
  }

  @Override
  public void visitEnd() {
    if (!hasVisibleConstructor && !isInterface(classAccess) && !isAnnotation(classAccess)) {
      String desc;
      if (isEnum(classAccess)) {
        desc =
            Type.getMethodType(
                    Type.VOID_TYPE, Type.getObjectType("java/lang/String"), Type.INT_TYPE)
                .getDescriptor();
      } else {
        desc =
            outerName == null
                ? Type.getMethodType(Type.VOID_TYPE).getDescriptor()
                : Type.getMethodType(Type.VOID_TYPE, Type.getObjectType(outerName)).getDescriptor();
      }
      super.visitMethod(Opcodes.ACC_PRIVATE, "<init>", desc, null, null);
    }

    // Filter nest members to included inner classes. Other nest members don't matter for ABI
    // purposes. Even though other members will point to this class as their nest host, this
    // asymmetry only matters at runtime when access control checks happen.
    for (String nestMember : nestMembers) {
      if (includedInnerClasses.contains(nestMember)) {
        super.visitNestMember(nestMember);
      }
    }

    super.visitEnd();
  }

  private boolean shouldIncludeInnerClass(int access, String name, @Nullable String outerName) {
    if (referencedClassNames == null || referencedClassNames.contains(name)) {
      // Either it's the first pass, and we're not filtering inner classes yet,
      // or it's the second one, and this inner class is part of the ABI and should
      // therefore be included
      return true;
    }

    String currentClassName = Objects.requireNonNull(this.name);
    if (name.equals(currentClassName)) {
      // Must always include the entry for our own class, since that's what makes it an inner class.
      return true;
    }

    boolean isAnonymousOrLocalClass = (outerName == null);
    if (isAnonymousOrLocalClass) {
      // Anonymous and local classes are never part of the ABI.
      return false;
    }

    if ((access & (Opcodes.ACC_SYNTHETIC | Opcodes.ACC_BRIDGE)) == Opcodes.ACC_SYNTHETIC) {
      // Don't include synthetic classes
      return false;
    }

    return currentClassName.equals(outerName);
  }

  private boolean shouldInclude(int access) {
    if ((access & Opcodes.ACC_PRIVATE) == Opcodes.ACC_PRIVATE) {
      return false;
    }

    int accessFilter = (Opcodes.ACC_SYNTHETIC | Opcodes.ACC_BRIDGE);
    if (!isInterface(classAccess) && !isAbstract(classAccess)) {
      // If this isn't an interface or abstract class, then we can include deprecated methods for
      // backwards compat.
      accessFilter |= Opcodes.ACC_DEPRECATED;
    }

    // for kotlin compilation in some cases when JvmSynthetic annotation is used together with
    // JvmName
    // we need to keep the method even if it is synthetic
    return keepSynthetic || ((access & accessFilter) != Opcodes.ACC_SYNTHETIC);
  }

  private boolean isInterface(int access) {
    return (access & Opcodes.ACC_INTERFACE) > 0;
  }

  private boolean isAbstract(int access) {
    return (access & Opcodes.ACC_ABSTRACT) > 0;
  }

  private boolean isAnnotation(int access) {
    return (access & Opcodes.ACC_ANNOTATION) > 0;
  }

  private boolean isEnum(int access) {
    return (access & Opcodes.ACC_ENUM) > 0;
  }

  /** A {@link MethodVisitor} that replicates the behavior of {@link ClassReader#SKIP_CODE}. */
  private static class SkipCodeMethodVisitor extends MethodVisitor {
    public SkipCodeMethodVisitor(int api, @Nullable MethodVisitor methodVisitor) {
      super(api, methodVisitor);
    }

    @Override
    public void visitAttribute(Attribute attribute) {}

    @Override
    public void visitCode() {}

    @Override
    public void visitFrame(int type, int numLocal, Object[] local, int numStack, Object[] stack) {}

    @Override
    public void visitInsn(int opcode) {}

    @Override
    public void visitIntInsn(int opcode, int operand) {}

    @Override
    public void visitVarInsn(int opcode, int var) {}

    @Override
    public void visitTypeInsn(int opcode, String type) {}

    @Override
    public void visitFieldInsn(int opcode, String owner, String name, String descriptor) {}

    /**
     * @deprecated
     */
    @Override
    @Deprecated
    public void visitMethodInsn(int opcode, String owner, String name, String descriptor) {}

    @Override
    public void visitMethodInsn(
        int opcode, String owner, String name, String descriptor, boolean isInterface) {}

    @Override
    public void visitInvokeDynamicInsn(
        String name,
        String descriptor,
        Handle bootstrapMethodHandle,
        Object... bootstrapMethodArguments) {}

    @Override
    public void visitJumpInsn(int opcode, Label label) {}

    @Override
    public void visitLabel(Label label) {}

    @Override
    public void visitLdcInsn(Object value) {}

    @Override
    public void visitIincInsn(int var, int increment) {}

    @Override
    public void visitTableSwitchInsn(int min, int max, Label dflt, Label... labels) {}

    @Override
    public void visitLookupSwitchInsn(Label dflt, int[] keys, Label[] labels) {}

    @Override
    public void visitMultiANewArrayInsn(String descriptor, int numDimensions) {}

    @Override
    public void visitTryCatchBlock(Label start, Label end, Label handler, String type) {}

    @Override
    public void visitLocalVariable(
        String name, String descriptor, String signature, Label start, Label end, int index) {}

    @Override
    public void visitLineNumber(int line, Label start) {}

    @Override
    public void visitMaxs(int maxStack, int maxLocals) {}
  }
}
