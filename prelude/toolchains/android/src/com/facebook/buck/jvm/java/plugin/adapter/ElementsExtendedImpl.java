/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.plugin.adapter;

import com.facebook.buck.jvm.java.lang.model.BridgeMethod;
import com.facebook.buck.jvm.java.lang.model.ClassFileBridges;
import com.facebook.buck.jvm.java.lang.model.ClassFileTypes;
import com.facebook.buck.jvm.java.lang.model.ElementsExtended;
import com.facebook.buck.jvm.java.lang.model.ErasedTypes;
import com.facebook.buck.jvm.java.lang.model.MoreAnnotations;
import com.facebook.buck.jvm.java.lang.model.MoreElements;
import com.facebook.infer.annotation.Nullsafe;
import com.sun.source.tree.ClassTree;
import com.sun.source.tree.CompilationUnitTree;
import com.sun.source.tree.ExpressionTree;
import com.sun.source.tree.ImportTree;
import com.sun.source.tree.MethodTree;
import com.sun.source.tree.Tree;
import com.sun.source.tree.VariableTree;
import com.sun.source.util.TreePath;
import com.sun.source.util.Trees;
import java.nio.file.Path;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Deque;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.function.Function;
import javax.lang.model.element.AnnotationMirror;
import javax.lang.model.element.Element;
import javax.lang.model.element.ExecutableElement;
import javax.lang.model.element.Modifier;
import javax.lang.model.element.Name;
import javax.lang.model.element.TypeElement;
import javax.lang.model.element.VariableElement;
import javax.lang.model.type.ArrayType;
import javax.lang.model.type.DeclaredType;
import javax.lang.model.type.ExecutableType;
import javax.lang.model.type.TypeKind;
import javax.lang.model.type.TypeMirror;
import javax.lang.model.type.TypeVariable;
import javax.lang.model.util.ElementFilter;
import javax.lang.model.util.Elements;
import javax.lang.model.util.Types;
import org.jspecify.annotations.Nullable;

/**
 * Wraps and extends {@link javax.lang.model.util.Elements} with methods that cannot be added as
 * pure extension methods on {@link MoreElements} because they require per-instance state.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ElementsExtendedImpl extends DelegatingElements implements ElementsExtended {

  private final Map<TypeElement, Map<Name, List<ExecutableElement>>> declaredMethodsMaps =
      new HashMap<>();
  private final Map<TypeElement, Map<Name, List<ExecutableElement>>> allMethodsMaps =
      new HashMap<>();
  private final BridgeMethods bridgeMethods;
  private final Map<TypeElement, Optional<Map<String, List<ExecutableElement>>>> classFileBridges =
      new HashMap<>();
  private List<Path> localClassDirectories = Collections.emptyList();
  private final Types types;
  private final Trees trees;

  public ElementsExtendedImpl(Elements inner, Types types, Trees trees) {
    super(inner);
    this.types = types;
    this.trees = trees;

    bridgeMethods = new BridgeMethods(this, types);
  }

  @Override
  public List<ExecutableElement> getDeclaredMethods(TypeElement owner, CharSequence name) {
    return getMethods(owner, name, declaredMethodsMaps, Element::getEnclosedElements);
  }

  @Override
  public List<ExecutableElement> getAllMethods(TypeElement owner, CharSequence name) {
    return getMethods(owner, name, allMethodsMaps, this::getAllMembers);
  }

  @Override
  public List<BridgeMethod> getBridgeMethods(TypeElement owner, CharSequence name) {
    return bridgeMethods.getBridgeMethods(owner, getName(name));
  }

  @Override
  @Nullable
  public ExecutableElement getImplementation(ExecutableElement baseMethod, TypeElement inType) {
    ExecutableElement result = null;
    ExecutableElement undecided = null;
    for (ExecutableElement candidate : getAllMethods(inType, baseMethod.getSimpleName())) {
      Element enclosingElement = candidate.getEnclosingElement();
      if (enclosingElement != inType && enclosingElement.getKind().isInterface()) {
        continue;
      }
      if (candidate == baseMethod) {
        if (result == null) {
          result = candidate;
        }
        continue;
      }
      if (!overrides(candidate, baseMethod, inType)
          || !returnTypeSubstitutable(candidate, baseMethod, inType)) {
        continue;
      }
      Boolean parametersMatch = parameterTypesMatch(candidate, baseMethod, inType);
      if (Boolean.TRUE.equals(parametersMatch)
          || standsInForDroppedOverload(candidate, baseMethod, inType)) {
        result = candidate;
      } else if (parametersMatch == null) {
        undecided = candidate;
      }
    }
    if (result == null && undecided == null) {
      // javac's member closure drops a method of a class-file superclass that a source method seems
      // to override only because a parameter names a missing class; javac's own lookup still finds
      // it walking up the superclasses.
      ExecutableElement inherited = findHiddenImplementation(baseMethod, inType);
      if (inherited != null) {
        return inherited;
      }
    }
    // A candidate whose parameters name a missing class of unknown identity overrides only when no
    // candidate is known to; a decided match is never replaced by an undecided one.
    if (undecided != null && (result == null || result == baseMethod)) {
      result = undecided;
    }
    if (result != null
        && result != baseMethod
        && hasOrderDependentImplementation(result)
        && !returnErasureDiffers(result, baseMethod)) {
      // javac's pick depends on a member order that the module's Kotlin stubs do not preserve;
      // take the method that needs a bridge, so the bridge is kept.
      for (ExecutableElement other :
          getDeclaredMethods((TypeElement) result.getEnclosingElement(), result.getSimpleName())) {
        if (other != result
            && isReturnTypeOverload(other, result)
            && returnTypeSubstitutable(other, baseMethod, inType)) {
          return other;
        }
      }
    }
    return result;
  }

  private boolean returnErasureDiffers(ExecutableElement candidate, ExecutableElement baseMethod) {
    return !ErasedTypes.isSameType(
        types,
        types.erasure(((ExecutableType) candidate.asType()).getReturnType()),
        types.erasure(baseMethod.getReturnType()));
  }

  /**
   * javac treats an error type (a class that is not on the classpath) as the same as any type, so a
   * method whose parameter names a missing class appears to override every method of that name and
   * arity. A parameter that names a missing class matches only a parameter that names the same
   * missing class. When either side's qualified name is unknown, the candidate matches, so the
   * bridge is kept.
   */
  @Nullable
  private ExecutableElement findHiddenImplementation(
      ExecutableElement baseMethod, TypeElement inType) {
    List<ExecutableElement> visible = getAllMethods(inType, baseMethod.getSimpleName());
    for (TypeElement type = MoreElements.getSuperclass(inType);
        type != null;
        type = MoreElements.getSuperclass(type)) {
      if (trees.getTree(type) != null) {
        continue;
      }
      for (ExecutableElement candidate : getDeclaredMethods(type, baseMethod.getSimpleName())) {
        if (candidate == baseMethod
            || visible.contains(candidate)
            || candidate.getModifiers().contains(Modifier.ABSTRACT)
            || !overrides(candidate, baseMethod, inType)
            || !returnTypeSubstitutable(candidate, baseMethod, inType)
            || !Boolean.TRUE.equals(parameterTypesMatch(candidate, baseMethod, inType))) {
          continue;
        }
        return candidate;
      }
    }
    return null;
  }

  /** TRUE or FALSE when decided, null when a missing class of unknown identity leaves it open. */
  @Nullable
  private Boolean parameterTypesMatch(
      ExecutableElement candidate, ExecutableElement baseMethod, TypeElement inType) {
    List<? extends TypeMirror> baseParameters = baseParameterTypes(baseMethod, inType);
    if (baseParameters == null) {
      return true;
    }
    List<? extends TypeMirror> candidateParameters = baseParameterTypes(candidate, inType);
    if (candidateParameters == null) {
      candidateParameters = ((ExecutableType) candidate.asType()).getParameterTypes();
    }
    if (candidateParameters.size() != baseParameters.size()) {
      return true;
    }
    Boolean result = true;
    for (int i = 0; i < candidateParameters.size(); i++) {
      Boolean same =
          sameErasure(
              candidateParameters.get(i),
              baseParameterContext(candidate, i, inType),
              baseParameters.get(i),
              baseParameterContext(baseMethod, i, inType),
              true);
      if (Boolean.FALSE.equals(same)) {
        return false;
      }
      if (same == null) {
        result = null;
      }
    }
    return result;
  }

  /**
   * javac does not enter a method whose parameter list differs from an earlier one of its class
   * only in a missing class. When the method it dropped is the one that overrides {@code
   * baseMethod}, the earlier method stands in for it, as javac's own lookup has it. The dropped
   * method is read from its attributed tree; when its types cannot be compared, the earlier method
   * stands in, so the bridge is kept.
   */
  private boolean standsInForDroppedOverload(
      ExecutableElement candidate, ExecutableElement baseMethod, TypeElement inType) {
    Element owner = candidate.getEnclosingElement();
    if (!(owner instanceof TypeElement)) {
      return false;
    }
    TreePath classPath = trees.getPath(owner);
    if (classPath == null || !(classPath.getLeaf() instanceof ClassTree)) {
      return false;
    }
    List<? extends TypeMirror> baseParameters = baseParameterTypes(baseMethod, inType);
    if (baseParameters == null) {
      return false;
    }
    for (ExecutableElement entered :
        getDeclaredMethods((TypeElement) owner, candidate.getSimpleName())) {
      if (Boolean.TRUE.equals(
          sameParameters(
              ((ExecutableType) entered.asType()).getParameterTypes(),
              entered,
              baseParameters,
              baseMethod,
              inType))) {
        return false;
      }
    }
    for (Tree member : ((ClassTree) classPath.getLeaf()).getMembers()) {
      if (!(member instanceof MethodTree)) {
        continue;
      }
      MethodTree method = (MethodTree) member;
      if (!method.getName().contentEquals(candidate.getSimpleName())
          || method.getParameters().size() != baseParameters.size()) {
        continue;
      }
      TreePath methodPath = new TreePath(classPath, method);
      List<TypeMirror> declaredParameters = new ArrayList<>();
      boolean unknown = false;
      for (VariableTree parameter : method.getParameters()) {
        TypeMirror type =
            trees.getTypeMirror(
                new TreePath(new TreePath(methodPath, parameter), parameter.getType()));
        if (type == null) {
          unknown = true;
          break;
        }
        declaredParameters.add(type);
      }
      if (unknown) {
        return true;
      }
      Boolean same =
          sameParameters(declaredParameters, candidate, baseParameters, baseMethod, inType);
      if (same == null || same) {
        return true;
      }
    }
    return false;
  }

  @Nullable
  private List<? extends TypeMirror> baseParameterTypes(
      ExecutableElement baseMethod, TypeElement inType) {
    try {
      return ((ExecutableType) types.asMemberOf((DeclaredType) inType.asType(), baseMethod))
          .getParameterTypes();
    } catch (IllegalArgumentException e) {
      return null;
    }
  }

  /**
   * TRUE or FALSE when the parameter lists are decided, null when a missing class leaves it open.
   */
  @Nullable
  private Boolean sameParameters(
      List<? extends TypeMirror> parameters,
      Element context,
      List<? extends TypeMirror> baseParameters,
      ExecutableElement baseMethod,
      TypeElement inType) {
    if (parameters.size() != baseParameters.size()) {
      return false;
    }
    Boolean result = true;
    for (int i = 0; i < parameters.size(); i++) {
      Boolean same =
          sameErasure(
              parameters.get(i),
              context,
              baseParameters.get(i),
              baseParameterContext(baseMethod, i, inType),
              false);
      if (same == null) {
        result = null;
      } else if (!same) {
        return false;
      }
    }
    return result;
  }

  /**
   * Where a parameter of {@code method}, viewed as a member of {@code inType}, was written. A
   * parameter declared as a type variable takes its type from the type argument that a source class
   * between {@code inType} and the method's class writes in its {@code extends} or {@code
   * implements} clause: the source ancestor nearest to the method's class in which the parameter is
   * no longer a type variable. Null when no single such class exists.
   */
  @Nullable
  private Element baseParameterContext(ExecutableElement method, int index, TypeElement inType) {
    List<? extends VariableElement> parameters = method.getParameters();
    if (index >= parameters.size()
        || componentType(parameters.get(index).asType()).getKind() != TypeKind.TYPEVAR) {
      return method;
    }
    List<? extends TypeMirror> inTypeView = baseParameterTypes(method, inType);
    if (inTypeView != null && index < inTypeView.size()) {
      TypeMirror viewed = componentType(inTypeView.get(index));
      if (viewed.getKind() == TypeKind.TYPEVAR) {
        // Still a type variable in the class being compiled: its erasure is its first bound,
        // written where the variable is declared.
        return ((TypeVariable) viewed).asElement().getEnclosingElement();
      }
    }
    TypeElement owner = (TypeElement) method.getEnclosingElement();
    TypeMirror ownerErasure = types.erasure(owner.asType());
    List<TypeElement> writers = new ArrayList<>();
    Deque<TypeElement> queue = new ArrayDeque<>();
    Set<TypeElement> visited = new HashSet<>();
    queue.add(inType);
    while (!queue.isEmpty()) {
      TypeElement type = queue.remove();
      if (!visited.add(type)
          || type == owner
          || !types.isSubtype(types.erasure(type.asType()), ownerErasure)) {
        continue;
      }
      if (trees.getTree(type) != null) {
        List<? extends TypeMirror> view = baseParameterTypes(method, type);
        if (view != null
            && index < view.size()
            && componentType(view.get(index)).getKind() != TypeKind.TYPEVAR) {
          writers.add(type);
        }
      }
      for (TypeMirror supertype : types.directSupertypes(type.asType())) {
        Element element = types.asElement(supertype);
        if (element instanceof TypeElement) {
          queue.add((TypeElement) element);
        }
      }
    }
    TypeElement nearest = null;
    for (TypeElement writer : writers) {
      boolean hasWriterAbove = false;
      for (TypeElement other : writers) {
        if (other != writer
            && types.isSubtype(types.erasure(writer.asType()), types.erasure(other.asType()))) {
          hasWriterAbove = true;
          break;
        }
      }
      if (!hasWriterAbove) {
        if (nearest != null) {
          return null;
        }
        nearest = writer;
      }
    }
    return nearest;
  }

  /**
   * Whether two types have the same erasure. Two missing classes are the same when their qualified
   * names are; null when either name is unknown. With {@code requireMissingOnBothSides}, a missing
   * class never equals a class that resolves.
   */
  @Nullable
  private Boolean sameErasure(
      TypeMirror type,
      @Nullable Element context,
      TypeMirror other,
      @Nullable Element otherContext,
      boolean requireMissingOnBothSides) {
    TypeMirror erased = types.erasure(type);
    TypeMirror otherErased = types.erasure(other);
    int dimensions = arrayDimensions(erased);
    if (dimensions != arrayDimensions(otherErased)) {
      if (isError(erased) || isError(otherErased)) {
        return requireMissingOnBothSides ? Boolean.FALSE : null;
      }
      return Boolean.FALSE;
    }
    TypeMirror component = componentType(erased);
    TypeMirror otherComponent = componentType(otherErased);
    boolean missing = component.getKind() == TypeKind.ERROR;
    boolean otherMissing = otherComponent.getKind() == TypeKind.ERROR;
    if (!missing && !otherMissing) {
      return types.isSameType(erased, otherErased);
    }
    if (missing != otherMissing) {
      return false;
    }
    if (types.asElement(component) != null
        && types.asElement(component) == types.asElement(otherComponent)) {
      return true;
    }
    String name = qualifiedErrorName(component, context);
    String otherName = qualifiedErrorName(otherComponent, otherContext);
    if (name == null || otherName == null) {
      // Classes with different simple names differ, whatever their packages.
      return simpleName(component).equals(simpleName(otherComponent)) ? null : Boolean.FALSE;
    }
    return name.equals(otherName);
  }

  /**
   * The qualified name of a missing class. A class file names it by its binary name. A source names
   * it as written, so the name is resolved against the compilation unit: a single-type import of
   * its first segment, else its own package when the unit has no on-demand imports. Null when it
   * cannot be decided.
   */
  @Nullable
  private String qualifiedErrorName(TypeMirror type, @Nullable Element context) {
    if (context == null) {
      return null;
    }
    String written = type.toString();
    int arguments = written.indexOf('<');
    if (arguments >= 0) {
      written = written.substring(0, arguments);
    }
    TreePath path = trees.getPath(context);
    if (path == null) {
      // A class file names a missing class by its binary name.
      return written.indexOf('.') >= 0 ? written.replace('$', '.') : null;
    }
    CompilationUnitTree unit = path.getCompilationUnit();
    int dot = written.indexOf('.');
    String first = dot < 0 ? written : written.substring(0, dot);
    String rest = dot < 0 ? "" : written.substring(dot);
    boolean onDemand = false;
    for (ImportTree importTree : unit.getImports()) {
      if (importTree.isStatic()) {
        continue;
      }
      String imported = importTree.getQualifiedIdentifier().toString();
      if (imported.endsWith(".*")) {
        onDemand = true;
      } else if (imported.endsWith("." + first)) {
        return imported + rest;
      }
    }
    if (dot >= 0 && Character.isLowerCase(first.charAt(0))) {
      return written;
    }
    if (onDemand) {
      return null;
    }
    ExpressionTree packageName = unit.getPackageName();
    return packageName == null ? written : packageName + "." + written;
  }

  private static String simpleName(TypeMirror type) {
    String name = type.toString();
    int arguments = name.indexOf('<');
    if (arguments >= 0) {
      name = name.substring(0, arguments);
    }
    return name.substring(Math.max(name.lastIndexOf('.'), name.lastIndexOf('$')) + 1);
  }

  private static int arrayDimensions(TypeMirror type) {
    int dimensions = 0;
    while (type.getKind() == TypeKind.ARRAY) {
      dimensions++;
      type = ((ArrayType) type).getComponentType();
    }
    return dimensions;
  }

  private static TypeMirror componentType(TypeMirror type) {
    while (type.getKind() == TypeKind.ARRAY) {
      type = ((ArrayType) type).getComponentType();
    }
    return type;
  }

  /**
   * javac's {@code TransTypes} resolves the implementation of a method with its return type checked
   * ({@code MethodSymbol.implementation(origin, types, true)}), while {@link Elements#overrides}
   * ignores it: a {@code void} setter and a builder-style method that returns the class can share a
   * name and parameter list in a Kotlin class. Only the cases the reduced classpath decides are
   * rejected here: {@code void} against a value, and different primitives.
   */
  private boolean returnTypeSubstitutable(
      ExecutableElement candidate, ExecutableElement baseMethod, TypeElement inType) {
    if (candidate == baseMethod) {
      return true;
    }
    TypeMirror candidateReturn = ((ExecutableType) candidate.asType()).getReturnType();
    TypeMirror baseReturn = baseMethod.getReturnType();
    boolean candidateVoid = candidateReturn.getKind() == TypeKind.VOID;
    boolean baseVoid = baseReturn.getKind() == TypeKind.VOID;
    if (candidateVoid || baseVoid) {
      return candidateVoid == baseVoid;
    }
    if (candidateReturn.getKind().isPrimitive() || baseReturn.getKind().isPrimitive()) {
      return candidateReturn.getKind() == baseReturn.getKind();
    }
    return true;
  }

  private static boolean isError(TypeMirror type) {
    return componentType(type).getKind() == TypeKind.ERROR;
  }

  @Nullable
  @Override
  public TypeElement getBinaryImplementationOwner(ExecutableElement method, TypeElement inType) {
    TypeElement implementationOwner = null;
    ExecutableElement implementation = getImplementation(method, inType);
    if (implementation != null && hasOrderDependentImplementation(implementation)) {
      return null;
    }
    if (implementation != null && binarySignaturesMatch(implementation, method)) {
      implementationOwner = (TypeElement) implementation.getEnclosingElement();
    }

    for (TypeElement type = inType; type != null; type = MoreElements.getSuperclass(type)) {
      if (implementationOwner != null
          && !types.isSubtype(
              types.erasure(type.asType()), types.erasure(implementationOwner.asType()))) {
        break;
      }

      if (getBinaryBridges(type, method.getSimpleName()).stream()
          .anyMatch(it -> binarySignaturesMatch(it, method))) {
        implementationOwner = type;
        break;
      }
    }

    return implementationOwner;
  }

  /**
   * A Kotlin class can declare two methods with one name and parameter list that differ only in
   * their return type, such as a property getter next to an override. javac then resolves the
   * implementation by member order in the class file. The module's own Kotlin classes are read here
   * as stubs whose member order need not be kotlinc's, so the bridge is kept.
   */
  private boolean hasOrderDependentImplementation(ExecutableElement implementation) {
    TypeElement owner = (TypeElement) implementation.getEnclosingElement();
    if (localClassDirectories.isEmpty()
        || !ClassFileTypes.isReadFromClassFile(owner)
        || classFileBridges(owner).isPresent()) {
      return false;
    }
    for (ExecutableElement other : getDeclaredMethods(owner, implementation.getSimpleName())) {
      if (other != implementation && isReturnTypeOverload(other, implementation)) {
        return true;
      }
    }
    return false;
  }

  /** Whether two methods of one class have the same parameter erasures and different returns. */
  private boolean isReturnTypeOverload(ExecutableElement method, ExecutableElement other) {
    if (method.getModifiers().contains(Modifier.STATIC)
        || other.getModifiers().contains(Modifier.STATIC)) {
      return false;
    }
    ExecutableType methodType = (ExecutableType) method.asType();
    ExecutableType otherType = (ExecutableType) other.asType();
    List<? extends TypeMirror> parameters = methodType.getParameterTypes();
    List<? extends TypeMirror> otherParameters = otherType.getParameterTypes();
    if (parameters.size() != otherParameters.size()) {
      return false;
    }
    for (int i = 0; i < parameters.size(); i++) {
      if (!ErasedTypes.isSameType(
          types, types.erasure(parameters.get(i)), types.erasure(otherParameters.get(i)))) {
        return false;
      }
    }
    return !ErasedTypes.isSameType(
        types, types.erasure(methodType.getReturnType()), types.erasure(otherType.getReturnType()));
  }

  /**
   * The bridges a class declares, as the VM sees them: those in its class file when it is read from
   * one, otherwise the bridges javac adds when it compiles the class.
   */
  private List<ExecutableElement> getBinaryBridges(TypeElement type, Name name) {
    Optional<Map<String, List<ExecutableElement>>> declared = classFileBridges(type);
    if (declared.isPresent()) {
      return declared.get().getOrDefault(name.toString(), Collections.emptyList());
    }
    List<ExecutableElement> result = new ArrayList<>();
    for (BridgeMethod bridgeMethod : bridgeMethods.getBridgeMethodsNoCreate(type, name)) {
      result.add(bridgeMethod.to);
    }
    return result;
  }

  private Optional<Map<String, List<ExecutableElement>>> classFileBridges(TypeElement type) {
    return classFileBridges.computeIfAbsent(
        type,
        it -> Optional.ofNullable(ClassFileBridges.getDeclaredBridges(it, localClassDirectories)));
  }

  private boolean binarySignaturesMatch(ExecutableElement e1, ExecutableElement e2) {
    return ErasedTypes.isSameDescriptor(
        types, (ExecutableType) e1.asType(), (ExecutableType) e2.asType());
  }

  @Override
  public void setLocalClassDirectories(List<Path> directories) {
    localClassDirectories = List.copyOf(directories);
    classFileBridges.clear();
  }

  @Override
  public List<BridgeMethod> getAllBridgeMethods(TypeElement type) {
    return bridgeMethods.getBridgeMethods(type);
  }

  private List<ExecutableElement> getMethods(
      TypeElement owner,
      CharSequence name,
      Map<TypeElement, Map<Name, List<ExecutableElement>>> methodsMaps,
      Function<? super TypeElement, List<? extends Element>> getMembersFn) {
    Map<Name, List<ExecutableElement>> methodsMap =
        methodsMaps.computeIfAbsent(owner, el -> buildMethodsMap(el, getMembersFn));

    List<ExecutableElement> result = methodsMap.get(getName(name));
    if (result == null) {
      result = Collections.emptyList();
    }
    return result;
  }

  @Override
  public boolean isCompiledInCurrentRun(Element element) {
    return trees.getTree(element) != null;
  }

  @Override
  public List<? extends AnnotationMirror> getAllTypeAnnotations(Element element) {
    return MoreAnnotations.getAllTypeAnnotations(element);
  }

  private static Map<Name, List<ExecutableElement>> buildMethodsMap(
      TypeElement owner, Function<? super TypeElement, List<? extends Element>> getMembersFn) {
    Map<Name, List<ExecutableElement>> result = new HashMap<>();

    for (ExecutableElement method : ElementFilter.methodsIn(getMembersFn.apply(owner))) {
      List<ExecutableElement> methodsWithName =
          result.computeIfAbsent(method.getSimpleName(), ignored -> new ArrayList<>());
      methodsWithName.add(method);
    }

    return result;
  }
}
