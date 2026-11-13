/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::fmt::Debug;
use std::marker::PhantomData;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_query_derive::query_module;
use buck2_query_parser::BinaryOp;
use buck2_query_parser::Expr;
use buck2_query_parser::spanned::Spanned;
use variant_name::VariantName;

use crate::query::environment::QueryEnvironment;
use crate::query::syntax::simple::eval::error::QueryError;
use crate::query::syntax::simple::eval::evaluator::QueryEvaluator;
use crate::query::syntax::simple::eval::file_set::FileSet;
use crate::query::syntax::simple::eval::set::TargetSet;
use crate::query::syntax::simple::eval::values::QueryResult;
use crate::query::syntax::simple::eval::values::QueryValue;
use crate::query::syntax::simple::eval::values::QueryValueDepth;
use crate::query::syntax::simple::eval::values::QueryValueSet;
use crate::query::syntax::simple::functions::deps::DepsFunction;
use crate::query::syntax::simple::functions::docs::ModuleDescription;
use crate::query::syntax::simple::functions::helpers::CapturedExpr;
use crate::query::syntax::simple::functions::helpers::QueryArgType;
use crate::query::syntax::simple::functions::helpers::QueryBinaryOp;
use crate::query::syntax::simple::functions::helpers::QueryFunction;

pub mod deps;
pub mod description;
pub mod docs;
pub mod helpers;

pub trait QueryLiteralVisitor<'a> {
    fn target_pattern(&mut self, pattern: &'a str) -> buck2_error::Result<()>;
}

pub trait HasModuleDescription {
    fn describe() -> ModuleDescription;
}

#[async_trait]
pub trait QueryFunctions: Debug + Send + Sync {
    type Env: QueryEnvironment;

    fn get(&self, name: &str) -> Option<&dyn QueryFunction<Self::Env>>;

    fn get_op(&self, op: BinaryOp) -> Option<&dyn QueryBinaryOp<Self::Env>>;
}

pub trait QueryFunctionsVisitLiterals: Debug + Send + Sync {
    fn visit_literals<'q>(
        &self,
        visitor: &mut dyn QueryLiteralVisitor<'q>,
        expr: &Spanned<Expr<'q>>,
    ) -> QueryResult<()>;
}

impl<F: QueryFunctions> QueryFunctionsVisitLiterals for F {
    fn visit_literals<'q>(
        &self,
        visitor: &mut dyn QueryLiteralVisitor<'q>,
        expr: &Spanned<Expr<'q>>,
    ) -> QueryResult<()> {
        fn visit_literals_recurse<'q, F: QueryFunctions>(
            this: &F,
            visitor: &mut dyn QueryLiteralVisitor<'q>,
            expr: &Expr<'q>,
        ) -> Result<(), QueryError> {
            match expr {
                Expr::None => Ok(()),
                Expr::Function {
                    function_name,
                    args,
                } => match this.get(function_name) {
                    Some(func) => {
                        for (i, arg) in args.iter().enumerate() {
                            visit_literals_item(
                                this,
                                visitor,
                                arg,
                                matches!(
                                    func.arg_type(i)?,
                                    QueryArgType::TargetSet
                                        | QueryArgType::Set
                                        | QueryArgType::Value
                                ),
                            )?;
                        }
                        Ok(())
                    }
                    None => Err(QueryError::UnknownFunction(
                        (*function_name.fragment()).to_owned(),
                    )),
                },
                Expr::BinaryOpSequence(left, exprs) => {
                    visit_literals_item(this, visitor, left, true)?;
                    // All binary ops are on targetsets currently.
                    for (_, right) in exprs {
                        visit_literals_item(this, visitor, right, true)?;
                    }
                    Ok(())
                }
                Expr::Set(args) => {
                    for arg in args {
                        visitor.target_pattern(arg.fragment())?;
                    }
                    Ok(())
                }
                Expr::FileSet(_args) => Ok(()),
                Expr::String(..) | Expr::Integer(..) => {
                    panic!(
                        "This shouldn't be called with literals, they should be handled in the caller"
                    )
                }
            }
        }

        fn visit_literals_item<'q, F: QueryFunctions>(
            this: &F,
            visitor: &mut dyn QueryLiteralVisitor<'q>,
            expr: &Spanned<Expr<'q>>,
            is_target_expr: bool,
        ) -> QueryResult<()> {
            expr.map_res(|value| -> Result<(), QueryError> {
                match value {
                    Expr::String(val) => {
                        if is_target_expr {
                            visitor.target_pattern(val)?;
                        }
                    }
                    Expr::Integer(..) => {
                        // ignored
                    }
                    _ => visit_literals_recurse(this, visitor, value)?,
                }
                Ok(())
            })
        }

        visit_literals_item(self, visitor, expr, true)
    }
}

#[derive(Allocative)]
#[allocative(bound = "")]
pub struct DefaultQueryFunctionsModule<Env: QueryEnvironment> {
    implementation: DefaultQueryFunctions<Env>,
}

impl<Env: QueryEnvironment> Debug for DefaultQueryFunctionsModule<Env> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DefaultQueryFunctionsModule")
            .finish_non_exhaustive()
    }
}

impl<Env: QueryEnvironment> DefaultQueryFunctionsModule<Env> {
    pub fn new() -> Self {
        Self {
            implementation: DefaultQueryFunctions::new(),
        }
    }
}

type QueryFuncResult<Env> =
    std::result::Result<QueryValue<<Env as QueryEnvironment>::Target>, QueryError>;

/// Common query functions
#[query_module(Env)]
impl<Env: QueryEnvironment> DefaultQueryFunctionsModule<Env> {
    /// All dependency paths.
    ///
    /// Generates a graph of paths between the [*target expressions*](#target-expression) *from* and *to*, based on the dependencies between nodes.
    ///
    /// For example:
    /// ```text
    /// $ buck2 uquery "allpaths('//foo:bar', '//foo/bar/lib:baz')"
    /// ```
    /// returns the dependency graph rooted at the target node `//foo:bar`, including all target nodes that transitively depend on `//foo/bar/lib:baz`.
    ///
    /// Arguments *from* and *to* can themselves be expressions, for example:
    /// ```text
    /// $ buck2 uquery "allpaths(kind(java_library, '//...'), '//foo:bar')"
    /// ```
    /// shows all the paths between any target with rule type `java_library` in the repository and the target `//foo:bar`.
    ///
    /// We recommend using it with the `--output-format=dot` parameter to generate a [Graphviz](https://graphviz.org/) [DOT](https://graphviz.org/doc/info/lang.html) file that can then be rendered as an image.
    ///
    /// ```text
    /// $ buck2 uquery "allpaths(//buck2:buck2, //buck2/app/buck2_validation:buck2_validation)" --output-format=dot > result.dot
    /// $ dot -Tpng result.dot -o image.png
    /// ```
    /// produces the following image:
    /// <img src={useBaseUrl('/img/allpaths_example.png')} class='query-example-image'/>
    async fn allpaths(
        &self,
        evaluator: &QueryEvaluator<'_, Env>,
        from: TargetSet<Env::Target>,
        to: TargetSet<Env::Target>,
        captured_expr: Option<CapturedExpr<'_>>,
    ) -> QueryFuncResult<Env> {
        Ok(self
            .implementation
            .allpaths(
                evaluator.env(),
                evaluator.functions(),
                &from,
                &to,
                captured_expr.as_ref(),
            )
            .await?
            .into())
    }

    /// Shortest dependency path between two sets of targets.
    ///
    /// * The first parameter `from` represents the upstream targets (e.g., final binary).
    /// * The second parameter `to` represents the downstream targets (e.g., a library).
    ///
    /// Results are returned in order from top to bottom (upstream to downstream).
    ///
    /// If multiple paths exist, the returned path is unspecified. If no path exists, an empty set is returned.
    ///
    /// For example:
    ///
    /// ```text
    /// $ buck2 uquery 'somepath(//buck2:buck2, //buck2/app/buck2_node:buck2_node)'
    ///
    /// //buck2:buck2
    /// //buck2/app/buck2:buck2-bin
    /// //buck2/app/buck2_analysis:buck2_analysis
    /// //buck2/app/buck2_node:buck2_node
    /// ```
    async fn somepath(
        &self,
        evaluator: &QueryEvaluator<'_, Env>,
        from: TargetSet<Env::Target>,
        to: TargetSet<Env::Target>,
        captured_expr: Option<CapturedExpr<'_>>,
    ) -> QueryFuncResult<Env> {
        Ok(self
            .implementation
            .somepath(
                evaluator.env(),
                evaluator.functions(),
                &from,
                &to,
                captured_expr.as_ref(),
            )
            .await?
            .into())
    }

    /// Rule attribute filtering.
    ///
    /// Evaluates the given [*target expression*](#target-expression) and filters the resulting build targets to those where the specified attribute contains the specified value.
    /// In this context, the term attribute refers to an argument in a build rule, such as name, headers, srcs, or deps.
    ///
    /// - If the attribute is a single value, say `name`, it is compared to the specified value, and the target is returned if they match.
    /// - If the attribute is a list, the target is returned if that list contains the specified value.
    /// - If the attribute is a dictionary, the target is returned if the value exists in either the keys or the values of the dictionary.
    ///
    /// For example:
    /// ```text
    /// $ buck2 uquery "attrfilter(deps, '//buck2/app/buck2_validation:buck2_validation', '//...')"
    ///
    /// //buck2/app/buck2:buck2-bin
    /// //buck2/app/buck2_server:buck2_server
    /// //buck2/app/buck2_server:buck2_server-unittest
    /// ```
    /// returns targets that contain `//buck2/app/buck2_validation:buck2_validation` target in their `deps` attribute.
    async fn attrfilter(
        &self,
        attr: String,
        value: String,
        targets: TargetSet<Env::Target>,
    ) -> QueryFuncResult<Env> {
        Ok(self
            .implementation
            .attrfilter(&attr, &value, &targets)?
            .into())
    }

    /// Negative rule attribute filtering, opposite of [`attrfilter`](#attrfilter).
    ///
    /// Evaluates the given [*target expression*](#target-expression) and filters the resulting build targets to those where the specified attribute doesn't contain the specified value.
    /// In this context, the term attribute refers to an argument in a build rule, such as name, headers, srcs, or deps.
    ///
    /// - If the attribute is a single value, say `name`, it is compared to the specified value, and the target is returned if they don't match.
    /// - If the attribute is a list, the target is returned if that list doesn't contain the specified value.
    /// - If the attribute is a dictionary, the target is returned if the value doesn't exist in both the keys and the values of the dictionary.
    ///
    /// For example:
    /// ```text
    /// $ buck2 uquery "nattrfilter(deps, '//foo:bar', '//...')"
    /// ```
    /// returns targets that don't contain `//foo:bar` target in their `deps` attribute.
    async fn nattrfilter(
        &self,
        attr: String,
        value: String,
        targets: TargetSet<Env::Target>,
    ) -> QueryFuncResult<Env> {
        Ok(self
            .implementation
            .nattrfilter(&attr, &value, &targets)?
            .into())
    }

    /// Rule attribute filtering with regex.
    ///
    /// Similar to the [`attrfilter`](#attrfilter) function except that it takes a regular expression as the second argument.
    ///
    /// For example:
    /// ```text
    /// $ buck2 uquery "attrregexfilter(deps, '.+validation$', '//...')"
    ///
    /// //buck2/app/buck2:buck2-bin
    /// //buck2/app/buck2_server:buck2_server
    /// //buck2/app/buck2_server:buck2_server-unittest
    /// ```
    /// returns targets whose `deps` attribute contains at least one target suffixed with 'validation'.
    async fn attrregexfilter(
        &self,
        attr: String,
        value: String,
        targets: TargetSet<Env::Target>,
    ) -> QueryFuncResult<Env> {
        Ok(self
            .implementation
            .attrregexfilter(&attr, &value, &targets)?
            .into())
    }

    /// Target build file.
    ///
    /// For each target in the provided [*target expression*](#target-expression), returns the build file where the target is defined.
    ///
    /// For example:
    /// ```text
    /// $ buck2 uquery 'buildfile(//buck2:buck2)'
    ///
    /// buck2/BUCK
    /// ```
    ///
    /// In order to find the build file associated with a source file, combine the owner operator with buildfile.
    /// Examples:
    /// ```text
    /// $ buck2 uquery "buildfile(//buck2/app/buck2_action_impl_tests:buck2_action_impl_tests)"
    /// ```
    /// and
    /// ```text
    /// $ buck2 uquery "buildfile(owner(app/buck2_action_impl_tests/src/context.rs))"
    /// ```
    /// both return `buck2/app/buck2_action_impl_tests/TARGETS`.
    async fn buildfile(&self, targets: TargetSet<Env::Target>) -> QueryFuncResult<Env> {
        Ok(self.implementation.buildfile(&targets)?.into())
    }

    /// Build file reverse dependencies.
    ///
    /// Returns all build files in the provided `universe` that have a transitive dependency on any of the specified build files.
    ///
    /// For example:
    /// ```text
    /// $ buck2 uquery "rbuildfiles(//buck2/BUCK, //buck2/defs.bzl)"
    ///
    /// buck2/defs.bzl
    /// buck2/BUCK
    /// ```
    async fn rbuildfiles(
        &self,
        env: &Env,
        universe: FileSet,
        argset: FileSet,
    ) -> QueryFuncResult<Env> {
        Ok(self
            .implementation
            .rbuildfiles(env, &universe, &argset)
            .await?
            .into())
    }

    /// Target build file with transitive imports.
    ///
    /// For each target in the provided [*target expression*](#target-expression),
    /// returns the build file where the target is defined,
    /// along with all transitive imports of that file.
    ///
    /// For example:
    /// ```text
    /// $ buck2 uquery 'allbuildfiles(//foo:bar)'
    ///
    /// foo/BUCK
    /// foo/defs_dependent_on_utils.bzl
    /// baz/utils.bzl
    /// ```
    async fn allbuildfiles(
        &self,
        env: &Env,
        universe: TargetSet<Env::Target>,
    ) -> QueryFuncResult<Env> {
        Ok(self
            .implementation
            .allbuildfiles(env, &universe)
            .await?
            .into())
    }

    /// Find the dependencies of the targets in the given target universe.
    ///
    /// The first parameter `targets` is a specific target or target pattern. It specifies the targets to find dependencies for.
    /// The second argument `depth` is an optional integer literal specifying an upper bound on the depth of the search. A value of one (1) specifies that buck query should return only direct dependencies. If the depth parameter is omitted, the search is unbounded.
    /// The third argument `captured_expr` is an optional filter expression that controls the
    /// traversal. The expression is evaluated for each node and returns the child nodes to recurse
    /// on when collecting transitive dependencies. Available filters are `configuration_deps()`,
    /// `exec_deps()`, `first_order_deps()`, `target_deps()`, and `toolchain_deps()`.
    ///
    /// The `first_order_deps()` operator returns a set that contains the first-order dependencies
    /// of the current node. It can be combined with other filter functions, for example,
    /// `kind(java_library, first_order_deps())` to make the `deps` traversal only traverse
    /// `java_library` targets. The `first_order_deps()` operator can only be used as an argument
    /// passed to `deps()`.
    ///
    /// The returned values include the nodes from the `targets` argument itself.
    ///
    /// For example following uquery:
    ///
    /// ```text
    /// $ buck2 uquery "deps(//buck2:buck2, 1)"
    /// ```
    /// returns all targets that `//buck2:buck2` depends on directly.
    async fn deps(
        &self,
        evaluator: &QueryEvaluator<'_, Env>,
        targets: TargetSet<Env::Target>,
        depth: QueryValueDepth,
        captured_expr: Option<CapturedExpr<'_>>,
    ) -> QueryFuncResult<Env> {
        Ok(self
            .implementation
            .deps(
                evaluator.env(),
                evaluator.functions(),
                &targets,
                depth,
                captured_expr.as_ref(),
            )
            .await?
            .into())
    }

    /// Filter targets or files by regex.
    ///
    /// Use regex partial match to filter either a [*target expression*](#target-expression) or [*file expression*](#file-expression).
    /// * Targets are matched against their fully-qualified name, such as `cell//foo/bar:baz`.
    /// * Files are matched against a repo path, such as `cell//foo/bar/baz.py`.
    ///
    /// For example:
    /// ```text
    /// $ buck2 uquery "filter(validation$, //buck2/app/...)"
    ///
    /// //buck2/app/buck2_validation:buck2_validation
    /// ```
    /// returns all targets within `//buck2/app` that have a label with a `validation` suffix.
    async fn filter(&self, regex: String, set: QueryValueSet<Env::Target>) -> QueryFuncResult<Env> {
        match set {
            QueryValueSet::TargetSet(targets) => Ok(self
                .implementation
                .filter_target_set(&regex, &targets)?
                .into()),
            QueryValueSet::FileSet(files) => {
                Ok(self.implementation.filter_file_set(&regex, &files)?.into())
            }
        }
    }

    /// Non-transitive inputs.
    ///
    /// For each target in the provided [*target expression*](#target-expression),
    /// returns the files which are an immediate input to the rule function and thus are needed to go through analysis phase (i.e. produce providers).
    ///
    /// You could consider the `inputs()` and `owner()` functions to be inverses of each other.
    ///
    /// For example:
    /// ```text
    /// $ buck2 uquery "inputs(//buck2/dice/...)"
    /// ```
    /// returns the direct inputs for the `//buck2/dice/...` targets.
    async fn inputs(&self, targets: TargetSet<Env::Target>) -> QueryFuncResult<Env> {
        Ok(self.implementation.inputs(&targets)?.into())
    }

    /// Filter targets by rule type.
    ///
    /// Returns a subset of `targets` where the rule type matches the specified `regex`. The specified pattern can be a regular expression.
    ///
    /// For example:
    /// ```text
    /// $ buck2 query "kind('java.*', deps('//foo:bar'))"
    /// ```
    /// This command returns targets matching rule type `java.*` (e.g., `java_library`, `java_binary`) in the transitive dependencies of `//foo:bar`.
    async fn kind(&self, regex: String, targets: TargetSet<Env::Target>) -> QueryFuncResult<Env> {
        Ok(targets.kind(&regex)?.into())
    }

    /// Not implemented.
    ///
    /// This function won't be implemented in the future, because buck2 query core does not support returning both files and targets from a single function.
    ///
    /// In buck1 it returns targets and files referenced by the given attribute in the given targets.
    ///
    /// <FbInternalOnly>
    /// For more context see discussion in T126638795.
    /// </FbInternalOnly>
    async fn labels(&self, attr: String, targets: TargetSet<Env::Target>) -> QueryFuncResult<Env> {
        self.implementation.labels(&attr, &targets)
    }

    /// Targets owning the given file.
    ///
    /// Returns all targets that have a specified file as an input.
    ///
    /// `owner()` and `inputs()` functions are inverses of each other.
    ///
    /// If the specified file has multiple owning targets, a set of targets is returned. If no owner exists, an empty set is returned.
    ///
    /// For example:
    /// ```text
    /// $ buck2 uquery "owner('app/buck2/src/lib.rs')"
    ///
    /// //buck2/app/buck2:buck2-unittest
    /// //buck2/app/buck2:buck2
    /// ```
    async fn owner(&self, env: &Env, files: FileSet) -> QueryFuncResult<Env> {
        Ok(self.implementation.owner(env, &files).await?.into())
    }

    /// Targets in build file.
    ///
    /// For each file in the provided [*file expression*](#file-expression), returns a list of all targets defined there.
    ///
    /// `targets_in_buildfile()` and `buildfile()` functions are inverses of each other.
    ///
    /// For example:
    /// ```text
    /// $ buck2 uquery 'targets_in_buildfile(buildfile(//buck2:buck2))'
    ///
    /// //buck2:buck2
    /// //buck2:buck2_bundle
    /// //buck2:symlinked_buck2_and_tpx
    /// ```
    async fn targets_in_buildfile(&self, env: &Env, files: FileSet) -> QueryFuncResult<Env> {
        Ok(self
            .implementation
            .targets_in_buildfile(env, &files)
            .await?
            .into())
    }
    /// Find the reverse dependencies of the targets in the given target universe.
    ///
    /// The first parameter `universe` defines where to look for reverse dependencies.
    /// The second parameter `targets` is a specific target or target pattern. It specifies the targets to find reverse dependencies for.
    /// The third argument `depth` is an optional integer literal specifying an upper bound on the depth of the search. A value of one (1) specifies that buck query should return only direct dependencies. If the depth parameter is omitted, the search is unbounded.
    /// The fourth argument `captured_expr` is an optional expression that can be used to filter the results.
    ///
    /// The returned values include the nodes from the `targets` argument itself.
    ///
    /// For example following uquery:
    ///
    /// ```text
    /// $ buck2 uquery "rdeps(//buck2/..., //buck2/dice/dice:dice, 1)"
    /// ```
    /// returns all targets under `//buck2/...` that depend on `//buck2/dice/dice:dice`.
    async fn rdeps(
        &self,
        evaluator: &QueryEvaluator<'_, Env>,
        universe: TargetSet<Env::Target>,
        targets: TargetSet<Env::Target>,
        depth: QueryValueDepth,
        captured_expr: Option<CapturedExpr<'_>>,
    ) -> QueryFuncResult<Env> {
        Ok(self
            .implementation
            .rdeps(
                evaluator.env(),
                evaluator.functions(),
                &universe,
                &targets,
                depth,
                captured_expr.as_ref(),
            )
            .await?
            .into())
    }

    /// Tests of specified targets.
    ///
    /// Returns the test targets associated with the targets from the given [*target expressions*](#target-expression).
    ///
    /// For example:
    /// ```text
    /// $ buck2 uquery "testsof(set(//buck2/dice/dice:dice //buck2/app/buck2:buck2))"
    ///
    /// //buck2/dice/dice:dice-unittest
    /// //buck2/app/buck2:buck2-unittest
    /// ```
    /// returns the tests associated with both `//buck2/dice/dice:dice` and `//buck2/app/buck2:buck2`.
    ///
    /// To obtain all the tests associated with the target and its dependencies,
    /// you can combine the `testsof()` function with the [`deps()`](#deps) function.
    ///
    /// For example:
    /// ```text
    /// $ buck2 uquery "testsof(deps(//buck2/app/buck2:buck2))"
    /// ```
    /// first finds the transitive closure of `//buck2/app/buck2:buck2`,
    /// and then lists all the tests associated with the targets in this transitive closure.
    async fn testsof(&self, env: &Env, targets: TargetSet<Env::Target>) -> QueryFuncResult<Env> {
        Ok(self.implementation.testsof(env, &targets).await?.into())
    }

    // These three functions are intentionally implemented as errors. They are only available within the context
    // of a deps functions 3rd parameter expr. When used in that context, the QueryFunctions will be augmented to
    // have non-erroring implementations.

    /// A filter function that can be used in the query expression of `deps` query function.
    /// Returns the output of deps function for the immediate dependencies of the given targets. Output is equivalent to `deps(<targets>, 1)`.
    ///
    /// See `deps()` for more information on how filter expressions work.
    ///
    /// Example:
    /// `buck2 cquery "deps('//foo:bar', 1, first_order_deps())"` is equivalent to `buck2 cquery "deps('//foo:bar', 1)"`
    async fn first_order_deps(&self) -> QueryFuncResult<Env> {
        Err(QueryError::NotAvailableInContext("first_order_deps"))
    }

    /// A filter function that can be used in the query expression of `deps` query function.
    /// Returns the target dependencies of each dependency of the given targets, excluding any configuration, toolchain and execution dependencies (build time dependencies)
    /// like compiler used as a part of the build.
    ///
    /// Example:
    /// `buck2 cquery "deps('//foo:bar', 1, target_deps())"`
    async fn target_deps(&self) -> QueryFuncResult<Env> {
        Err(QueryError::NotAvailableInContext("target_deps"))
    }

    /// A filter function that can be used in the query expression of `deps` query function.
    /// Returns the output of deps function for execution dependencies (build time dependencies), ex. compiler used as a part of the build.
    ///
    /// Example:
    /// `buck2 cquery "deps('//foo:bar', 1, exec_deps())"`
    async fn exec_deps(&self) -> QueryFuncResult<Env> {
        Err(QueryError::NotAvailableInContext("exec_deps"))
    }

    /// A filter function that can be used in the query expression of `deps` query function.
    /// Returns the output of deps function for configuration dependencies (that appear as conditions in selects).
    ///
    /// Example:
    /// `buck2 cquery "deps('//foo:bar', 1, configuration_deps())"`
    async fn configuration_deps(&self) -> QueryFuncResult<Env> {
        Err(QueryError::NotAvailableInContext("configuration_deps"))
    }

    /// A filter function that can be used in the query expression of `deps` query function.
    /// Returns the output of deps function for toolchain dependencies.
    ///
    /// Example:
    /// `buck2 cquery "deps('//foo:bar', 1, toolchain_deps())"`
    async fn toolchain_deps(&self) -> QueryFuncResult<Env> {
        Err(QueryError::NotAvailableInContext("toolchain_deps"))
    }
    /// Computes the set intersection over the given arguments.
    /// Can be used with the `^` symbol. This operator is commutative.
    ///
    /// The parser treats this operator as left-associative and of equal precedence, so we recommend
    /// that you use parentheses if you need to ensure a specific order of evaluation. A parenthesized expression
    /// resolves to the value of the expression it encloses.
    ///
    /// Example:
    /// `buck2 aquery "deps('//foo:bar') intersect deps('//baz:lib')"` is the same as
    /// `buck2 aquery "deps('//foo:bar') ^ deps('//baz:lib')"`
    /// Both return the targets that appear in the transitive closure of `//foo:bar` and `//baz:lib`.
    #[binary_op(BinaryOp::Intersect)]
    async fn intersect(
        &self,
        env: &Env,
        left: QueryValue<Env::Target>,
        right: QueryValue<Env::Target>,
    ) -> Result<QueryValue<Env::Target>, QueryError> {
        self.implementation.intersect(env, left, right).await
    }

    /// Computes the arguments that are in argument A but not in argument B.
    /// Can be used with the `-` symbol. This operator is NOT commutative.
    ///
    /// The parser treats this operator as left-associative and of equal precedence, so we recommend
    /// that you use parentheses if you need to ensure a specific order of evaluation. A parenthesized expression
    /// resolves to the value of the expression it encloses.
    ///
    /// Example:
    /// `buck2 aquery "deps('//foo:bar') except deps('//baz:lib')"` is the same as
    /// `buck2 aquery "deps('//foo:bar') - deps('//baz:lib')"`
    /// Both return the targets that `//foo:bar` depends on and that `//baz:lib` does NOT depend on.
    #[binary_op(BinaryOp::Except)]
    async fn except(
        &self,
        env: &Env,
        left: QueryValue<Env::Target>,
        right: QueryValue<Env::Target>,
    ) -> Result<QueryValue<Env::Target>, QueryError> {
        self.implementation.except(env, left, right).await
    }

    /// Computes the set union over the given arguments.
    /// Can be used with the `+` symbol. This operator is commutative.
    ///
    /// The parser treats all this operator as left-associative and of equal precedence, so we recommend
    /// that you use parentheses if you need to ensure a specific order of evaluation. A parenthesized expression
    /// resolves to the value of the expression it encloses.
    ///
    /// Example:
    /// `buck2 aquery "deps('//foo:bar') union deps('//baz:lib')"` is the same as
    /// `buck2 aquery "deps('//foo:bar') + deps('//baz:lib')"`
    /// Both return the aggregation of the targets that `//foo:bar` and `//baz:lib` depend on.
    #[binary_op(BinaryOp::Union)]
    async fn union(
        &self,
        env: &Env,
        left: QueryValue<Env::Target>,
        right: QueryValue<Env::Target>,
    ) -> Result<QueryValue<Env::Target>, QueryError> {
        self.implementation.union(env, left, right).await
    }
}

#[derive(Allocative)]
#[allocative(bound = "")]
pub struct DefaultQueryFunctions<Env: QueryEnvironment> {
    _marker: std::marker::PhantomData<Env>,
}

impl<Env: QueryEnvironment> DefaultQueryFunctions<Env> {
    pub fn new() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

impl<Env: QueryEnvironment> DefaultQueryFunctions<Env> {
    pub async fn allpaths(
        &self,
        env: &Env,
        functions: &dyn QueryFunctions<Env = Env>,
        from: &TargetSet<Env::Target>,
        to: &TargetSet<Env::Target>,
        captured_expr: Option<&CapturedExpr<'_>>,
    ) -> Result<TargetSet<Env::Target>, QueryError> {
        Ok(DepsFunction::<Env> {
            _marker: PhantomData,
        }
        .invoke_allpaths(env, functions, from, to, captured_expr)
        .await?)
    }

    pub async fn somepath(
        &self,
        env: &Env,
        functions: &dyn QueryFunctions<Env = Env>,
        from: &TargetSet<Env::Target>,
        to: &TargetSet<Env::Target>,
        captured_expr: Option<&CapturedExpr<'_>>,
    ) -> Result<TargetSet<Env::Target>, QueryError> {
        Ok(DepsFunction::<Env> {
            _marker: PhantomData,
        }
        .invoke_somepath(env, functions, from, to, captured_expr)
        .await?)
    }

    pub fn attrfilter(
        &self,
        attr: &str,
        value: &str,
        targets: &TargetSet<Env::Target>,
    ) -> buck2_error::Result<TargetSet<Env::Target>> {
        targets.attrfilter(attr, &|v| Ok(v == value))
    }

    pub fn nattrfilter(
        &self,
        attr: &str,
        value: &str,
        targets: &TargetSet<Env::Target>,
    ) -> buck2_error::Result<TargetSet<Env::Target>> {
        targets.nattrfilter(attr, &|v| Ok(v == value))
    }

    pub fn attrregexfilter(
        &self,
        attr: &str,
        value: &str,
        targets: &TargetSet<Env::Target>,
    ) -> buck2_error::Result<TargetSet<Env::Target>> {
        targets.attrregexfilter(attr, value)
    }

    pub fn buildfile(&self, targets: &TargetSet<Env::Target>) -> buck2_error::Result<FileSet> {
        targets.buildfile()
    }

    pub async fn allbuildfiles(
        &self,
        env: &Env,
        universe: &TargetSet<Env::Target>,
    ) -> buck2_error::Result<FileSet> {
        env.allbuildfiles(universe).await
    }

    pub async fn rbuildfiles(
        &self,
        env: &Env,
        universe: &FileSet,
        argset: &FileSet,
    ) -> buck2_error::Result<FileSet> {
        env.rbuildfiles(universe, argset).await
    }

    pub async fn deps(
        &self,
        env: &Env,
        functions: &dyn QueryFunctions<Env = Env>,
        targets: &TargetSet<Env::Target>,
        depth: QueryValueDepth,
        captured_expr: Option<&CapturedExpr<'_>>,
    ) -> buck2_error::Result<TargetSet<Env::Target>> {
        DepsFunction::<Env> {
            _marker: PhantomData,
        }
        .invoke_deps(env, functions, targets, depth, captured_expr)
        .await
    }

    /// Filter targets by fully qualified name using regex partial match.
    pub fn filter_target_set(
        &self,
        regex: &str,
        targets: &TargetSet<Env::Target>,
    ) -> buck2_error::Result<TargetSet<Env::Target>> {
        targets.filter_name(regex)
    }

    pub fn filter_file_set(&self, regex: &str, files: &FileSet) -> buck2_error::Result<FileSet> {
        files.filter_name(regex)
    }

    pub fn inputs(&self, targets: &TargetSet<Env::Target>) -> buck2_error::Result<FileSet> {
        targets.inputs()
    }

    pub fn labels(
        &self,
        _attr: &str,
        _targets: &TargetSet<Env::Target>,
    ) -> Result<QueryValue<Env::Target>, QueryError> {
        Err(QueryError::FunctionUnimplemented("labels"))
    }

    pub async fn owner(
        &self,
        env: &Env,
        files: &FileSet,
    ) -> buck2_error::Result<TargetSet<Env::Target>> {
        env.owner(files).await
    }

    pub async fn targets_in_buildfile(
        &self,
        env: &Env,
        paths: &FileSet,
    ) -> buck2_error::Result<TargetSet<Env::Target>> {
        env.targets_in_buildfile(paths).await
    }

    pub async fn rdeps(
        &self,
        env: &Env,
        functions: &dyn QueryFunctions<Env = Env>,
        universe: &TargetSet<Env::Target>,
        targets: &TargetSet<Env::Target>,
        depth: QueryValueDepth,
        captured_expr: Option<&CapturedExpr<'_>>,
    ) -> buck2_error::Result<TargetSet<Env::Target>> {
        DepsFunction::<Env> {
            _marker: PhantomData,
        }
        .invoke_rdeps(env, functions, universe, targets, depth, captured_expr)
        .await
    }

    pub async fn testsof(
        &self,
        env: &Env,
        targets: &TargetSet<Env::Target>,
    ) -> buck2_error::Result<TargetSet<Env::Target>> {
        env.testsof(targets).await
    }

    pub async fn testsof_with_default_target_platform(
        &self,
        env: &Env,
        targets: &TargetSet<Env::Target>,
    ) -> buck2_error::Result<Vec<MaybeCompatible<Env::Target>>> {
        env.testsof_with_default_target_platform(targets).await
    }

    pub async fn intersect(
        &self,
        env: &Env,
        left: QueryValue<Env::Target>,
        right: QueryValue<Env::Target>,
    ) -> Result<QueryValue<Env::Target>, QueryError> {
        self.apply_set_op(
            env,
            left,
            right,
            |l, r| l.intersect(r),
            |l, r| l.intersect(r),
        )
        .await
    }

    pub async fn except(
        &self,
        env: &Env,
        left: QueryValue<Env::Target>,
        right: QueryValue<Env::Target>,
    ) -> Result<QueryValue<Env::Target>, QueryError> {
        self.apply_set_op(
            env,
            left,
            right,
            |l, r| l.difference(r),
            |l, r| l.difference(r),
        )
        .await
    }

    pub async fn union(
        &self,
        env: &Env,
        left: QueryValue<Env::Target>,
        right: QueryValue<Env::Target>,
    ) -> Result<QueryValue<Env::Target>, QueryError> {
        // Special-case String + String to match buck1 behavior:
        // treat both strings as target literals in a single evaluation.
        if let (QueryValue::String(l), QueryValue::String(r)) = (&left, &right) {
            return Ok(QueryValue::TargetSet(env.eval_literals(&[l, r]).await?));
        }

        self.apply_set_op(
            env,
            left,
            right,
            |l, r| Ok(l.union(r)),
            |l, r| Ok(l.union(r)),
        )
        .await
    }

    async fn apply_set_op<Ft, Ff>(
        &self,
        env: &Env,
        left: QueryValue<Env::Target>,
        right: QueryValue<Env::Target>,
        target_op: Ft,
        file_op: Ff,
    ) -> Result<QueryValue<Env::Target>, QueryError>
    where
        Ft: Fn(
            &TargetSet<Env::Target>,
            &TargetSet<Env::Target>,
        ) -> buck2_error::Result<TargetSet<Env::Target>>,
        Ff: Fn(&FileSet, &FileSet) -> buck2_error::Result<FileSet>,
    {
        match (left, right) {
            (QueryValue::TargetSet(l), QueryValue::TargetSet(r)) => {
                Ok(QueryValue::TargetSet(target_op(&l, &r)?))
            }
            (QueryValue::String(l), QueryValue::TargetSet(r)) => {
                let l = env.eval_literals(&[&l]).await?;
                Ok(QueryValue::TargetSet(target_op(&l, &r)?))
            }
            (QueryValue::TargetSet(l), QueryValue::String(r)) => {
                let r = env.eval_literals(&[&r]).await?;
                Ok(QueryValue::TargetSet(target_op(&l, &r)?))
            }
            (QueryValue::FileSet(l), QueryValue::FileSet(r)) => {
                Ok(QueryValue::FileSet(file_op(&l, &r)?))
            }
            (QueryValue::String(l), QueryValue::FileSet(r)) => {
                let l = env.eval_file_literal(&l).await?;
                Ok(QueryValue::FileSet(file_op(&l, &r)?))
            }
            (QueryValue::FileSet(l), QueryValue::String(r)) => {
                let r = env.eval_file_literal(&r).await?;
                Ok(QueryValue::FileSet(file_op(&l, &r)?))
            }
            (QueryValue::String(l), QueryValue::String(r)) => {
                // For union we want to treat both as target literals in one call,
                // but intersect/except expect two sets; we evaluate separately.
                let l_targets = env.eval_literals(&[&l]).await?;
                let r_targets = env.eval_literals(&[&r]).await?;
                Ok(QueryValue::TargetSet(target_op(&l_targets, &r_targets)?))
            }
            (left, right) => Err(QueryError::SetIncompatibleTypes(
                left.variant_name(),
                right.variant_name(),
            )),
        }
    }
}

pub struct AugmentedQueryFunctions<'a, Env: QueryEnvironment> {
    inner: &'a dyn QueryFunctions<Env = Env>,
    extra: Box<dyn QueryFunctions<Env = Env> + 'a>,
}

impl<'a, Env: QueryEnvironment> Debug for AugmentedQueryFunctions<'a, Env> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AugmentedQueryFunctions")
            .finish_non_exhaustive()
    }
}

impl<'a, Env: QueryEnvironment> AugmentedQueryFunctions<'a, Env> {
    pub fn augment(
        inner: &'a dyn QueryFunctions<Env = Env>,
        extra: Box<dyn QueryFunctions<Env = Env> + 'a>,
    ) -> Self {
        Self { inner, extra }
    }
}

impl<'a, Env: QueryEnvironment> QueryFunctions for AugmentedQueryFunctions<'a, Env> {
    type Env = Env;
    fn get(&self, name: &str) -> Option<&dyn QueryFunction<Env>> {
        match self.extra.get(name) {
            None => self.inner.get(name),
            Some(v) => Some(v),
        }
    }

    fn get_op(&self, op: BinaryOp) -> Option<&dyn QueryBinaryOp<Env>> {
        match self.extra.get_op(op) {
            None => self.inner.get_op(op),
            Some(v) => Some(v),
        }
    }
}
