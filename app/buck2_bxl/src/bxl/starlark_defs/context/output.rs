/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::RefCell;
use std::io::Write;
use std::iter;
use std::ops::Deref;
use std::sync::Arc;
use std::sync::MappedMutexGuard;
use std::sync::Mutex;
use std::sync::MutexGuard;

use allocative::Allocative;
use buck2_artifact::artifact::artifact_path::ArtifactPath;
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_api::bxl::build_result::BxlBuildResult;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact::StarlarkArtifact;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineArgLike;
use buck2_build_api::interpreter::rule_defs::cmd_args::SimpleCommandLineArtifactVisitor;
use buck2_build_api::interpreter::rule_defs::cmd_args::StarlarkCommandLineInputs;
use buck2_build_api::interpreter::rule_defs::cmd_args::value_as::ValueAsCommandLineLike;
use buck2_common::events::HasEvents;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::project::ProjectRoot;
use buck2_error::BuckErrorContext;
use buck2_error::buck2_error;
use buck2_error::starlark_error::from_starlark_with_options;
use buck2_hash::BuckMutInsertionSet;
use buck2_server_ctx::bxl::BxlStreamingTracker;
use buck2_server_ctx::bxl::GetBxlStreamingTracker;
use derivative::Derivative;
use derive_more::Display;
use dupe::Dupe;
use either::Either;
use futures::FutureExt;
use itertools::Itertools;
use serde::Serialize;
use serde::Serializer;
use serde::ser::SerializeSeq;
use starlark::any::ProvidesStaticType;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::AllocValue;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::dict::DictRef;
use starlark::values::dict::DictType;
use starlark::values::list::ListRef;
use starlark::values::list::ListType;
use starlark::values::none::NoneType;
use starlark::values::record::Record;
use starlark::values::set::SetRef;
use starlark::values::starlark_value;
use starlark::values::structs::StructRef;
use starlark::values::tuple::TupleRef;
use starlark::values::tuple::TupleType;
use starlark::values::type_repr::StarlarkTypeRepr;

use crate::bxl::starlark_defs::artifacts::ArtifactArg;
use crate::bxl::starlark_defs::artifacts::EnsuredArtifact;
use crate::bxl::starlark_defs::artifacts::EnsuredArtifactGroup;
use crate::bxl::starlark_defs::build_result::StarlarkBxlBuildResult;
use crate::bxl::starlark_defs::context::build::StarlarkProvidersArtifactIterable;
use crate::bxl::starlark_defs::context::starlark_async::BxlDiceComputations;
use crate::bxl::starlark_defs::eval_extra::BxlEvalExtra;
use crate::bxl::streaming_output_writer::StreamingOutputWriter;

/// Represents the internal state of an output stream, including collected artifacts,
/// standard output buffers, and error buffers.
#[derive(Default, Derivative, Display, Allocative)]
#[display("{:?}", self)]
#[derivative(Debug)]
struct OutputStreamStateInner {
    /// The output string bytes where all non-streaming outputs are written to.
    artifacts_to_ensure: BuckMutInsertionSet<EnsuredArtifactOrGroup>,
    /// The output string bytes where all non-streaming outputs are written to.
    output: Vec<u8>,
    /// The output streaming string bytes where all streaming outputs that not waiting on anything are written to.
    streaming: Vec<u8>,
    /// The error string bytes where all errors are written to.
    pub(crate) error: Vec<u8>,
    /// Pairs of artifacts set and their corresponding streaming output string.
    /// Used to track outputs that are waiting for their associated artifacts to be materialized.
    pending_streaming_outputs: Vec<(BuckMutInsertionSet<EnsuredArtifactOrGroup>, Vec<u8>)>,
}

#[derive(Derivative, Display, Allocative, Dupe, Clone)]
#[display("{:?}", self)]
#[derivative(Debug)]
pub(crate) struct OutputStreamState {
    /// Wrapped in Arc<Mutex<Option<...>>> to allow
    /// - Shared ownership (Arc), we also need to hold it in `BxlEvalExtra`
    /// - Runtime borrow checking for innter mutability (RefCell)
    /// - Optional state for take operations (Option)
    ///
    /// FIXME(JakobDegen): This is completely disgusting, we should not store this here and keep it
    /// only in the `BxlEvalExtra`. Not super easy to make happen though
    inner: Arc<Mutex<Option<OutputStreamStateInner>>>,
}

/// Final result container for output stream processing.
/// This structure is used to construct the final BxlResult.
#[derive(Derivative, Display, Allocative, Clone)]
#[display("{:?}", self)]
#[derivative(Debug)]
pub(crate) struct OutputStreamOutcome {
    /// set of artifacts that need to be materialized, flattened from
    /// the original EnsuredArtifactOrGroup entries.
    pub(crate) ensured_artifacts: BuckMutInsertionSet<ArtifactGroup>,
    pub(crate) output: Vec<u8>,
    pub(crate) streaming: Vec<u8>,
    pub(crate) error: Vec<u8>,
    pub(crate) pending_streaming_outputs: Vec<(BuckMutInsertionSet<ArtifactGroup>, Vec<u8>)>,
}

#[derive(
    ProvidesStaticType,
    Derivative,
    Display,
    Trace,
    NoSerialize,
    Allocative
)]
#[display("{:?}", self)]
#[derivative(Debug)]
pub(crate) struct StarlarkOutputStream {
    state: OutputStreamState,
    #[derivative(Debug = "ignore")]
    pub(crate) project_fs: ProjectRoot,
    #[derivative(Debug = "ignore")]
    pub(crate) artifact_fs: ArtifactFs,
}

/// We can ensure either an `Artifact` or an `ArtifactGroup`. When we want to ensure a `CommandLineArgLike` object,
/// the result of visiting its artifacts is a list of `ArtifactGroup`s. It's convenient to preserve the group rather
/// than extract the individual `Artifact`s from it, for perf/memory optimizations.
#[derive(Display, Debug, Allocative, Hash, Eq, PartialEq, Clone, Dupe)]
pub(crate) enum EnsuredArtifactOrGroup {
    Artifact(EnsuredArtifact),
    ArtifactGroup(ArtifactGroup),
}

impl EnsuredArtifactOrGroup {
    pub(crate) fn into_artifact_groups(self) -> buck2_error::Result<Vec<ArtifactGroup>> {
        match self {
            EnsuredArtifactOrGroup::Artifact(artifact) => {
                let as_artifact = artifact.as_artifact();
                let bound_artifact = as_artifact.get_bound_artifact()?;
                let associated_artifacts = as_artifact.get_associated_artifacts();

                Ok(associated_artifacts
                    .iter()
                    .flat_map(|v| v.iter())
                    .cloned()
                    .chain(iter::once(ArtifactGroup::Artifact(bound_artifact)))
                    .collect::<Vec<_>>())
            }
            EnsuredArtifactOrGroup::ArtifactGroup(group) => Ok(vec![group]),
        }
    }
}

impl Deref for StarlarkOutputStream {
    type Target = OutputStreamState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl OutputStreamState {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Some(OutputStreamStateInner::default()))),
        }
    }

    pub(crate) fn take_state(&self) -> buck2_error::Result<OutputStreamOutcome> {
        let state = self.inner.try_lock().unwrap().take().unwrap();
        let artifacts = state
            .artifacts_to_ensure
            .into_iter()
            .map(EnsuredArtifactOrGroup::into_artifact_groups)
            .flatten_ok()
            .collect::<buck2_error::Result<BuckMutInsertionSet<ArtifactGroup>>>()?;
        let pending_streaming_outputs = state
            .pending_streaming_outputs
            .into_iter()
            .map(|(ensured_artifact_set, output_str)| {
                let artifacts = ensured_artifact_set
                    .into_iter()
                    .map(|ensured_artifact| ensured_artifact.into_artifact_groups())
                    .flatten_ok()
                    .collect::<buck2_error::Result<BuckMutInsertionSet<ArtifactGroup>>>()?;
                Ok((artifacts, output_str))
            })
            .collect::<buck2_error::Result<Vec<(BuckMutInsertionSet<ArtifactGroup>, Vec<u8>)>>>()?;
        Ok(OutputStreamOutcome {
            ensured_artifacts: artifacts,
            output: state.output,
            streaming: state.streaming,
            error: state.error,
            pending_streaming_outputs,
        })
    }

    fn populate_ensured_artifacts(
        &self,
        ensured: EnsuredArtifactOrGroup,
    ) -> buck2_error::Result<()> {
        self.inner
            .lock()
            .unwrap()
            .as_mut()
            .expect("should not have been taken")
            .artifacts_to_ensure
            .insert(ensured);
        Ok(())
    }

    fn output(&self) -> MappedMutexGuard<'_, impl Write + use<>> {
        MutexGuard::map(self.inner.try_lock().unwrap(), |inner| {
            &mut inner.as_mut().expect("should not have been taken").output
        })
    }

    pub(crate) fn error(&self) -> MappedMutexGuard<'_, impl Write + use<>> {
        MutexGuard::map(self.inner.try_lock().unwrap(), |inner| {
            &mut inner.as_mut().expect("should not have been taken").error
        })
    }

    pub(crate) fn streaming(&self) -> MappedMutexGuard<'_, impl Write + use<>> {
        MutexGuard::map(self.inner.try_lock().unwrap(), |inner| {
            &mut inner
                .as_mut()
                .expect("should not have been taken")
                .streaming
        })
    }

    fn pending_streaming_outputs(
        &self,
    ) -> MappedMutexGuard<'_, Vec<(BuckMutInsertionSet<EnsuredArtifactOrGroup>, Vec<u8>)>> {
        MutexGuard::map(self.inner.try_lock().unwrap(), |inner| {
            &mut inner
                .as_mut()
                .expect("should not have been taken")
                .pending_streaming_outputs
        })
    }
}

struct BufferPrintOutput<'a, T: Write> {
    output: MappedMutexGuard<'a, T>,
}

impl<'a, T: Write> Write for BufferPrintOutput<'a, T> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.output.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct StreamingOutput {
    wait_on_artifacts: BuckMutInsertionSet<EnsuredArtifactOrGroup>,
    output_stream_state: OutputStreamState,
    streaming_writer: BxlStreamingWriter,
    buffer: Vec<u8>,
}

impl StreamingOutput {
    fn new(
        wait_on: impl Iterator<Item = EnsuredArtifactOrGroup>,
        output_stream_state: OutputStreamState,
        streaming_writer: BxlStreamingWriter,
    ) -> Self {
        Self {
            wait_on_artifacts: wait_on.collect(),
            output_stream_state,
            buffer: Vec::new(),
            streaming_writer,
        }
    }

    fn insert_pending_streaming_output(&mut self, output: Vec<u8>) {
        self.output_stream_state
            .pending_streaming_outputs()
            .push((self.wait_on_artifacts.clone(), output));
    }
}

impl Write for StreamingOutput {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buffer.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if !self.wait_on_artifacts.is_empty() {
            // We record the buffer as the pending message for the artifacts that are waiting to be materialized
            self.insert_pending_streaming_output(self.buffer.clone());
        } else {
            // Streaming output the output through partial result dispatcher
            self.streaming_writer.write_all(&self.buffer)?;

            // write the buffer to the streaming sink which will be use as the cache
            self.output_stream_state
                .streaming()
                .write_all(&self.buffer)?;

            self.streaming_writer.flush()?;
        }
        self.buffer.clear();
        Ok(())
    }
}

struct BxlStreamingWriter {
    output: StreamingOutputWriter,
    streaming_tracker: Arc<BxlStreamingTracker>,
}

impl BxlStreamingWriter {
    fn new(dice: &BxlDiceComputations) -> Self {
        let dispatcher = dice.per_transaction_data().get_dispatcher().dupe();
        let streaming_tracker = dice
            .per_transaction_data()
            .get_bxl_streaming_tracker()
            .expect("BxlStreamingTracker should be set");
        Self {
            output: StreamingOutputWriter::new(dispatcher),
            streaming_tracker,
        }
    }
}

impl Write for BxlStreamingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let len = self.output.write(buf)?;
        self.streaming_tracker.mark_as_called();

        Ok(len)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.output.flush()
    }
}

impl StarlarkOutputStream {
    pub(crate) fn new(
        project_fs: ProjectRoot,
        artifact_fs: ArtifactFs,
        stream_state: OutputStreamState,
    ) -> Self {
        Self {
            state: stream_state,
            project_fs,
            artifact_fs,
        }
    }

    fn print<'v>(
        &self,
        args: impl Iterator<Item = Value<'v>>,
        sep: &'v str,
        eval: &mut Evaluator<'v, '_, '_>,
        mut output: impl Write,
    ) -> buck2_error::Result<()> {
        let mut first = true;

        fn write_item(
            output: &mut impl Write,
            sep: &str,
            first: &mut bool,
            item: &dyn std::fmt::Display,
        ) -> buck2_error::Result<()> {
            if !*first {
                write!(output, "{sep}{item}")?;
            } else {
                write!(output, "{item}")?;
                *first = false;
            }
            Ok(())
        }

        for arg in args {
            if let Some(ensured) = <&EnsuredArtifact>::unpack_value(arg)? {
                let path = get_artifact_path_display(
                    ensured.get_artifact_path(),
                    ensured.abs(),
                    &self.project_fs,
                    &self.artifact_fs,
                )?;
                write_item(&mut output, sep, &mut first, &path)?;
            } else if let Some(ensured) = <&EnsuredArtifactGroup>::unpack_value(arg)? {
                BxlEvalExtra::from_context(eval)?.dice.via(|dice| {
                    ensured
                        .visit_artifact_path_without_associated_deduped(
                            |artifact_path, abs| {
                                let path = get_artifact_path_display(
                                    artifact_path,
                                    abs,
                                    &self.project_fs,
                                    &self.artifact_fs,
                                )?;
                                write_item(&mut output, sep, &mut first, &path)
                            },
                            dice,
                        )
                        .boxed_local()
                })?;
            } else {
                write_item(&mut output, sep, &mut first, &arg.to_str())?;
            }
        }

        writeln!(output)?;

        output.flush()?;

        Ok(())
    }

    fn print_json<'v>(
        &self,
        value: Value<'v>,
        pretty: bool,
        eval: &mut Evaluator<'v, '_, '_>,
        mut output: impl Write,
    ) -> buck2_error::Result<()> {
        /// A wrapper with a Serialize instance so we can pass down the necessary context.
        struct SerializeValue<'a, 'v, 'd, 's> {
            value: Value<'v>,
            artifact_fs: &'a ArtifactFs,
            project_fs: &'a ProjectRoot,
            async_ctx: &'a RefCell<&'s mut BxlDiceComputations<'d>>,
        }

        impl<'v> SerializeValue<'_, 'v, '_, '_> {
            fn with_value(&self, x: Value<'v>) -> Self {
                Self {
                    value: x,
                    artifact_fs: self.artifact_fs,
                    project_fs: self.project_fs,
                    async_ctx: self.async_ctx,
                }
            }
        }

        impl Serialize for SerializeValue<'_, '_, '_, '_> {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                if let Some(ensured) = <&EnsuredArtifact>::unpack_value(self.value)
                    .map_err(|e| serde::ser::Error::custom(format!("{e:#}")))?
                {
                    let path = get_artifact_path_display(
                        ensured.get_artifact_path(),
                        ensured.abs(),
                        self.project_fs,
                        self.artifact_fs,
                    )
                    .map_err(|err| serde::ser::Error::custom(format!("{err:#}")))?;
                    serializer.serialize_str(&path)
                } else if let Some(ensured) = <&EnsuredArtifactGroup>::unpack_value(self.value)
                    .map_err(|e| serde::ser::Error::custom(format!("{e:#}")))?
                {
                    let mut seq_ser = serializer.serialize_seq(None)?;

                    self.async_ctx
                        .borrow_mut()
                        .via(|dice| {
                            ensured
                                .visit_artifact_path_without_associated_deduped(
                                    |artifact_path, abs| {
                                        let path = get_artifact_path_display(
                                            artifact_path,
                                            abs,
                                            self.project_fs,
                                            self.artifact_fs,
                                        )?;
                                        seq_ser.serialize_element(&path).map_err(|err| {
                                            buck2_error!(
                                                buck2_error::ErrorTag::Tier0,
                                                "{}",
                                                format!("{:#}", err)
                                            )
                                        })?;
                                        Ok(())
                                    },
                                    dice,
                                )
                                .boxed_local()
                        })
                        .map_err(|err| serde::ser::Error::custom(format!("{err:#}")))?;
                    seq_ser.end()
                } else if let Some(x) = ListRef::from_value(self.value) {
                    serializer.collect_seq(x.iter().map(|v| self.with_value(v)))
                } else if let Some(x) = TupleRef::from_value(self.value) {
                    serializer.collect_seq(x.iter().map(|v| self.with_value(v)))
                } else if let Some(x) = SetRef::from_value(self.value) {
                    serializer.collect_seq(x.iter().map(|v| self.with_value(v)))
                } else if let Some(x) = DictRef::from_value(self.value) {
                    serializer.collect_map(
                        x.iter()
                            .map(|(k, v)| (self.with_value(k), self.with_value(v))),
                    )
                } else if let Some(x) = StructRef::from_value(self.value) {
                    serializer.collect_map(x.iter().map(|(k, v)| (k, self.with_value(v))))
                } else if let Some(x) = Record::from_value(self.value) {
                    serializer.collect_map(x.iter().map(|(k, v)| (k, self.with_value(v))))
                } else {
                    self.value.serialize(serializer)
                }
            }
        }

        let writer = if pretty {
            serde_json::to_writer_pretty
        } else {
            serde_json::to_writer
        };

        writer(
            &mut output,
            &SerializeValue {
                value,
                artifact_fs: &self.artifact_fs,
                project_fs: &self.project_fs,
                async_ctx: &RefCell::new(&mut BxlEvalExtra::from_context(eval)?.dice),
            },
        )
        .buck_error_context("Error writing to JSON for `write_json`")?;

        writeln!(&mut output)?;
        output.flush()?;

        Ok(())
    }
}

starlark::methods_static!(OUTPUT_STREAM_METHODS = output_stream_methods);

#[starlark_value(
    type = "bxl.OutputStream",
    StarlarkTypeRepr,
    UnpackValue,
    ty_vtable_no_freeze
)]
impl<'v> StarlarkValue<'v> for StarlarkOutputStream {
    fn get_methods() -> Option<&'static Methods> {
        Some(OUTPUT_STREAM_METHODS.methods())
    }
}

impl<'v> AllocValue<'v> for StarlarkOutputStream {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex_no_freeze(self)
    }
}

#[derive(StarlarkTypeRepr, UnpackValue)]
enum EnsureMultipleArtifactsArg<'v> {
    None(NoneType),
    EnsuredArtifactArgs(ValueOf<'v, ListType<ArtifactArg<'v>>>),
    ProvidersArtifactIterable(&'v StarlarkProvidersArtifactIterable<'v>),
    BxlBuildResult(&'v StarlarkBxlBuildResult),
    Dict(ValueOf<'v, DictType<Value<'v>, &'v StarlarkBxlBuildResult>>),
    CmdLine(ValueAsCommandLineLike<'v>),
}

/// The output stream for bxl to print values to the console as their result
#[starlark_module]
fn output_stream_methods(builder: &mut MethodsBuilder) {
    /// Outputs results to the console via stdout. These outputs are considered to be the results
    /// of a bxl script, which will be displayed to stdout by buck2 even when the script is cached.
    /// Accepts an optional separator that defaults to " ".
    ///
    /// Prints that are not result of the bxl should be printed via stderr via the stdlib `print`
    /// and `pprint`.
    ///
    /// **Note** that `ctx.output.print()` is intended for simple outputs. For more complex
    /// outputs, the recommendation would be to write them to a file.
    ///
    /// **Note** that the output of `ctx.output.print()` will be displayed to stdout at the end of bxl.
    /// If you want to print something to stdout immediately or the when ensured artifacts are ready and also want this even when the script is cached,
    /// use `ctx.output.stream()` instead.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_print(ctx):
    ///     ctx.output.print("test")
    /// ```
    fn print<'v>(
        this: &'v StarlarkOutputStream,
        #[starlark(args)] args: ValueOf<'v, TupleType<Value<'v>>>,
        #[starlark(default = " ")] sep: &'v str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<NoneType> {
        let buffer_output = BufferPrintOutput {
            output: this.output(),
        };
        this.print(args.items(), sep, eval, buffer_output)?;

        Ok(NoneType)
    }

    /// Outputs results to the console via stdout as pretty-printed json. Pretty
    /// printing can be turned off by the `pretty` keyword-only parameter.
    /// These outputs are considered to be the results of a bxl script, which will be displayed to
    /// stdout by buck2 even when the script is cached.
    ///
    /// Prints that are not result of the bxl should be printed via stderr via the stdlib `print`
    /// and `pprint`.
    ///
    /// **Note** that the output of `ctx.output.print_json()` will be displayed to stdout at the end of bxl.
    /// If you want to print something to stdout immediately or the when ensured artifacts are ready and also want this even when the script is cached,
    /// use `ctx.output.stream_json()` instead.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_print_json(ctx):
    ///     outputs = {}
    ///     outputs.update({"foo": bar})
    ///     ctx.output.print_json(outputs)
    /// ```
    fn print_json<'v>(
        this: &'v StarlarkOutputStream,
        value: Value<'v>,
        #[starlark(require=named, default=true)] pretty: bool,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<NoneType> {
        let buffer_output = BufferPrintOutput {
            output: this.output(),
        };

        this.print_json(value, pretty, eval, buffer_output)?;

        Ok(NoneType)
    }

    /// Streaming outputs results to the console via stdout **immediately** when it is ready.
    /// It will be displayed to stdout by buck2 even when the script is cached.
    /// Accepts an optional separator that defaults to " ".
    ///
    /// The streaming behavior is controlled by the `wait_on` parameter:
    /// - If 'wait_on' is not specified or empty, output is displayed **immediately** during evaluation
    /// - If 'wait_on' contains artifacts，output is displayed **as soon as all specified artifacts are materialized**
    ///
    /// The `wait_on` parameter explicitly specifies artifacts that must be materialized before showing output.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_print(ctx):
    ///     # Immediate output during evaluation
    ///     ctx.output.stream("Starting process...")
    ///
    ///     # Output as soon as artifact is materialized
    ///     artifact = ctx.output.ensure(my_artifact)
    ///     ctx.output.stream("Artifact ready:", artifact, wait_on=[artifact])
    ///
    ///     # Output when both artifacts are materialized
    ///     artifact1 = ctx.output.ensure(my_artifact1)
    ///     artifact2 = ctx.output.ensure(my_artifact2)
    ///     ctx.output.stream("First artifact:", artifact1, wait_on=[artifact1, artifact2])
    /// ```
    fn stream<'v>(
        this: &'v StarlarkOutputStream,
        #[starlark(args)] args: ValueOf<'v, TupleType<Value<'v>>>,
        #[starlark(default = " ")] sep: &'v str,
        #[starlark(require = named, default = ValueOf::empty_list())] wait_on: ValueOf<
            'v,
            ListType<Either<&'v EnsuredArtifact, &'v EnsuredArtifactGroup<'v>>>,
        >,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<NoneType> {
        let extra = BxlEvalExtra::from_context(eval)?;
        let streaming_writer = BxlStreamingWriter::new(&extra.dice);

        let wait_on = wait_on
            .try_to_vec()?
            .into_iter()
            .flat_map(|wait_on| match wait_on {
                Either::Left(ensured_artifact) => {
                    vec![EnsuredArtifactOrGroup::Artifact(ensured_artifact.dupe())]
                }
                Either::Right(ensured_group) => ensured_group
                    .inner()
                    .iter()
                    .map(|group| EnsuredArtifactOrGroup::ArtifactGroup(group.dupe()))
                    .collect(),
            });

        let streaming_output = StreamingOutput::new(wait_on, this.state.dupe(), streaming_writer);
        this.print(args.items(), sep, eval, streaming_output)?;

        Ok(NoneType)
    }

    /// Streaming outputs results to the console via stdout as pretty-printed json **immediately** when it is ready.
    /// Pretty printing can be turned off by the `pretty` keyword-only parameter.
    /// It will be displayed to stdout by buck2 even when the script is cached.
    ///
    /// The streaming behavior is controlled by the `wait_on` parameter:
    /// - If 'wait_on' is not specified or empty, output is displayed **immediately** during evaluation
    /// - If 'wait_on' contains artifacts，output is displayed **as soon as all specified artifacts are materialized**
    ///
    /// The `wait_on` parameter explicitly specifies artifacts that must be materialized before showing output.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_print_json(ctx):
    ///     outputs = {}
    ///     outputs.update({"foo": bar})
    ///
    ///     # Stream JSON output immediately
    ///     ctx.output.stream_json({"status": "starting"})
    ///
    ///     # Stream JSON when artifact is ready
    ///     artifact = ctx.output.ensure(my_artifact)
    ///     ctx.output.stream_json({"artifact": artifact}, wait_on=[artifact])
    ///
    ///    # Stream JSON waiting on artifact to be ready
    ///    ctx.output.stream_json({"status": "starting"}, wait_on=[artifact])
    /// ```
    fn stream_json<'v>(
        this: &'v StarlarkOutputStream,
        value: Value<'v>,
        #[starlark(require=named, default=true)] pretty: bool,
        #[starlark(require = named, default = ValueOf::empty_list())] wait_on: ValueOf<
            'v,
            ListType<Either<&'v EnsuredArtifact, &'v EnsuredArtifactGroup<'v>>>,
        >,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<NoneType> {
        let extra = BxlEvalExtra::from_context(eval)?;
        let streaming_writer = BxlStreamingWriter::new(&extra.dice);
        let wait_on = wait_on
            .try_to_vec()?
            .into_iter()
            .flat_map(|wait_on| match wait_on {
                Either::Left(ensured_artifact) => {
                    vec![EnsuredArtifactOrGroup::Artifact(ensured_artifact.dupe())]
                }
                Either::Right(ensured_group) => ensured_group
                    .inner()
                    .iter()
                    .map(|group| EnsuredArtifactOrGroup::ArtifactGroup(group.dupe()))
                    .collect(),
            });
        let streaming_output = StreamingOutput::new(wait_on, this.state.dupe(), streaming_writer);

        this.print_json(value, pretty, eval, streaming_output)?;

        Ok(NoneType)
    }

    /// Marks the artifact as an artifact that should be available to the users at the end of
    /// the bxl invocation. Any artifacts that do not get registered via this call is not
    /// accessible by users at the end of bxl script.
    ///
    /// This function returns an `ensured_artifact` type that can be printed via `ctx.output.print()`
    /// to print its actual path on disk.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_ensure(ctx):
    ///     actions = ctx.bxl_actions().actions
    ///     output = actions.write("my_output", "my_content")
    ///     ensured = ctx.output.ensure(output)
    ///     ctx.output.print(ensured)
    /// ```
    fn ensure<'v>(
        this: &StarlarkOutputStream,
        artifact: ArtifactArg<'v>,
    ) -> starlark::Result<EnsuredArtifact> {
        let artifact = artifact.into_ensured_artifact()?;
        this.populate_ensured_artifacts(EnsuredArtifactOrGroup::Artifact(artifact.clone()))?;

        Ok(artifact)
    }

    /// Same as `ensure`, but for multiple artifacts. Will preserve the shape of the inputs (i.e. if the resulting
    /// `Dict` of a `ctx.build()` is passed in, the output will be a `Dict` where the key is preserved,
    /// and the values are converted to `ensured_artifact`s).
    ///
    /// Note that is slower to loop through objects and ensure them one by one than it is to call `ensure_multiple()`
    /// on all the objects at once (if possible).
    /// So, it is suggested to use this method when you are only ensuring a few individual artifacts that are not stored in an iterable.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_ensure_multiple(ctx):
    ///     outputs = {}
    ///     for target, value in ctx.build(ctx.cli_args.target).items():
    ///     outputs.update({target.raw_target(): ctx.output.ensure_multiple(value.artifacts())})
    ///     ctx.output.print_json(outputs)
    /// ```
    fn ensure_multiple<'v>(
        this: &'v StarlarkOutputStream,
        // TODO(nga): must be either positional or named.
        artifacts: EnsureMultipleArtifactsArg<'v>,
        heap: Heap<'v>,
    ) -> starlark::Result<Value<'v>> {
        match artifacts {
            EnsureMultipleArtifactsArg::None(_) => Ok(heap.alloc(Vec::<EnsuredArtifact>::new())),
            EnsureMultipleArtifactsArg::EnsuredArtifactArgs(list) => {
                let artifacts: Vec<EnsuredArtifact> = list
                    .try_items()
                    .map(|artifact| {
                        let artifact = artifact?.into_ensured_artifact()?;
                        this.populate_ensured_artifacts(EnsuredArtifactOrGroup::Artifact(
                            artifact.clone(),
                        ))?;

                        Ok::<EnsuredArtifact, buck2_error::Error>(artifact)
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                Ok(heap.alloc(artifacts))
            }
            EnsureMultipleArtifactsArg::ProvidersArtifactIterable(artifact_gen) => {
                Ok(heap.alloc(get_artifacts_from_bxl_build_result(
                    artifact_gen
                        .0
                        .downcast_ref::<StarlarkBxlBuildResult>()
                        .unwrap(),
                    this,
                )?))
            }
            EnsureMultipleArtifactsArg::BxlBuildResult(bxl_build_result) => {
                Ok(heap.alloc(get_artifacts_from_bxl_build_result(bxl_build_result, this)?))
            }
            EnsureMultipleArtifactsArg::Dict(build_result_dict) => Ok(heap.alloc_dict(
                build_result_dict
                    .try_items()
                    .map(|entry| {
                        let (label, bxl_build_result) = entry?;
                        Ok((
                            label.get_hashed().map_err(|e| {
                                from_starlark_with_options(
                                    e,
                                    buck2_error::starlark_error::NativeErrorHandling::Unknown,
                                    false,
                                )
                            })?,
                            heap.alloc(get_artifacts_from_bxl_build_result(
                                bxl_build_result,
                                this,
                            )?),
                        ))
                    })
                    .collect::<buck2_error::Result<_>>()?,
            )),
            EnsureMultipleArtifactsArg::CmdLine(cmd_line) => {
                // TODO(nga): we should not be doing that here.
                //   If we pass random string to this function,
                //   it will be interpreted as a command line without inputs,
                //   and this function will return empty `EnsuredArtifactGroup`.
                let inputs = get_cmd_line_inputs(cmd_line.0)?;
                let mut result = Vec::new();

                for artifact_group in &inputs.inputs {
                    this.populate_ensured_artifacts(EnsuredArtifactOrGroup::ArtifactGroup(
                        artifact_group.dupe(),
                    ))?;
                    result.push(artifact_group.dupe());
                }

                Ok(heap.alloc(EnsuredArtifactGroup::new(result, false, heap)))
            }
        }
    }
}

pub(crate) fn get_cmd_line_inputs(
    cmd_line: &dyn CommandLineArgLike,
) -> buck2_error::Result<StarlarkCommandLineInputs> {
    let mut visitor = SimpleCommandLineArtifactVisitor::new();
    cmd_line.visit_artifacts(&mut visitor)?;
    let inputs = StarlarkCommandLineInputs {
        inputs: visitor.inputs,
    };
    Ok(inputs)
}

pub(crate) fn get_artifact_path_display(
    artifact_path: ArtifactPath,
    abs: bool,
    project_fs: &ProjectRoot,
    artifact_fs: &ArtifactFs,
) -> buck2_error::Result<String> {
    // We always use the configuration-based path, since that's what we expose to the user.
    let resolved = artifact_path.resolve_configuration_hash_path(artifact_fs)?;
    Ok(if abs {
        project_fs.resolve(&resolved).to_string()
    } else {
        resolved.to_string()
    })
}

fn get_artifacts_from_bxl_build_result(
    bxl_build_result: &StarlarkBxlBuildResult,
    output_stream: &StarlarkOutputStream,
) -> buck2_error::Result<Vec<EnsuredArtifact>> {
    match &bxl_build_result.0 {
        BxlBuildResult::None => Ok(Vec::new()),
        BxlBuildResult::Built { result, .. } => result
            .outputs
            .iter()
            .filter_map(|built| {
                built.inner.as_ref().ok().map(|artifacts| {
                    artifacts
                        .values
                        .iter()
                        .map(|(artifact, _)| EnsuredArtifact {
                            artifact: StarlarkArtifact::new(artifact.dupe()),
                            abs: false,
                        })
                })
            })
            .flatten()
            .map(|artifact| try {
                output_stream.populate_ensured_artifacts(EnsuredArtifactOrGroup::Artifact(
                    artifact.clone(),
                ))?;
                artifact
            })
            .collect::<buck2_error::Result<_>>(),
    }
}
