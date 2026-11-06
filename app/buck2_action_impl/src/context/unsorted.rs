/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_artifact::artifact::output_type::OutputType;
use buck2_build_api::interpreter::rule_defs::artifact::associated::AssociatedArtifacts;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_declared_artifact::StarlarkDeclaredArtifact;
use buck2_build_api::interpreter::rule_defs::artifact_tagging::ArtifactTag;
use buck2_build_api::interpreter::rule_defs::context::AnalysisActions;
use buck2_build_api::interpreter::rule_defs::digest_config::StarlarkDigestConfig;
use buck2_build_api::interpreter::rule_defs::transitive_set::TransitiveSet;
use buck2_build_api::interpreter::rule_defs::transitive_set::TransitiveSetDefinitionView;
use buck2_core::fs::buck_out_path::BuckOutPathKind;
use starlark::environment::MethodsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::typing::StarlarkIter;

#[starlark_module]
pub(crate) fn analysis_actions_methods_unsorted(builder: &mut MethodsBuilder) {
    /// Returns an unbound `artifact`, representing where a file will go, which must be bound before analysis terminates.
    /// The usual way of binding an artifact is with `ctx.actions.run`. As an example:
    ///
    /// ```python
    /// my_output = ctx.actions.declare_output("output.o")
    /// ctx.actions.run(["gcc", "-c", my_source, "-o", my_output.as_output()], category = "compile")
    /// ```
    ///
    /// This snippet declares an output with the filename `output.o` (it will be located in the output directory
    /// for this target). Note the use of `as_output` to tag this artifact as being an output in
    /// the action. After binding the artifact you can subsequently use `my_output` as either an
    /// input for subsequent actions, or as the result in a provider.
    ///
    /// Artifacts from a single target may not have the same name, so if you then want a second
    /// artifact also named `output.o` you need to supply a prefix, e.g.
    /// `ctx.actions.declare_output("directory", "output.o")`. The artifact will still report having
    /// name `output.o`, but will be located at `directory/output.o`.
    ///
    /// The `dir` argument should be set to `True` if the binding will be a directory.
    fn declare_output<'v>(
        this: &AnalysisActions<'v>,
        #[starlark(require = pos)] prefix: &str,
        #[starlark(require = pos)] filename: Option<&str>,
        #[starlark(require = named, default = false)] dir: bool,
        #[starlark(require = named)] has_content_based_path: Option<bool>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkDeclaredArtifact<'v>> {
        // We take either one or two positional arguments, namely (filename) or (prefix, filename).
        // The prefix argument is optional, but first, so we pretend the filename is optional
        // and fix them up here.
        let (prefix, filename) = match filename {
            None => (None, prefix),
            Some(filename) => (Some(prefix), filename),
        };

        let output_type = if dir {
            OutputType::Directory
        } else {
            OutputType::FileOrDirectory
        };
        let has_content_based_path = has_content_based_path.unwrap_or(true);
        let path_resolution_method = if has_content_based_path {
            BuckOutPathKind::ContentHash
        } else {
            BuckOutPathKind::Configuration
        };
        let artifact = this.state()?.declare_output(
            prefix,
            filename,
            output_type,
            eval.call_stack_top_location(),
            path_resolution_method,
            eval.heap(),
        )?;

        Ok(StarlarkDeclaredArtifact::new(
            eval.call_stack_top_location(),
            artifact,
            AssociatedArtifacts::new(),
        ))
    }

    /// Creates a new transitive set. For details, see https://buck2.build/docs/rule_authors/transitive_sets/.
    fn tset<'v>(
        this: &AnalysisActions<'v>,
        #[starlark(require = pos)] definition: TransitiveSetDefinitionView<'v>,
        #[starlark(require = named)] value: Option<Value<'v>>,
        #[starlark(require = named)] children: Option<ValueOf<'v, StarlarkIter<Value<'v>>>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ValueTyped<'v, TransitiveSet<'v>>> {
        let definition = definition.frozen()?;
        let mut this = this.state()?;
        this.create_transitive_set(definition, value, children.map(|v| v.to_value()), eval)
    }

    /// Create a unique artifact tag for tracking input dependencies with dependency files.
    ///
    /// An `ArtifactTag` is used to associate inputs and outputs in a build action with a dependency
    /// file (depfile), enabling more accurate incremental builds by tracking which inputs were
    /// actually used during action execution.
    ///
    /// ### How Dependency Files Work
    ///
    /// Traditional build systems face a dilemma: if an action depends on 1000 header files but only
    /// uses 10, should changing any of the 1000 trigger a rebuild? Being conservative (always rebuild)
    /// is slow, while being optimistic (never rebuild) risks correctness.
    ///
    /// Dependency files solve this by letting the build tool (e.g., GCC, Clang, Swift compiler) report
    /// which inputs it actually used:
    ///
    /// 1. **First build**: The action runs with all potential inputs available
    /// 2. **Tool generates depfile**: The compiler writes a file listing only the inputs it actually read
    /// 3. **Subsequent builds**: Buck2 only triggers rebuilds when files listed in the depfile change
    ///
    /// ### Usage Pattern
    ///
    /// The typical workflow involves three steps:
    ///
    /// 1. Create a unique tag with `ctx.actions.artifact_tag()`
    /// 2. Use the tag's `tag_artifacts()` method to mark:
    ///    - All potential input dependencies
    ///    - The depfile output
    /// 3. Associate the tag with a label in the `dep_files` parameter of `ctx.actions.run()`
    ///
    /// ### Example: C++ Compilation
    ///
    /// ```python
    /// def _compile_impl(ctx):
    ///     # Step 1: Create a unique tag for tracking header dependencies
    ///     headers_tag = ctx.actions.artifact_tag()
    ///
    ///     # Prepare inputs and outputs
    ///     headers_dir = ctx.actions.copied_dir("headers", {...})
    ///     dep_file = ctx.actions.declare_output("depfile")
    ///     output = ctx.actions.declare_output("output.o")
    ///
    ///     # Step 2: Tag the inputs and depfile output
    ///     cmd = cmd_args([
    ///         "gcc", "-c", "main.cpp",
    ///         "-I", headers_tag.tag_artifacts(headers_dir),  # Mark potential inputs
    ///         "-o", output.as_output(),
    ///         "-MMD",                                        # Tell GCC to generate depfile
    ///         "-MF", headers_tag.tag_artifacts(dep_file.as_output()),  # Mark depfile output
    ///     ])
    ///
    ///     # Step 3: Associate the tag with the "headers" label
    ///     ctx.actions.run(
    ///         cmd,
    ///         category = "cxx_compile",
    ///         dep_files = {"headers": headers_tag}
    ///     )
    /// ```
    ///
    /// In this example:
    /// - `headers_dir` contains 1000 header files
    /// - GCC generates `depfile` listing only the 10 headers actually used
    /// - On subsequent builds, only changes to those 10 headers trigger recompilation
    ///
    ///
    /// ### Depfile Format
    ///
    /// Dependency files use Makefile syntax:
    ///
    /// ```makefile
    /// output.o: main.cpp foo.h bar.h internal.h
    /// ```
    ///
    /// This tells Buck2 that `output.o` depends on these specific files. Buck2 reads this file
    /// after the action completes and uses it to determine which tagged inputs to track for
    /// future incremental builds.
    ///
    /// ### Return Value
    ///
    /// Returns a new `ArtifactTag` instance. Each call creates a unique tag that can be compared
    /// for equality, allowing Buck2 to match tagged inputs with their corresponding depfiles.
    ///
    /// ### See Also
    ///
    /// - [`ArtifactTag.tag_artifacts()`](../ArtifactTag#artifacttagtag_artifacts): Tag both inputs and outputs
    /// - [`ArtifactTag.tag_inputs()`](../ArtifactTag#artifacttagtag_inputs): Tag only inputs
    /// - [`ctx.actions.run()`](../AnalysisActions#analysisactionsrun): The `dep_files` parameter documentation
    fn artifact_tag<'v>(this: &AnalysisActions<'v>) -> starlark::Result<ArtifactTag> {
        Ok(this.state()?.actions.next_artifact_tag())
    }

    /// Obtain this daemon's digest configuration. This allows rules to discover what digests the
    /// daemon may be able to e.g. defer download because they conform to its RE backend's expected
    /// digest format.
    fn digest_config<'v>(this: &AnalysisActions<'v>) -> starlark::Result<StarlarkDigestConfig> {
        Ok(StarlarkDigestConfig {
            digest_config: this.digest_config,
        })
    }
}
