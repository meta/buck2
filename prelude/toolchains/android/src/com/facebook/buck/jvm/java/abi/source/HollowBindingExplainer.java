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

import com.facebook.buck.util.liteinfersupport.Nullable;
import com.sun.source.tree.ClassTree;
import com.sun.source.tree.CompilationUnitTree;
import com.sun.source.tree.IdentifierTree;
import com.sun.source.tree.ImportTree;
import com.sun.source.tree.MemberSelectTree;
import com.sun.source.tree.ParameterizedTypeTree;
import com.sun.source.tree.Tree;
import com.sun.source.util.TreePath;
import com.sun.source.util.TreeScanner;
import com.sun.source.util.Trees;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeSet;
import javax.lang.model.element.Element;
import javax.lang.model.element.TypeElement;
import javax.lang.model.type.TypeKind;
import javax.lang.model.type.TypeMirror;

/**
 * Explains source-only ABI arity crashes. Resolving a generic reference through an incomplete
 * supertype hierarchy can bind a hollow element with no type parameters, and building the
 * parameterized type then throws {@code IllegalArgumentException: Incorrect number of type
 * arguments} naming nothing fixable. The explanation names the unavailable supertypes of the
 * enclosing class and the fix.
 */
class HollowBindingExplainer {
  static IllegalStateException explain(
      Trees javacTrees,
      @Nullable TreePath treePath,
      String hollowName,
      IllegalArgumentException cause) {
    try {
      if (treePath == null) {
        return new IllegalStateException(
            "Source-only ABI generation bound a hollow "
                + hollowName
                + " with no type parameters and cannot emit a correct ABI.",
            cause);
      }
      return new IllegalStateException(buildMessage(javacTrees, treePath, hollowName), cause);
    } catch (RuntimeException e) {
      return new IllegalStateException(fallbackMessage(treePath, hollowName), cause);
    }
  }

  private static String buildMessage(Trees javacTrees, TreePath treePath, String hollowName) {
    TreePath classPath = enclosingClassPath(treePath);
    if (classPath == null) {
      return fallbackMessage(treePath, hollowName);
    }
    ClassTree classTree = (ClassTree) classPath.getLeaf();
    Element element = javacTrees.getElement(classPath);
    if (!(element instanceof TypeElement)) {
      return fallbackMessage(treePath, hollowName);
    }
    TypeElement type = (TypeElement) element;
    Set<String> broken = new TreeSet<>();
    Set<String> approximate = new TreeSet<>();
    collectBrokenLinks(classTree, type, classPath.getCompilationUnit(), broken, approximate);
    if (broken.isEmpty() && approximate.isEmpty()) {
      return fallbackMessage(treePath, hollowName);
    }
    List<String> named = broken.isEmpty() ? new ArrayList<>(approximate) : new ArrayList<>(broken);
    StringBuilder message = new StringBuilder();
    message.append(
        String.format(
            "Source-only ABI generation cannot emit a correct ABI for %s (in %s): resolving `%s`"
                + " bound a hollow %s with no type parameters because the following supertypes"
                + " are unavailable on the reduced classpath: %s.\n",
            classFqn(classPath, type),
            treePath.getCompilationUnit().getSourceFile().getName(),
            treePath.getLeaf(),
            hollowName,
            String.join(", ", named)));
    if (!broken.isEmpty() && !approximate.isEmpty()) {
      message.append(
          String.format(
              "Further unreadable supertype links (names as spelled): %s.\n",
              String.join(", ", approximate)));
    }
    message.append("To fix, add the BUCK targets providing these types to source_only_abi_deps.\n");
    message.append("KOSABI_JASABI_MISSING_SUPERTYPES: ").append(String.join(", ", broken));
    return message.toString();
  }

  private static String fallbackMessage(TreePath treePath, String hollowName) {
    String file;
    try {
      file = treePath.getCompilationUnit().getSourceFile().getName();
    } catch (RuntimeException e) {
      file = "<unknown file>";
    }
    return String.format(
        "Source-only ABI generation cannot emit a correct ABI (in %s): resolving a generic"
            + " reference bound a hollow %s with no type parameters, but no direct supertype"
            + " link of the enclosing class is unreadable. The hierarchy may be incomplete"
            + " deeper down; add the BUCK targets providing its supertypes to"
            + " source_only_abi_deps.\n"
            + "KOSABI_JASABI_MISSING_SUPERTYPES: ",
        file, hollowName);
  }

  private static TreePath enclosingClassPath(TreePath path) {
    TreePath current = path;
    while (current != null && !(current.getLeaf() instanceof ClassTree)) {
      current = current.getParentPath();
    }
    return current;
  }

  private static String classFqn(TreePath classPath, TypeElement type) {
    try {
      return type.getQualifiedName().toString();
    } catch (RuntimeException e) {
      return classFqnFromTree(classPath);
    }
  }

  private static String classFqnFromTree(TreePath classPath) {
    List<String> names = new ArrayList<>();
    TreePath current = classPath;
    while (current != null) {
      if (current.getLeaf() instanceof ClassTree) {
        String simple = ((ClassTree) current.getLeaf()).getSimpleName().toString();
        // Collapse links repeated by manually built paths.
        if (!simple.isEmpty() && (names.isEmpty() || !names.get(0).equals(simple))) {
          names.add(0, simple);
        }
      } else if (current.getLeaf() instanceof CompilationUnitTree) {
        CompilationUnitTree unit = (CompilationUnitTree) current.getLeaf();
        if (unit.getPackageName() != null) {
          names.add(0, unit.getPackageName().toString());
        }
      }
      current = current.getParentPath();
    }
    return String.join(".", names);
  }

  private static void collectBrokenLinks(
      ClassTree classTree,
      TypeElement type,
      CompilationUnitTree unit,
      Set<String> broken,
      Set<String> approximate) {
    UnitNames names = UnitNames.collect(unit);
    TypeMirror superclass;
    try {
      superclass = type.getSuperclass();
    } catch (RuntimeException e) {
      return;
    }
    if (superclass != null
        && superclass.getKind() != TypeKind.NONE
        && superclass.getKind() == TypeKind.ERROR) {
      recordBroken(superclass, classTree.getExtendsClause(), names, broken, approximate);
    }
    List<? extends TypeMirror> interfaces;
    try {
      interfaces = type.getInterfaces();
    } catch (RuntimeException e) {
      return;
    }
    List<? extends Tree> implementsClause = classTree.getImplementsClause();
    for (int i = 0; i < interfaces.size(); i++) {
      if (interfaces.get(i).getKind() == TypeKind.ERROR) {
        Tree spelling = i < implementsClause.size() ? implementsClause.get(i) : null;
        recordBroken(interfaces.get(i), spelling, names, broken, approximate);
      }
    }
  }

  private static void recordBroken(
      TypeMirror mirror,
      Tree spelling,
      UnitNames names,
      Set<String> broken,
      Set<String> approximate) {
    String resolved = spelling == null ? null : names.resolve(spelling);
    if (resolved == null) {
      approximate.add(mirror.toString());
    } else {
      broken.add(resolved);
    }
  }

  /**
   * Resolves a supertype-clause spelling to a fully qualified name using the file's imports,
   * same-package and same-module types. Star imports cannot resolve; callers treat a null result as
   * approximate.
   */
  private static class UnitNames {
    private final String packageName;
    private final Map<String, String> singleImports = new HashMap<>();
    private final Map<String, String> declaredTypes = new HashMap<>();
    private final Set<String> ambiguousDeclared = new HashSet<>();
    private boolean hasStarImports = false;

    static UnitNames collect(CompilationUnitTree unit) {
      UnitNames names =
          new UnitNames(unit.getPackageName() == null ? "" : unit.getPackageName().toString());
      for (ImportTree importTree : unit.getImports()) {
        if (importTree.isStatic() || importTree.getQualifiedIdentifier() == null) {
          continue;
        }
        String qualified = importTree.getQualifiedIdentifier().toString();
        if (qualified.endsWith(".*")) {
          names.hasStarImports = true;
          continue;
        }
        int lastDot = qualified.lastIndexOf('.');
        if (lastDot > 0) {
          names.singleImports.put(qualified.substring(lastDot + 1), qualified);
        }
      }
      names.collectDeclared(unit, names.packageName);
      return names;
    }

    private void collectDeclared(Tree tree, String prefix) {
      new TreeScanner<Void, Void>() {
        @Override
        public Void visitClass(ClassTree node, Void unused) {
          String simple = node.getSimpleName().toString();
          if (!simple.isEmpty()) {
            String fqn = prefix.isEmpty() ? simple : prefix + "." + simple;
            if (declaredTypes.containsKey(simple) && !declaredTypes.get(simple).equals(fqn)) {
              ambiguousDeclared.add(simple);
            } else {
              declaredTypes.put(simple, fqn);
            }
            for (Tree member : node.getMembers()) {
              if (member instanceof ClassTree) {
                collectDeclared(member, fqn);
              }
            }
          }
          return null;
        }
      }.scan(tree, null);
    }

    private UnitNames(String packageName) {
      this.packageName = packageName;
    }

    /** Fully qualified name for a supertype-clause spelling, or null when unresolvable. */
    String resolve(Tree spelling) {
      String dotted = dottedName(spelling);
      if (dotted == null || dotted.isEmpty()) {
        return null;
      }
      int firstDot = dotted.indexOf('.');
      String head = firstDot < 0 ? dotted : dotted.substring(0, firstDot);
      String tail = firstDot < 0 ? "" : dotted.substring(firstDot);
      String imported = singleImports.get(head);
      if (imported != null) {
        return imported + tail;
      }
      if (!ambiguousDeclared.contains(head) && declaredTypes.containsKey(head)) {
        return declaredTypes.get(head) + tail;
      }
      if (ambiguousDeclared.contains(head)
          || head.isEmpty()
          || !Character.isUpperCase(head.charAt(0))
          || hasStarImports) {
        return null;
      }
      return packageName.isEmpty() ? dotted : packageName + "." + dotted;
    }

    private static String dottedName(Tree spelling) {
      if (spelling instanceof IdentifierTree) {
        return ((IdentifierTree) spelling).getName().toString();
      }
      if (spelling instanceof MemberSelectTree) {
        MemberSelectTree select = (MemberSelectTree) spelling;
        String prefix = dottedName(select.getExpression());
        if (prefix == null) {
          return null;
        }
        return prefix + "." + select.getIdentifier().toString();
      }
      if (spelling instanceof ParameterizedTypeTree) {
        return dottedName(((ParameterizedTypeTree) spelling).getType());
      }
      return null;
    }
  }
}
