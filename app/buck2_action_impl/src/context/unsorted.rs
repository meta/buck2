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
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkArtifactView;
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
use starlark::values::Absent;
use starlark::values::EmptyList;
use starlark::values::Heap;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::none::NoneOr;
use starlark::values::none::NoneType;
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
        #[starlark(require = pos, default = NoneType)] filename: NoneOr<&str>,
        #[starlark(require = named, default = false)] dir: bool,
        #[starlark(require = named, default = true)] has_content_based_path: bool,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ValueOf<'v, StarlarkArtifactView<'v>>> {
        // We take either one or two positional arguments, namely (filename) or (prefix, filename).
        // The prefix argument is optional, but first, so we pretend the filename is optional
        // and fix them up here.
        let (prefix, filename) = match filename.into_option() {
            None => (None, prefix),
            Some(filename) => (Some(prefix), filename),
        };

        let output_type = if dir {
            OutputType::Directory
        } else {
            OutputType::FileOrDirectory
        };
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

        Ok(eval.heap().alloc(StarlarkDeclaredArtifact::new(
            eval.call_stack_top_location(),
            artifact,
            AssociatedArtifacts::new(),
        )))
    }

    /// Creates a new transitive set. For details, see https://buck2.build/docs/rule_authors/transitive_sets/.
    fn tset<'v>(
        this: &AnalysisActions<'v>,
        #[starlark(require = pos)] definition: TransitiveSetDefinitionView<'v>,
        #[starlark(require = named, default = Absent)] value: Option<Value<'v>>,
        #[starlark(require = named, default = EmptyList)] children: ValueOf<
            'v,
            StarlarkIter<Value<'v>>,
        >,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ValueTyped<'v, TransitiveSet<'v>>> {
        let definition = definition.frozen()?;
        let mut this = this.state()?;
        this.create_transitive_set(definition, value, Some(children.to_value()), eval)
    }

    /// Create a unique tag for associating inputs and outputs of an action.
    ///
    /// Tags are mostly used for dep files: wrap the dep file and the inputs it covers with
    /// `tag.tag_artifacts()`, then pass the tag to `ctx.actions.run(dep_files = ...)`. See
    /// [Dep Files](https://buck2.build/docs/rule_authors/dep_files/) for an example and the
    /// format.
    fn artifact_tag<'v>(
        this: &AnalysisActions<'v>,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueOf<'v, ArtifactTag>> {
        Ok(ValueOf::alloc(
            heap,
            this.state()?.actions.next_artifact_tag(),
        ))
    }

    /// Obtain this daemon's digest configuration. This allows rules to discover what digests the
    /// daemon may be able to e.g. defer download because they conform to its RE backend's expected
    /// digest format.
    fn digest_config<'v>(
        this: &AnalysisActions<'v>,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueOf<'v, StarlarkDigestConfig>> {
        Ok(ValueOf::alloc(
            heap,
            StarlarkDigestConfig {
                digest_config: this.digest_config,
            },
        ))
    }
}
