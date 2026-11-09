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
import com.sun.source.tree.Tree;
import com.sun.source.util.TreePath;
import com.sun.source.util.Trees;
import java.lang.annotation.Retention;
import java.lang.annotation.RetentionPolicy;
import java.util.List;
import java.util.Objects;
import java.util.stream.Collectors;
import javax.lang.model.element.Element;
import javax.lang.model.element.ElementKind;
import javax.lang.model.element.Modifier;
import javax.lang.model.element.Name;
import javax.lang.model.element.PackageElement;
import javax.lang.model.element.QualifiedNameable;
import javax.lang.model.element.TypeElement;
import javax.lang.model.element.VariableElement;
import javax.lang.model.type.DeclaredType;
import javax.lang.model.type.TypeKind;
import javax.lang.model.type.TypeMirror;
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
    InterfaceScanner interfaceScanner = new InterfaceScanner(trees);
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
    private final Trees trees;
    private final SourceOnlyAbiRuleInfo ruleInfo;
    private final TreeBackedTypeResolutionSimulator treeBackedResolver;
    private final FileManagerSimulator fileManager;
    private final CompletionSimulator completer;
    private final CompilerTypeResolutionSimulator compilerResolver;
    private final ImportsTracker imports;
    private final boolean inheritedMemberTypesOnly;

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
            || isInDiscardedSourceAnnotation(path)) {
          return;
        }
        for (Element scope = referencingElement;
            scope != null;
            scope = scope.getEnclosingElement()) {
          if (scope.getModifiers().contains(Modifier.PRIVATE)) {
            return;
          }
        }
      }
      compilerResolver.setImports(imports);
      treeBackedResolver.setImports(imports);
      ResolvedType compilerResolvedType = compilerResolver.resolve(path);

      if (inheritedMemberTypesOnly) {
        if (!compilerResolvedType.missingMemberSupertypes.isEmpty()) {
          trees.printMessage(
              messageKind,
              String.format(
                  "Inherited member type %s requires missing supertypes during source-only ABI"
                      + " generation.\n"
                      + "Use the fully qualified type or add the providers of the following classes"
                      + " to source_only_abi_deps in %s.\n"
                      + "KOSABI_JASABI_MISSING_SUPERTYPES: %s",
                  canonicalTypeElement.getQualifiedName(),
                  ruleInfo.getRuleName(),
                  compilerResolvedType.missingMemberSupertypes.stream()
                      .sorted()
                      .collect(Collectors.joining(", "))),
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
