/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util

import com.facebook.kotlin.compilerplugins.kosabi.common.FullTypeQualifier
import com.sun.source.tree.ClassTree
import com.sun.source.tree.CompilationUnitTree
import com.sun.source.tree.ExpressionTree
import com.sun.source.tree.IdentifierTree
import com.sun.source.tree.MemberSelectTree
import com.sun.source.tree.ParameterizedTypeTree
import com.sun.source.tree.Tree
import com.sun.source.util.JavacTask
import com.sun.source.util.TreeScanner
import java.io.File
import java.nio.charset.StandardCharsets
import java.util.Collections
import java.util.IdentityHashMap
import javax.tools.Diagnostic
import javax.tools.DiagnosticCollector
import javax.tools.JavaFileObject
import javax.tools.ToolProvider

/**
 * What stubgen needs from a Java source: its package, explicit imports, declared types, and
 * fully-qualified usages. Parsed with javac, not regex: PSI's Java implementation isn't on the
 * kotlinc classpath, and KSP runs after stubgen. A textual fallback covers runtimes without a Java
 * compiler. Shared by the kotlinc plugin and the standalone CLI.
 */
data class JavaSourceInfo(
    val file: File,
    val packageSegments: List<String>,
    /** Explicit imports, static included. Star imports land in [starImports], never here. */
    val imports: List<FullTypeQualifier>,
    /**
     * Raw star imports. Stubgen ignores them (like their Kotlin counterparts) and kosabi.py gates
     * them at onboarding, so existing targets keep building unchanged.
     */
    val starImports: List<String>,
    /** Top-level and nested declared types, as package + outer[.inner]* qualifiers. */
    val declaredTypes: Set<FullTypeQualifier>,
    /**
     * Fully-qualified type usages. A dotted-path heuristic over AST member selects: it
     * over-approximates (a static call `foo.bar.Baz()` reads as a type), but wrong guesses stay
     * inert — in valid code every package.Type path in type position names a real type, and
     * expression positions never resolve against stub types.
     */
    val fullQualifierTypes: Set<FullTypeQualifier>,
    /**
     * Each parameterized type the source writes, by the names it is written with, paired with its
     * type-argument count: `Box<String>` gives `[Box] to 1`, and `Outer<A, B>.Inner<C>` gives
     * `[Outer] to 2` and `[Outer, Inner] to 1`. A type that only Java writes with type arguments
     * gets its stub's arity from here. A use in the body of a class that may inherit member types
     * (one with a supertype, an enum, or an anonymous class) is left out: a name there may denote
     * an inherited member type that no source shows.
     */
    val genericTypeUsages: Set<Pair<List<String>, Int>> = emptySet(),
    /**
     * The static ones among [imports]. A static import binds members, which may be fields or
     * methods, so only the other imports bind a type name for sure.
     */
    val staticImports: Set<FullTypeQualifier> = emptySet(),
    /**
     * The simple name of every type the source declares, nested and local ones included, as
     * written. [declaredTypes] splits package, type and member names by casing, which reads an
     * all-caps or lower-case nested name (`Use.BOX`) as a member.
     */
    val declaredSimpleNames: Set<String> = emptySet(),
)

private val systemJavaCompiler by
    lazy(LazyThreadSafetyMode.PUBLICATION) {
      ToolProvider.getSystemJavaCompiler()
    }

fun parseJavaSource(file: File): JavaSourceInfo {
  val compiler = systemJavaCompiler
  return if (compiler != null) parseJavaSourceAst(file, compiler) else parseJavaSourceTextual(file)
}

/**
 * Declarations only, never javac: package plus top-level and nested declared types, with empty
 * imports and usages. Stubgen takes this path for modules without KSP processors — nothing reads
 * Java imports there — while the declared types still suppress duplicate stubs for same-module Java
 * classes (PrimitiveStubsGenerator stubs every import missing from declaredTypes).
 */
fun parseJavaDeclarations(files: List<File>): List<JavaSourceInfo> {
  val failures = mutableListOf<Pair<File, Exception>>()
  val parsed = files.mapNotNull { file ->
    try {
      val scan = scanJavaTextual(file.readText())
      JavaSourceInfo(
          file,
          scan.pkg,
          emptyList(),
          emptyList(),
          scan.declaredTypes,
          emptySet(),
      )
    } catch (e: Exception) {
      failures += file to e
      null
    }
  }
  if (failures.isNotEmpty()) {
    throw IllegalStateException(
        failures.joinToString(prefix = "Failed to parse Java sources for stubgen: ") { (file, e) ->
          "${file.absolutePath} (${e.message})"
        },
        failures.first().second,
    )
  }
  return parsed
}

/** Parse-only; attribution would need the full classpath stubgen is trying to shrink. */
private fun parseJavaSourceAst(
    file: File,
    compiler: javax.tools.JavaCompiler,
): JavaSourceInfo {
  val diagnostics = DiagnosticCollector<JavaFileObject>()
  val fileManager = compiler.getStandardFileManager(diagnostics, null, StandardCharsets.UTF_8)
  try {
    val task =
        compiler.getTask(
            null,
            fileManager,
            diagnostics,
            listOf("-proc:none"),
            null,
            fileManager.getJavaFileObjects(file),
        ) as? JavacTask ?: return parseJavaSourceTextual(file)
    val units = task.parse().toList()
    if (units.size != 1) throw IllegalStateException("expected one compilation unit for $file")
    val errors = diagnostics.diagnostics.filter { it.kind == Diagnostic.Kind.ERROR }
    if (errors.isNotEmpty()) {
      // Degrade to the textual scan instead of aborting the batch: a file this single-file
      // parse-only javac rejects (preview syntax, source-level mismatch) still yields package,
      // imports, and declarations textually.
      return parseJavaSourceTextual(file)
    }
    return readJavaCompilationUnit(file, units[0])
  } finally {
    fileManager.close()
  }
}

private fun readJavaCompilationUnit(file: File, unit: CompilationUnitTree): JavaSourceInfo {
  val pkg = unit.packageName?.toString()?.split(".").orEmpty().filter { it.isNotEmpty() }
  val imports = mutableListOf<FullTypeQualifier>()
  val staticImports = mutableSetOf<FullTypeQualifier>()
  val stars = mutableListOf<String>()
  for (importTree in unit.imports) {
    val path = importTree.qualifiedIdentifier.toString()
    if (path.endsWith(".*")) {
      stars += "import ${if (importTree.isStatic) "static " else ""}$path;"
    } else {
      val qualifier = FullTypeQualifier(path.split("."))
      imports += qualifier
      if (importTree.isStatic) staticImports += qualifier
    }
  }
  val usages = JavaUsageCollector()
  for (decl in unit.typeDecls) usages.scan(decl, Unit)
  return JavaSourceInfo(
      file,
      pkg,
      imports,
      stars,
      usages.declaredTypes(pkg),
      usages.fullQualifierTypes,
      usages.genericTypeUsages,
      staticImports,
      usages.declaredSimpleNames(),
  )
}

private class JavaUsageCollector : TreeScanner<Unit, Unit>() {
  private val enclosing = ArrayDeque<String>()
  private val declared = mutableSetOf<List<String>>()
  val fullQualifierTypes = mutableSetOf<FullTypeQualifier>()
  val genericTypeUsages = mutableSetOf<Pair<List<String>, Int>>()

  /**
   * For each class being visited, innermost last: its members when it may inherit member types,
   * else nothing. Only the members are in its body; its header (type parameters, `extends`,
   * `implements`) is in the scope around it.
   */
  private val inheritingClassMembers = ArrayDeque<Set<Tree>>()

  /** How many bodies of classes that may inherit member types enclose the node being scanned. */
  private var inheritingBodies = 0

  fun declaredTypes(pkg: List<String>) =
      declared.mapTo(mutableSetOf()) { FullTypeQualifier(pkg + it) }

  fun declaredSimpleNames() = declared.mapTo(mutableSetOf()) { it.last() }

  override fun scan(tree: Tree?, p: Unit?): Unit? {
    val entersInheritingBody =
        tree != null && inheritingClassMembers.lastOrNull()?.contains(tree) == true
    if (entersInheritingBody) inheritingBodies++
    try {
      return super.scan(tree, p)
    } finally {
      if (entersInheritingBody) inheritingBodies--
    }
  }

  override fun visitClass(node: ClassTree, p: Unit) {
    val name = node.simpleName.toString()
    // `Enum` declares a member type, and an anonymous class always has a supertype.
    val mayInheritMemberTypes =
        name.isEmpty() ||
            node.kind == Tree.Kind.ENUM ||
            node.extendsClause != null ||
            node.implementsClause.isNotEmpty()
    inheritingClassMembers.addLast(
        if (mayInheritMemberTypes) {
          Collections.newSetFromMap(IdentityHashMap<Tree, Boolean>()).apply {
            addAll(node.members)
          }
        } else {
          emptySet()
        },
    )
    try {
      if (name.isEmpty()) return super.visitClass(node, p)
      declared += enclosing + name
      enclosing.addLast(name)
      try {
        super.visitClass(node, p)
      } finally {
        enclosing.removeLast()
      }
    } finally {
      inheritingClassMembers.removeLast()
    }
  }

  override fun visitParameterizedType(node: ParameterizedTypeTree, p: Unit) {
    // A diamond (`new Box<>()`) writes no type arguments, so it says nothing about the arity, and
    // in a body that may inherit member types the name may denote one that no source shows.
    if (node.typeArguments.isNotEmpty() && inheritingBodies == 0) {
      typeSegments(node.type)?.let { genericTypeUsages += it to node.typeArguments.size }
    }
    super.visitParameterizedType(node, p)
  }

  /**
   * The names a type is written with. An outer written with type arguments of its own
   * (`Outer<String>.Inner`) gives its names here and its own arity when the scan reaches it.
   */
  private fun typeSegments(tree: Tree): List<String>? =
      when (tree) {
        is IdentifierTree -> listOf(tree.name.toString())
        is MemberSelectTree -> typeSegments(tree.expression)?.plus(tree.identifier.toString())
        is ParameterizedTypeTree -> typeSegments(tree.type)
        else -> null
      }

  override fun visitMemberSelect(node: MemberSelectTree, p: Unit) {
    dottedPath(node)?.let {
      FQ_TYPE_RE.find(it)?.let { match ->
        fullQualifierTypes +=
            FullTypeQualifier(
                match.groupValues[1].split(".") + match.groupValues[2].split("."),
            )
      }
    }
    super.visitMemberSelect(node, p)
  }

  private fun dottedPath(node: MemberSelectTree): String? {
    val parts = ArrayDeque<String>()
    var expression: ExpressionTree = node
    while (expression is MemberSelectTree) {
      parts.addFirst(expression.identifier.toString())
      expression = expression.expression
    }
    if (expression is IdentifierTree) parts.addFirst(expression.name.toString()) else return null
    return parts.joinToString(".")
  }
}

private fun parseJavaSourceTextual(file: File): JavaSourceInfo {
  val scan = scanJavaTextual(file.readText())
  return JavaSourceInfo(
      file,
      scan.pkg,
      scan.imports,
      scan.stars,
      scan.declaredTypes,
      scan.fullQualifierTypes,
  )
}

fun parseJavaSources(files: List<File>): List<JavaSourceInfo> {
  val failures = mutableListOf<Pair<File, Exception>>()
  val parsed = files.mapNotNull { file ->
    try {
      parseJavaSource(file)
    } catch (e: Exception) {
      failures += file to e
      null
    }
  }
  if (failures.isNotEmpty()) {
    throw IllegalStateException(
        failures.joinToString(prefix = "Failed to parse Java sources for stubgen: ") { (file, e) ->
          "${file.absolutePath} (${e.message})"
        },
        failures.first().second,
    )
  }
  return parsed
}

private val FQ_TYPE_RE =
    Regex("""(?<![\w$.])([a-z]\w*(?:\.[a-z]\w*)*)\.([A-Z]\w*(?:\.[A-Z]\w*)*)""")

data class JavaTextualScan(
    val pkg: List<String>,
    val imports: List<FullTypeQualifier>,
    val stars: List<String>,
    val declaredTypes: Set<FullTypeQualifier>,
    val fullQualifierTypes: Set<FullTypeQualifier>,
)

private val TYPE_KEYWORDS = setOf("class", "interface", "enum", "record")

private fun Char.isJavaIdentPart(): Boolean = this == '_' || isLetterOrDigit()

/**
 * One pass over the file: comments and literals are skipped inline (blanked with spaces so token
 * separation matches a stripped view), while package, imports, brace depth, and type declarations
 * are collected directly. Newlines end a line even inside an unterminated literal, mirroring the
 * old strip-then-regex behavior.
 *
 * Import and package statements must sit on one line each; google-java-format never breaks them,
 * and a missed statement only drops textual-fallback precision (javac stays the primary parse).
 */
fun scanJavaTextual(text: String): JavaTextualScan {
  var pkg = emptyList<String>()
  val imports = mutableListOf<FullTypeQualifier>()
  val stars = mutableListOf<String>()
  val usages = mutableSetOf<FullTypeQualifier>()
  data class Decl(val name: String, val depth: Int)
  val decls = mutableListOf<Decl>()

  val line = StringBuilder()
  var depth = 0
  var wordStart = -1
  var pendingKeywordDepth = -1
  var i = 0

  fun endWord(end: Int) {
    if (wordStart < 0) return
    val word = text.substring(wordStart, end)
    val keywordStart = wordStart
    wordStart = -1
    if (pendingKeywordDepth >= 0) {
      decls += Decl(word, pendingKeywordDepth)
      pendingKeywordDepth = -1
      return
    }
    if (word in TYPE_KEYWORDS && (keywordStart == 0 || text[keywordStart - 1] != '.')) {
      pendingKeywordDepth = depth
    }
  }

  fun String.isStmt(keyword: String): Boolean =
      startsWith(keyword) && getOrNull(keyword.length)?.isWhitespace() == true

  fun endLine() {
    val s = line.toString().trim()
    when {
      s.isStmt("package") -> {
        pkg =
            s.removePrefix("package").trim().substringBefore(';').split(".").filter {
              it.isNotEmpty()
            }
      }
      s.isStmt("import") -> {
        val semi = s.indexOf(';')
        if (semi > 0) {
          val rest = s.substring(6, semi).trim()
          val path = if (rest.isStmt("static")) rest.substring(6).trim() else rest
          if (path.endsWith(".*")) {
            stars += s.substring(0, semi + 1)
          } else {
            // Malformed paths (`a..b`, `.a`) are dropped: a recorded wrong import would
            // fabricate a bogus stub, while a dropped one only misses a suppression.
            val segments = path.split(".")
            if (segments.all { it.isNotEmpty() && it.all { c -> c.isJavaIdentPart() } }) {
              imports += FullTypeQualifier(segments)
            }
          }
        }
      }
      else -> {
        for (match in FQ_TYPE_RE.findAll(line)) {
          usages +=
              FullTypeQualifier(
                  match.groupValues[1].split(".") + match.groupValues[2].split("."),
              )
        }
      }
    }
    line.clear()
  }

  fun blankTo(end: Int) {
    for (j in i until end) if (text[j] == '\n') endLine() else line.append(' ')
    i = end
  }

  while (i < text.length) {
    when {
      text.startsWith("//", i) -> {
        endWord(i)
        blankTo(text.indexOf('\n', i).let { if (it == -1) text.length else it })
      }
      text.startsWith("/*", i) -> {
        endWord(i)
        blankTo(text.indexOf("*/", i + 2).let { if (it == -1) text.length else it + 2 })
      }
      text.startsWith("\"\"\"", i) -> {
        endWord(i)
        blankTo(text.indexOf("\"\"\"", i + 3).let { if (it == -1) text.length else it + 3 })
      }
      text[i] == '"' || text[i] == '\'' -> {
        endWord(i)
        val quote = text[i]
        line.append(' ')
        i++
        while (i < text.length && text[i] != quote) {
          if (text[i] == '\\' && i + 1 < text.length) {
            line.append("  ")
            i += 2
          } else if (text[i] == '\n') {
            endLine()
            i++
          } else {
            line.append(' ')
            i++
          }
        }
        if (i < text.length) {
          line.append(' ')
          i++
        }
      }
      text[i] == '\n' -> {
        endWord(i)
        endLine()
        i++
      }
      text[i].isJavaIdentPart() -> {
        if (wordStart < 0) wordStart = i
        line.append(text[i])
        i++
      }
      else -> {
        if (wordStart >= 0) {
          endWord(i)
        } else if (text[i] != ' ' && text[i] != '\t' && text[i] != '\r') {
          // A non-whitespace token ends a dangling keyword: the name must follow the keyword
          // through whitespace alone, mirroring `keyword\s+NAME`.
          pendingKeywordDepth = -1
        }
        when (text[i]) {
          '{' -> depth++
          '}' -> if (depth > 0) depth--
        }
        line.append(text[i])
        i++
      }
    }
  }
  endWord(text.length)
  if (line.isNotEmpty()) endLine()

  val result = mutableSetOf<FullTypeQualifier>()
  val stack = ArrayDeque<Decl>()
  for (decl in decls) {
    while (stack.isNotEmpty() && stack.last().depth >= decl.depth) stack.removeLast()
    stack.addLast(decl)
    result += FullTypeQualifier(pkg + stack.map { it.name })
  }
  return JavaTextualScan(pkg, imports, stars, result, usages)
}
