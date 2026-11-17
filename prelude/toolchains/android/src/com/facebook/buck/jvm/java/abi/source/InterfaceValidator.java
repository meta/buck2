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

import com.facebook.buck.jvm.java.abi.source.CompletionSimulator.CompletedType;
import com.facebook.buck.jvm.java.abi.source.TreeBackedTypeResolutionSimulator.TreeBackedResolvedType;
import com.facebook.buck.jvm.java.abi.source.api.SourceOnlyAbiRuleInfoFactory.SourceOnlyAbiRuleInfo;
import com.facebook.buck.jvm.java.plugin.adapter.BuckJavacTask;
import com.facebook.infer.annotation.Nullsafe;
import com.sun.source.tree.AnnotationTree;
import com.sun.source.tree.ClassTree;
import com.sun.source.tree.CompilationUnitTree;
import com.sun.source.tree.ImportTree;
import com.sun.source.tree.MemberSelectTree;
import com.sun.source.tree.MethodTree;
import com.sun.source.tree.Scope;
import com.sun.source.tree.Tree;
import com.sun.source.util.TreePath;
import com.sun.source.util.Trees;
import java.lang.annotation.Retention;
import java.lang.annotation.RetentionPolicy;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Deque;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.stream.Collectors;
import javax.lang.model.element.Element;
import javax.lang.model.element.ElementKind;
import javax.lang.model.element.Modifier;
import javax.lang.model.element.Name;
import javax.lang.model.element.NestingKind;
import javax.lang.model.element.PackageElement;
import javax.lang.model.element.QualifiedNameable;
import javax.lang.model.element.TypeElement;
import javax.lang.model.element.VariableElement;
import javax.lang.model.type.DeclaredType;
import javax.lang.model.type.TypeKind;
import javax.lang.model.type.TypeMirror;
import javax.lang.model.util.ElementFilter;
import javax.lang.model.util.Elements;
import javax.lang.model.util.Types;
import javax.tools.Diagnostic;
import org.jspecify.annotations.Nullable;

/**
 * Validates the type and compile-time-constant references in the non-private interface of classes
 * being compiled in the current compilation run, ensuring that they are compatible with correctly
 * generating ABIs from source. When generating ABIs from source, we may be missing some (or most)
 * dependencies.
 *
 * <p>In order to be compatible, type references to missing classes in the non-private interface of
 * a class being compiled must be one of the following:
 *
 * <ul>
 *   <li>The fully-qualified name of the type
 *   <li>The simple name of a type that appears in an import statement
 *   <li>The simple name of a top-level type in the same package as the type being compiled
 * </ul>
 *
 * <p>In order to be compatible, compile-time constants in missing classes may not be referenced
 * from the non-private interface of a class being compiled.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
class InterfaceValidator {
  private final Elements elements;
  private final Diagnostic.Kind messageKind;
  private final Types types;
  private final Trees trees;
  private final SourceOnlyAbiRuleInfo ruleInfo;
  private final FileManagerSimulator fileManager;
  private final CompilerTypeResolutionSimulator compilerResolver;
  private final boolean inheritedMemberTypesOnly;

  public InterfaceValidator(
      Diagnostic.Kind messageKind, BuckJavacTask task, SourceOnlyAbiRuleInfo ruleInfo) {
    this(messageKind, task, ruleInfo, false);
  }

  public InterfaceValidator(
      Diagnostic.Kind messageKind,
      BuckJavacTask task,
      SourceOnlyAbiRuleInfo ruleInfo,
      boolean inheritedMemberTypesOnly) {
    this.messageKind = messageKind;
    this.inheritedMemberTypesOnly = inheritedMemberTypesOnly;
    trees = task.getTrees();
    types = task.getTypes();
    elements = task.getElements();
    this.ruleInfo = ruleInfo;
    fileManager = new FileManagerSimulator(elements, trees, ruleInfo);
    compilerResolver = new CompilerTypeResolutionSimulator(trees, fileManager);
  }

  public void validate(List<? extends CompilationUnitTree> compilationUnits) {
    InterfaceScanner interfaceScanner = new InterfaceScanner(trees, inheritedMemberTypesOnly);
    for (CompilationUnitTree compilationUnit : compilationUnits) {
      interfaceScanner.findReferences(
          compilationUnit,
          new ValidatingListener(
              messageKind,
              elements,
              types,
              trees,
              ruleInfo,
              fileManager,
              compilerResolver,
              compilationUnit,
              inheritedMemberTypesOnly));
    }
  }

  private static class ValidatingListener implements InterfaceScanner.Listener {
    private final Diagnostic.Kind messageKind;
    private final Elements elements;
    private final Trees trees;
    private final SourceOnlyAbiRuleInfo ruleInfo;
    private final TreeBackedTypeResolutionSimulator treeBackedResolver;
    private final FileManagerSimulator fileManager;
    private final CompletionSimulator completer;
    private final CompilerTypeResolutionSimulator compilerResolver;
    private final ImportsTracker imports;
    private final boolean inheritedMemberTypesOnly;
    private final Map<TypeElement, Set<TypeElement>> exposedSupertypes = new HashMap<>();

    public ValidatingListener(
        Diagnostic.Kind messageKind,
        Elements elements,
        Types types,
        Trees trees,
        SourceOnlyAbiRuleInfo ruleInfo,
        FileManagerSimulator fileManager,
        CompilerTypeResolutionSimulator compilerResolver,
        CompilationUnitTree compilationUnit,
        boolean inheritedMemberTypesOnly) {
      this.messageKind = messageKind;
      this.inheritedMemberTypesOnly = inheritedMemberTypesOnly;
      this.trees = trees;
      this.elements = elements;
      this.ruleInfo = ruleInfo;
      this.fileManager = fileManager;
      this.compilerResolver = compilerResolver;
      completer = new CompletionSimulator(fileManager);
      imports =
          new ImportsTracker(
              elements,
              types,
              (PackageElement)
                  Objects.requireNonNull(trees.getElement(new TreePath(compilationUnit))));
      treeBackedResolver = new TreeBackedTypeResolutionSimulator(elements, trees, compilationUnit);
    }

    @Override
    public void onTypeDeclared(TypeElement type, TreePath path) {
      if (inheritedMemberTypesOnly) {
        return;
      }
      if (type.getKind() == ElementKind.ANNOTATION_TYPE
          && !ruleInfo.ruleIsRequiredForSourceOnlyAbi()) {
        trees.printMessage(
            messageKind,
            String.format(
                "Annotation definitions must be in rules with required_for_source_only_abi ="
                    + " True.\n"
                    + "For a quick fix, add required_for_source_only_abi = True to %s.\n"
                    + "A better fix is to move %s to a new rule that contains only\n"
                    + "annotations, and mark that rule required_for_source_only_abi.\n",
                ruleInfo.getRuleName(), type.getSimpleName()),
            path.getLeaf(),
            path.getCompilationUnit());
      }

      ClassTree classTree = (ClassTree) path.getLeaf();

      // The compiler can handle superclasses and interfaces that are outright missing
      // (because it is possible for those to be generated by annotation processors).
      // However, if a superclass or interface is present, the compiler expects
      // the entire class hierarchy of that class/interface to also be present, and
      // gives a sketchy error if they are missing.
      Tree extendsClause = classTree.getExtendsClause();
      if (extendsClause != null) {
        ensureAbsentOrComplete(type.getSuperclass(), new TreePath(path, extendsClause));
      }

      List<? extends Tree> implementsClause = classTree.getImplementsClause();
      if (implementsClause != null) {
        List<? extends TypeMirror> interfaces = type.getInterfaces();
        for (int i = 0; i < implementsClause.size(); i++) {
          ensureAbsentOrComplete(interfaces.get(i), new TreePath(path, implementsClause.get(i)));
        }
      }
    }

    private void ensureAbsentOrComplete(TypeMirror typeMirror, TreePath path) {
      if (typeMirror.getKind() != TypeKind.DECLARED) {
        return;
      }

      DeclaredType declaredType = (DeclaredType) typeMirror;
      TypeElement typeElement = (TypeElement) declaredType.asElement();
      CompletedType completedType = Objects.requireNonNull(completer.complete(typeElement, true));

      switch (completedType.kind) {
        case COMPLETED_TYPE:
        case PARTIALLY_COMPLETED_TYPE:
        case ERROR_TYPE:
          // These are all fine
          break;
        case CRASH:
          reportMissingDeps(completedType, path);
          break;
      }
    }

    public void reportMissingDeps(CompletedType type, TreePath path) {
      // TODO: Different message based on kind.
      trees.printMessage(
          messageKind,
          String.format(
              "Source-only ABI generation requires that this type be unavailable, or that all of"
                  + " its superclasses/interfaces be available.\n"
                  + "To fix, add the following rules to source_only_abi_deps in %s: %s",
              ruleInfo.getRuleName(),
              type.getMissingDependencies().stream().sorted().collect(Collectors.joining(", "))),
          path.getLeaf(),
          path.getCompilationUnit());
    }

    public void reportMissingDeps(ResolvedType type, TreePath path) {
      trees.printMessage(
          messageKind,
          String.format(
              "Source-only ABI generation requires that this type be unavailable, or that all of"
                  + " its superclasses/interfaces be available.\n"
                  + "To fix, add the following rules to source_only_abi_deps in %s: %s",
              ruleInfo.getRuleName(),
              type.missingDependencies.stream().sorted().collect(Collectors.joining(", "))),
          path.getLeaf(),
          path.getCompilationUnit());
    }

    @Override
    public void onImport(
        boolean isStatic,
        boolean isStarImport,
        TreePath leafmostElementPath,
        QualifiedNameable leafmostElement,
        @Nullable Name memberName) {
      if (!inheritedMemberTypesOnly && leafmostElement.getKind() != ElementKind.PACKAGE) {
        if (isStatic) {
          CompletedType completedType = completer.complete(leafmostElement, true);
          if (completedType != null
              && (completedType.kind == CompletedTypeKind.CRASH
                  || completedType.kind == CompletedTypeKind.PARTIALLY_COMPLETED_TYPE)) {
            reportMissingDeps(completedType, leafmostElementPath);
          }
        } else {
          ResolvedType compilerResolvedType = compilerResolver.resolve(leafmostElementPath);
          if (compilerResolvedType != null) {
            switch (compilerResolvedType.kind) {
              case CRASH:
                reportMissingDeps(compilerResolvedType, leafmostElementPath);
                break;
              case RESOLVED_TYPE:
                // Nothing to do; it would resolve fine
                break;
              // $CASES-OMITTED$
              default:
                {
                  TreeBackedResolvedType treeBackedResolvedType =
                      treeBackedResolver.resolve(leafmostElementPath);
                  if (!treeBackedResolvedType.isCorrect()) {
                    if (treeBackedResolvedType.isCorrectable()) {
                      treeBackedResolvedType.reportErrors(messageKind);
                    } else {
                      reportMissingDeps(compilerResolvedType, leafmostElementPath);
                    }
                  }
                }
                break;
            }
          }
        }
      }

      if (!isStatic) {
        if (!isStarImport) {
          imports.importType((TypeElement) leafmostElement, leafmostElementPath);
        } else {
          imports.importMembers(leafmostElement, leafmostElementPath);
        }
      } else if (!isStarImport) {
        imports.importStatic((TypeElement) leafmostElement, Objects.requireNonNull(memberName));
      } else {
        imports.importStaticMembers((TypeElement) leafmostElement);
      }
    }

    private boolean isInDiscardedSourceAnnotation(TreePath path) {
      for (TreePath current = path;
          current.getParentPath() != null;
          current = current.getParentPath()) {
        if (!(current.getLeaf() instanceof AnnotationTree)) {
          continue;
        }
        AnnotationTree annotation = (AnnotationTree) current.getLeaf();
        // Annotation values and defaults are emitted regardless of their own retention.
        switch (current.getParentPath().getLeaf().getKind()) {
          case MODIFIERS:
          case ANNOTATED_TYPE:
          case TYPE_PARAMETER:
          case COMPILATION_UNIT:
          case PACKAGE:
            Element element =
                trees.getElement(new TreePath(current, annotation.getAnnotationType()));
            if (element == null || element.getKind() != ElementKind.ANNOTATION_TYPE) {
              return false;
            }
            Retention retention = element.getAnnotation(Retention.class);
            return retention != null && retention.value() == RetentionPolicy.SOURCE;
          default:
            break;
        }
      }
      return false;
    }

    @Override
    public void onTypeReferenceFound(
        TypeElement canonicalTypeElement, TreePath path, Element referencingElement) {
      if (inheritedMemberTypesOnly) {
        Tree.Kind kind = path.getLeaf().getKind();
        if ((kind != Tree.Kind.IDENTIFIER && kind != Tree.Kind.MEMBER_SELECT)
            || isInDiscardedSourceAnnotation(path)
            || isOutsideAbi(referencingElement)
            || !isReachableFromOutside(referencingElement)) {
          return;
        }
      }
      compilerResolver.setImports(imports);
      treeBackedResolver.setImports(imports);
      ResolvedType compilerResolvedType = compilerResolver.resolve(path);

      if (inheritedMemberTypesOnly) {
        if (!compilerResolvedType.missingMemberSupertypes.isEmpty()) {
          trees.printMessage(
              messageKind,
              inheritedMemberTypeMessage(
                  canonicalTypeElement, path, compilerResolvedType.missingMemberSupertypes),
              path.getLeaf(),
              path.getCompilationUnit());
        }
        return;
      }

      if (compilerResolvedType.kind != ResolvedTypeKind.RESOLVED_TYPE) {
        TreeBackedResolvedType treeBackedResolvedType = treeBackedResolver.resolve(path);
        if (!treeBackedResolvedType.isCorrect()) {
          if (treeBackedResolvedType.isCorrectable()) {
            treeBackedResolvedType.reportErrors(messageKind);
          } else {
            reportMissingDeps(compilerResolvedType, path);
          }
        }
      }
    }

    /**
     * The error for a reference that names an inherited member type through supertypes missing from
     * the source-only ABI classpath. It lists the spellings that name the type without those
     * supertypes: through its declaring type, fully qualified, or imported. Only when none of them
     * can name it here does it ask for the providers of the missing supertypes. Tooling parses the
     * first sentence and the last line.
     */
    private String inheritedMemberTypeMessage(
        TypeElement memberType, TreePath path, Set<String> missingSupertypes) {
      String canonicalName = memberType.getQualifiedName().toString();
      StringBuilder message =
          new StringBuilder(
              String.format(
                  "Inherited member type %s requires missing supertypes during source-only ABI"
                      + " generation.\n",
                  canonicalName));
      List<TypeElement> chain = enclosingChain(memberType);
      String unnameable = whyNoSpellingNames(chain, path);
      if (unnameable == null) {
        String declaringSpelling =
            chain.stream()
                .map(type -> type.getSimpleName().toString())
                .collect(Collectors.joining("."));
        TypeElement top = chain.get(0);
        CompilationUnitTree unit = path.getCompilationUnit();
        List<String> spellings = new ArrayList<>();
        if (!simpleNameIsTaken(top, unit) && !isShadowed(top, path)) {
          spellings.add(
              simpleNameIsVisible(top, unit)
                  ? declaringSpelling
                  : String.format(
                      "%s (with import %s;)", declaringSpelling, top.getQualifiedName()));
        }
        spellings.add(canonicalName);
        message.append("Refer to it without those supertypes: write ");
        message.append(String.join(" or ", spellings));
        if (isImportable(chain, unit) && !isShadowed(memberType, path)) {
          message.append(
              String.format(
                  ", or add import %s; and write %s", canonicalName, memberType.getSimpleName()));
        }
        message.append(".\n");
        message.append(
            String.format(
                "To write %s automatically, run: arc aast codemod --jast"
                    + " qualifyinheritedmembertypes %s -- -member-types %s\n",
                spellings.get(0).equals(canonicalName) ? canonicalName : declaringSpelling,
                ruleInfo.getRuleName(),
                canonicalName));
      } else {
        message.append(
            String.format(
                "No spelling names it here without those supertypes: %s. Add the providers of the"
                    + " following classes to source_only_abi_deps in %s.\n",
                unnameable, ruleInfo.getRuleName()));
      }
      message.append("KOSABI_JASABI_MISSING_SUPERTYPES: ");
      message.append(missingSupertypes.stream().sorted().collect(Collectors.joining(", ")));
      return message.toString();
    }

    /** {@code memberType} and the types enclosing it, outermost first. */
    private static List<TypeElement> enclosingChain(TypeElement memberType) {
      List<TypeElement> chain = new ArrayList<>();
      for (Element type = memberType;
          type instanceof TypeElement;
          type = type.getEnclosingElement()) {
        chain.add((TypeElement) type);
      }
      Collections.reverse(chain);
      return chain;
    }

    /**
     * Why no spelling of the member type at {@code path} avoids the missing supertypes, or null if
     * the declaring-type, fully qualified and imported spellings do.
     */
    @Nullable
    private String whyNoSpellingNames(List<TypeElement> chain, TreePath path) {
      Scope scope = Objects.requireNonNull(trees.getScope(path));
      for (TypeElement type : chain) {
        if (!trees.isAccessible(scope, type)) {
          TypeElement from = scope.getEnclosingClass();
          return String.format(
              "%s is not accessible from %s",
              type.getQualifiedName(), from == null ? "this file" : from.getQualifiedName());
        }
      }
      TypeMirror referenced = trees.getTypeMirror(typePath(path));
      if (referenced instanceof DeclaredType) {
        for (TypeMirror enclosing = ((DeclaredType) referenced).getEnclosingType();
            enclosing instanceof DeclaredType;
            enclosing = ((DeclaredType) enclosing).getEnclosingType()) {
          if (!((DeclaredType) enclosing).getTypeArguments().isEmpty()) {
            return String.format(
                "it is an inner class of %s, and a qualified name would drop the type arguments",
                enclosing);
          }
        }
      }
      return null;
    }

    /** The path of the whole type at {@code path}, including its type arguments. */
    private static TreePath typePath(TreePath path) {
      TreePath parent = path.getParentPath();
      return parent != null && parent.getLeaf().getKind() == Tree.Kind.PARAMETERIZED_TYPE
          ? parent
          : path;
    }

    /**
     * Whether a single-type import of the member type compiles in {@code unit}: every type in
     * {@code chain} is accessible at the top level of the file, and no other type there has the
     * member type's simple name.
     */
    private boolean isImportable(List<TypeElement> chain, CompilationUnitTree unit) {
      Scope scope = Objects.requireNonNull(trees.getScope(new TreePath(unit)));
      for (TypeElement type : chain) {
        if (!trees.isAccessible(scope, type)) {
          return false;
        }
      }
      return !simpleNameIsTaken(chain.get(chain.size() - 1), unit);
    }

    /**
     * Whether a type parameter or a member type of a class enclosing {@code path} gives {@code
     * type}'s simple name to another type. That name then wins over an import, and over a top-level
     * type, when source-only ABI generation cannot see the inherited member type.
     */
    private boolean isShadowed(TypeElement type, TreePath path) {
      Name simpleName = type.getSimpleName();
      for (TreePath current = path; current != null; current = current.getParentPath()) {
        Tree leaf = current.getLeaf();
        if (leaf instanceof MethodTree
            && Objects.requireNonNull(((MethodTree) leaf).getTypeParameters()).stream()
                .anyMatch(
                    parameter ->
                        Objects.requireNonNull(parameter.getName()).contentEquals(simpleName))) {
          return true;
        }
        if (!(leaf instanceof ClassTree)) {
          continue;
        }
        if (Objects.requireNonNull(((ClassTree) leaf).getTypeParameters()).stream()
            .anyMatch(
                parameter ->
                    Objects.requireNonNull(parameter.getName()).contentEquals(simpleName))) {
          return true;
        }
        Element enclosing = trees.getElement(current);
        if (enclosing instanceof TypeElement) {
          for (TypeElement member :
              Objects.requireNonNull(
                  ElementFilter.typesIn(elements.getAllMembers((TypeElement) enclosing)))) {
            if (member.getSimpleName().contentEquals(simpleName) && !member.equals(type)) {
              return true;
            }
          }
        }
      }
      return false;
    }

    /**
     * Whether a single import or a top-level type of {@code unit} gives {@code type}'s simple name
     * to another type.
     */
    private static boolean simpleNameIsTaken(TypeElement type, CompilationUnitTree unit) {
      Name simpleName = type.getSimpleName();
      String qualifiedName = type.getQualifiedName().toString();
      for (ImportTree importTree : unit.getImports()) {
        Tree imported = importTree.getQualifiedIdentifier();
        if (imported instanceof MemberSelectTree
            && ((MemberSelectTree) imported).getIdentifier().contentEquals(simpleName)
            && !imported.toString().equals(qualifiedName)) {
          return true;
        }
      }
      String unitPackage = packageName(unit);
      for (Tree declaration : unit.getTypeDecls()) {
        if (declaration instanceof ClassTree) {
          Name declared = ((ClassTree) declaration).getSimpleName();
          String declaredName =
              unitPackage.isEmpty() ? declared.toString() : unitPackage + "." + declared;
          if (declared.contentEquals(simpleName) && !declaredName.equals(qualifiedName)) {
            return true;
          }
        }
      }
      return false;
    }

    /**
     * Whether the simple name of the top-level type {@code top} names it in {@code unit} without
     * another import: it is in the same package or {@code java.lang}, or imported already.
     */
    private static boolean simpleNameIsVisible(TypeElement top, CompilationUnitTree unit) {
      Element enclosing = top.getEnclosingElement();
      String topPackage =
          enclosing instanceof PackageElement
              ? ((PackageElement) enclosing).getQualifiedName().toString()
              : "";
      if (topPackage.equals(packageName(unit)) || topPackage.equals("java.lang")) {
        return true;
      }
      for (ImportTree importTree : unit.getImports()) {
        if (importTree.isStatic()) {
          continue;
        }
        String imported = importTree.getQualifiedIdentifier().toString();
        if (imported.contentEquals(top.getQualifiedName()) || imported.equals(topPackage + ".*")) {
          return true;
        }
      }
      return false;
    }

    private static String packageName(CompilationUnitTree unit) {
      return unit.getPackageName() == null ? "" : unit.getPackageName().toString();
    }

    /**
     * Whether code outside the top-level class can use the declaration of {@code element} against
     * an ABI. It can when no class enclosing {@code element} is private. Otherwise it reaches a
     * private member class's members only through a class accessible outside the top-level class
     * that inherits them, so the innermost class enclosing {@code element}, or a private class
     * enclosing that, has to be a supertype, directly or indirectly, of such a class. A private
     * class that only appears in accessible signatures does not count: code outside cannot use its
     * members against either ABI.
     */
    private boolean isReachableFromOutside(Element element) {
      List<TypeElement> enclosingTypes = new ArrayList<>();
      for (Element scope = element; scope != null; scope = scope.getEnclosingElement()) {
        if (scope instanceof TypeElement) {
          enclosingTypes.add((TypeElement) scope);
        }
      }
      if (enclosingTypes.stream().noneMatch(ValidatingListener::isPrivate)) {
        return true;
      }
      Set<TypeElement> exposed =
          exposedSupertypes.computeIfAbsent(
              enclosingTypes.get(enclosingTypes.size() - 1), ValidatingListener::exposedSupertypes);
      for (int i = 0; i < enclosingTypes.size(); i++) {
        TypeElement type = enclosingTypes.get(i);
        if ((i == 0 || isPrivate(type)) && exposed.contains(type)) {
          return true;
        }
      }
      return false;
    }

    /**
     * The supertypes, direct or indirect, of {@code topLevel} and of the classes nested in it that
     * code outside it can access.
     */
    private static Set<TypeElement> exposedSupertypes(TypeElement topLevel) {
      Set<TypeElement> supertypes = new HashSet<>();
      Deque<TypeElement> accessible = new ArrayDeque<>();
      accessible.add(topLevel);
      while (!accessible.isEmpty()) {
        TypeElement type = accessible.removeFirst();
        addSupertypes(type, supertypes);
        for (TypeElement member :
            Objects.requireNonNull(ElementFilter.typesIn(type.getEnclosedElements()))) {
          if (!isPrivate(member)) {
            accessible.add(member);
          }
        }
      }
      return supertypes;
    }

    private static boolean isPrivate(Element element) {
      return element.getModifiers().contains(Modifier.PRIVATE);
    }

    private static void addSupertypes(TypeElement type, Set<TypeElement> supertypes) {
      List<TypeMirror> direct = new ArrayList<>(type.getInterfaces());
      direct.add(type.getSuperclass());
      for (TypeMirror supertype : direct) {
        if (supertype instanceof DeclaredType) {
          Element supertypeElement = ((DeclaredType) supertype).asElement();
          if (supertypeElement instanceof TypeElement
              && supertypes.add((TypeElement) supertypeElement)) {
            addSupertypes((TypeElement) supertypeElement, supertypes);
          }
        }
      }
    }

    /**
     * Whether references from {@code element} are left out of both ABIs: it is, or is inside, a
     * private field, method or constructor, or a local or anonymous class. Private member types are
     * not left out; both ABIs keep them with their non-private members.
     */
    private static boolean isOutsideAbi(Element element) {
      for (Element scope = element; scope != null; scope = scope.getEnclosingElement()) {
        if (scope instanceof TypeElement) {
          NestingKind nestingKind = ((TypeElement) scope).getNestingKind();
          if (nestingKind == NestingKind.LOCAL || nestingKind == NestingKind.ANONYMOUS) {
            return true;
          }
        } else if (scope.getModifiers().contains(Modifier.PRIVATE)) {
          return true;
        }
      }
      return false;
    }

    @Override
    public void onConstantReferenceFound(
        VariableElement constant, TreePath path, Element referencingElement) {
      if (inheritedMemberTypesOnly) {
        return;
      }
      TypeElement constantEnclosingType = (TypeElement) constant.getEnclosingElement();

      if (fileManager.typeWillBeAvailable(constantEnclosingType)) {
        // All good!
        return;
      }

      String owningTarget = fileManager.getOwningTarget(constant);
      trees.printMessage(
          messageKind,
          String.format(
              "This constant will not be available during source-only ABI generation.\n"
                  + "For a quick fix, add required_for_source_only_abi = True to %s.\n"
                  + "A better fix is to move %s to a new rule that contains only\n"
                  + "constants, and mark that rule required_for_source_only_abi.\n",
              owningTarget, constant.getEnclosingElement().getSimpleName()),
          path.getLeaf(),
          path.getCompilationUnit());
    }
  }
}
