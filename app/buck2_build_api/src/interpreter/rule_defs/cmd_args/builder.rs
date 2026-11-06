/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::hash::Hasher;
use std::ops::Deref;
use std::ops::DerefMut;
use std::sync::Arc;

use buck2_artifact::artifact::artifact_dyn::ArtifactDyn;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::content_hash::ContentBasedPathHash;
use buck2_core::execution_types::executor_config::PathSeparatorKind;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_error::internal_error;
use buck2_execute::artifact::fs::ExecutorFs;
use buck2_fs::paths::RelativePath;
use buck2_interpreter::types::regex::StarlarkBuckRegex;
use buck2_util::strong_hasher::Blake3StrongHasher;
use buck2_util::strong_hasher::StrongHash128;
use dupe::Dupe;
use regex::Regex;
use starlark::values::ValueOf;
use strong_hash::StrongHash;

use crate::interpreter::rule_defs::artifact::starlark_output_artifact::StarlarkOutputArtifactView;
use crate::interpreter::rule_defs::cmd_args::options::CommandLineOptionsRef;
use crate::interpreter::rule_defs::cmd_args::options::OptionsReplacementsRef;
use crate::interpreter::rule_defs::cmd_args::options::QuoteStyle;
use crate::interpreter::rule_defs::cmd_args::options::RelativeOrigin;
use crate::interpreter::rule_defs::cmd_args::regex::CmdArgsRegex;
use crate::interpreter::rule_defs::cmd_args::shlex_quote::shlex_quote;
use crate::interpreter::rule_defs::cmd_args::sink::CommandLineSink;
use crate::interpreter::rule_defs::cmd_args::traits::ArtifactPathMapper;
use crate::interpreter::rule_defs::transitive_set::TransitiveSetOrdering;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum CommandLineOptionsError {
    #[error("Path `{0}` does not have {1} parent(s)")]
    TooManyParentCalls(String, u32),
    #[error("Command line args in this position do not support write-to-file macros")]
    WriteToFileMacroNotSupported,
}

fn path_popn(path: &mut ProjectRelativePathBuf, parents: u32) -> buck2_error::Result<()> {
    let path_str = path.to_string();
    for _ in 0..parents {
        if !path.pop() {
            return Err(CommandLineOptionsError::TooManyParentCalls(path_str, parents).into());
        }
    }
    Ok(())
}

fn unpack_relative_origin<'v>(
    t: ValueOf<'v, RelativeOrigin<'v>>,
    parents: u32,
) -> buck2_error::Result<RelativeToOption> {
    Ok(match t.unpack() {
        RelativeOrigin::OutputArtifact(artifact) => {
            let value = match artifact {
                StarlarkOutputArtifactView::Frozen(value) => value,
                StarlarkOutputArtifactView::Unfrozen(_) => {
                    return Err(internal_error!(
                        "Non-frozen output artifacts can't be added to CLIs"
                    ));
                }
            };
            RelativeToOption::OutputArtifact(value.inner().artifact.dupe(), parents)
        }
        RelativeOrigin::Artifact(artifact) => RelativeToOption::Artifact(
            artifact.as_input_artifact_like().get_bound_artifact()?,
            parents,
        ),
        RelativeOrigin::CellRoot(cell_root) => {
            RelativeToOption::CellPath(cell_root.cell_path(), parents)
        }
        RelativeOrigin::ProjectRoot(_) => {
            let mut path = ProjectRelativePath::empty().to_owned();
            path_popn(&mut path, parents)?;
            RelativeToOption::Path(path)
        }
    })
}

fn finish_relative_to_path(
    o: &RelativeToOption,
    fs: &ArtifactFs,
    artifact_path_mapping: &'_ dyn ArtifactPathMapper,
) -> buck2_error::Result<ProjectRelativePathBuf> {
    match o {
        RelativeToOption::Path(p) => Ok(p.to_owned()),
        RelativeToOption::CellPath(cell_path, parents) => {
            let mut p = fs.resolve_cell_path(*cell_path)?;
            path_popn(&mut p, *parents)?;
            Ok(p)
        }
        RelativeToOption::OutputArtifact(a, parents) => {
            let mut p = a.resolve_path(fs, Some(&ContentBasedPathHash::for_output_artifact()))?;
            path_popn(&mut p, *parents)?;
            Ok(p)
        }
        RelativeToOption::Artifact(a, parents) => {
            // If we have the actual path of the artifact (e.g. because it is an input to the
            // action), then we use that to resolve the relative path normally.
            //
            // Otherwise, we have a trick where we use a unique constant value to resolve the path.
            // This works because it guarantees that we get enough `..` components to walk past the
            // content hashes:
            // - "a/b/hash1/c".relative_to("a/b/hash2/d") => "../../hash1/c"
            // - "a/b/hash1/c".relative_to("a/b/placeholder/d") => "../../hash1/c" Concretely, that
            // means that relative origins are not required to be inputs to actions
            //
            // FIXME(JakobDegen): Just checking the artifact path mapping here is pretty fragile,
            // it's very unclear what the conditions are under which things end up in there so there
            // may be surprise behavior changes. I had tried just doing the `RelativePathResolution`
            // version unconditionally, but that breaks a bunch of genrules which do
            // `srcs.project("a").relative_to(srcs)` and expect that to not have any `..`
            // components. Fixing those genrules might be ok, or doing some other more principled
            // thing (like omitting `..` components only if the artifact on both sides is the same)
            // might be ok too
            let hash = artifact_path_mapping
                .get(a)
                .unwrap_or(&ContentBasedPathHash::RelativePathResolution);
            let mut p = a.resolve_path(fs, Some(hash))?;
            path_popn(&mut p, *parents)?;
            Ok(p)
        }
    }
}

pub(crate) fn compute_relative_to_path<'v>(
    t: ValueOf<'v, RelativeOrigin<'v>>,
    parents: u32,
    fs: &ArtifactFs,
    artifact_path_mapping: &'_ dyn ArtifactPathMapper,
) -> buck2_error::Result<ProjectRelativePathBuf> {
    finish_relative_to_path(
        &unpack_relative_origin(t, parents)?,
        fs,
        artifact_path_mapping,
    )
}

/// A `relative_to`, as the hashing builder hashes it and the rendering builder resolves it:
/// origins stay unresolved, so the identity of what is hashed does not depend on where the
/// project's cells and `buck-out` are.
#[derive(StrongHash)]
enum RelativeToOption {
    // parents already "applied" here
    Path(ProjectRelativePathBuf),
    // A cell path, then `parents` levels up
    CellPath(CellPath, u32),
    // An output artifact at its placeholder path, then `parents` levels up
    OutputArtifact(Artifact, u32),
    // FIXME(JakobDegen): Having the hash of the artifact in here is... acceptable, but really not
    // ideal. It's a bit unstable, and also the risk that the `StrongHash` impl changes in a
    // semantically incompatible way above buck versions is kind of high, there's a lot behind this.
    //
    // u32 is the `parents` count
    Artifact(Artifact, u32),
}

#[derive(StrongHash)]
struct ArtifactOptions<'v> {
    relative_to: Option<Arc<RelativeToOption>>,
    absolute_prefix: Option<&'v str>,
    absolute_suffix: Option<&'v str>,
    parent: u32,
}

const DEFAULT_ARTIFACT_OPTIONS: ArtifactOptions<'_> = ArtifactOptions {
    relative_to: None,
    absolute_prefix: None,
    absolute_suffix: None,
    parent: 0,
};

enum FormatRegex<'v> {
    Starlark(&'v StarlarkBuckRegex),
    Owned(regex::Regex),
}

impl<'v> FormatRegex<'v> {
    fn make(o: OptionsReplacementsRef<'v, '_>) -> Vec<(FormatRegex<'v>, &'v str)> {
        o.iter()
            .map(|(pattern, replacement)| {
                let regex = match pattern {
                    // We checked that regex is valid in replace_regex(), so unwrap is safe.
                    CmdArgsRegex::Str(s) => FormatRegex::Owned(Regex::new(s.as_str()).unwrap()),
                    CmdArgsRegex::Regex(r) => FormatRegex::Starlark(r.as_ref()),
                };
                (regex, replacement.as_str())
            })
            .collect()
    }
}

impl StrongHash for FormatRegex<'_> {
    fn strong_hash<H: std::hash::Hasher>(&self, state: &mut H) {
        #[derive(StrongHash)]
        enum E<'a> {
            StarlarkRegex(&'a str),
            StarlarkFancyRegex(&'a str),
            Regex(&'a str),
        }

        match self {
            FormatRegex::Starlark(StarlarkBuckRegex::Regular(r)) => E::StarlarkRegex(r.as_str()),
            FormatRegex::Starlark(StarlarkBuckRegex::Fancy(r)) => E::StarlarkFancyRegex(r.as_str()),
            FormatRegex::Owned(r) => E::Regex(r.as_str()),
        }
        .strong_hash(state);
    }
}

#[derive(StrongHash)]
enum DelimiterJoinState {
    Unstarted,
    Started(String),
}

impl DelimiterJoinState {
    fn push(&mut self, s: String, delim: &str) {
        match self {
            DelimiterJoinState::Unstarted => {
                *self = DelimiterJoinState::Started(s);
            }
            DelimiterJoinState::Started(cur) => {
                cur.reserve(delim.len() + s.len());
                cur.push_str(delim);
                cur.push_str(s.as_str());
            }
        }
    }

    fn finish(self) -> String {
        match self {
            Self::Unstarted => String::new(),
            Self::Started(s) => s,
        }
    }
}

#[derive(Default, StrongHash)]
struct StringOptions<'v> {
    /// The options are generally immutable, but this is the one exception. The `DelimiterJoinState`
    /// is responsible for accumulating the delimited parts into a single string
    delimiter: Option<(&'v str, DelimiterJoinState)>,
    format: Option<&'v str>,
    prepend: Option<&'v str>,
    quote: Option<QuoteStyle>,
    replacements: Vec<(FormatRegex<'v>, &'v str)>,
}

/// The version of what `CommandLineBuilder::new_hashing` hashes, and so of the hashes themselves:
/// the first thing every hash covers. The hashes outlive the daemon that computes them: they key
/// action cache entries that a later build, on another host and another version of buck2, looks
/// up. Bump this when rendering or hashing changes in any way, so that nothing keyed by a hash
/// from before the change is found after it; the golden test in `buck2_build_api_tests` says
/// when that is.
pub const HASH_VERSION: u32 = 1;

/// An operation that can be performed on a command line builder, used for its hash impl
///
/// IMPORTANT: If you change any of the definition or the hash impl of this type, or *any* of the
/// behavior/interpretation of any of these operations, you must increment the constant above.
#[derive(StrongHash)]
enum Op<'a> {
    PushScope(&'a StringOptions<'a>, &'a ArtifactOptions<'a>),
    PopScope,
    PushStr(&'a str),
    PushArtifact(&'a Artifact),
    PushOutputArtifact(&'a Artifact),
    PushCellPath(&'a CellPath),
    PushNextWriteToFileMacroPath,
    PushProjectPath(&'a ProjectRelativePath),
    /// A transitive set projection, by `TransitiveSet::projection_hash` and how it is traversed.
    PushTransitiveSetProjection {
        hash: StrongHash128,
        projection: usize,
        ordering: TransitiveSetOrdering,
    },
}

enum CommandLineBuilderInner<'v, 'a> {
    Hash { h: &'a mut Blake3StrongHasher },
    Regular(CommandLineBuilderRegular<'v, 'a>),
}

struct CommandLineBuilderRegular<'v, 'a> {
    sink: &'a mut dyn CommandLineSink,
    artifact_path_mapping: &'a dyn ArtifactPathMapper,
    fs: &'a ExecutorFs<'a>,
    absolute: bool,
    // Stacks of currently in-scope actions; these are pushed/popped as scopes are entered/exited.
    //
    // Note that these two stacks work differently; the artifact options stack is cumulative, ie the
    // top entry in the stack represents the combined effects of all previous entries and so is the
    // only one that is "applied," while the same is not true of the string options.
    artifact_options_stack: Vec<ArtifactOptions<'v>>,
    string_options_stack: Vec<StringOptions<'v>>,

    // Paths where write-to-file macros go. Note that order matters and this list is in *reverse*
    // order (so that we can pop off the end).
    write_to_file_macro_paths: Option<Vec<ProjectRelativePathBuf>>,
}

pub struct CommandLineBuilder<'v, 'a>(CommandLineBuilderInner<'v, 'a>);

/// The last bytes a hashing builder feeds its hasher, on drop; see `new_hashing`. Random, so
/// that no sequence of operations feeds the same bytes.
const HASH_TERMINATOR: [u8; 32] = [
    0xef, 0x80, 0x72, 0x2a, 0xf2, 0x0c, 0xbc, 0xcc, 0xcf, 0x39, 0x11, 0xd8, 0xbe, 0x5a, 0xa2, 0x54,
    0x0b, 0xd3, 0xb1, 0xf8, 0x5d, 0xb6, 0x33, 0xf9, 0x48, 0x7a, 0xc4, 0xca, 0xb1, 0x83, 0x4f, 0xaa,
];

/// A hashing builder, from `CommandLineBuilder::new_hashing`. Ends what it has written when it is
/// dropped, so the builder itself is used through it as any other.
pub struct HashingCommandLineBuilder<'v, 'a>(CommandLineBuilder<'v, 'a>);

impl<'v, 'a> Deref for HashingCommandLineBuilder<'v, 'a> {
    type Target = CommandLineBuilder<'v, 'a>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for HashingCommandLineBuilder<'_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for HashingCommandLineBuilder<'_, '_> {
    fn drop(&mut self) {
        if let CommandLineBuilderInner::Hash { h } = &mut self.0.0 {
            h.write(&HASH_TERMINATOR);
        }
    }
}

impl<'v, 'a> CommandLineBuilder<'v, 'a> {
    pub fn new(
        sink: &'a mut dyn CommandLineSink,
        artifact_path_mapping: &'a dyn ArtifactPathMapper,
        fs: &'a ExecutorFs<'a>,
    ) -> Self {
        Self::new_with_options(sink, artifact_path_mapping, fs, false, None)
    }

    pub fn new_with_options(
        sink: &'a mut dyn CommandLineSink,
        artifact_path_mapping: &'a dyn ArtifactPathMapper,
        fs: &'a ExecutorFs<'a>,
        absolute: bool,
        mut write_to_file_macro_paths: Option<Vec<ProjectRelativePathBuf>>,
    ) -> Self {
        if let Some(write_to_file_macro_paths) = &mut write_to_file_macro_paths {
            write_to_file_macro_paths.reverse();
        }
        Self(CommandLineBuilderInner::Regular(
            CommandLineBuilderRegular {
                sink,
                artifact_path_mapping,
                fs,
                absolute,
                artifact_options_stack: Vec::new(),
                string_options_stack: Vec::new(),
                write_to_file_macro_paths,
            },
        ))
    }

    /// Create a hashing builder that computes a strong hash of the command line.
    ///
    /// The hash is an identity for the command line that outlives this daemon: it keys action
    /// cache entries, which a later build and a later version of buck2 look up. `HASH_VERSION`
    /// is what keeps a change to rendering or hashing from reading as a hit.
    ///
    /// The general contract of this function is that if two `CommandArgLike`s hash to the same hash
    /// value as per this builder, then they would have produced the exact same command line when
    /// using the non-hashing builder. The only caveat to the contract is that the hash produced
    /// here must first be combined with a hash of the options that would have been passed to
    /// `new_with_options` in the non-hashing builder - without that there are no guarantees.
    ///
    /// This contract is fairly narrow in both directions. Equality does not go the other way; some
    /// `CommandArgLike`s that produce the same command line will nonetheless hash to different
    /// values. Despite that, also not all properties of a `CommandArgLike` are reflected in this
    /// hash. For example, two `CommandArgLike`s may hash to the same value despite having different
    /// artifact dependencies.
    ///
    /// Artifacts and cell paths are hashed by identity, not by the paths they render as, so the
    /// hash does not depend on where `buck-out` and the cells are. A key built on it has to cover
    /// those itself where they can differ.
    ///
    /// What the builder writes to `sink` is prefix-free: a terminator ends it when the builder is
    /// dropped, so a caller may hash more into `sink` afterwards.
    ///
    /// For all of these reasons, this function is extremely difficult to use correctly. Strongly
    /// reconsider if tempted.
    pub fn new_hashing(sink: &'a mut Blake3StrongHasher) -> HashingCommandLineBuilder<'v, 'a> {
        sink.write_u32(HASH_VERSION);
        HashingCommandLineBuilder(Self(CommandLineBuilderInner::Hash { h: sink }))
    }

    /// Whether this is a hashing builder (`new_hashing`) rather than a rendering one.
    pub(crate) fn is_hashing(&self) -> bool {
        matches!(&self.0, CommandLineBuilderInner::Hash { .. })
    }

    /// Hashes a transitive set projection by its precomputed `hash` in place of its contents. For
    /// a hashing builder only; a rendering builder renders the contents.
    pub(crate) fn push_transitive_set_projection(
        &mut self,
        hash: StrongHash128,
        projection: usize,
        ordering: TransitiveSetOrdering,
    ) -> buck2_error::Result<()> {
        match &mut self.0 {
            CommandLineBuilderInner::Hash { h, .. } => {
                Op::PushTransitiveSetProjection {
                    hash,
                    projection,
                    ordering,
                }
                .strong_hash(h);
                Ok(())
            }
            CommandLineBuilderInner::Regular(_) => Err(internal_error!(
                "Precomputed transitive set projection hashes are for hashing builders only"
            )),
        }
    }

    pub(crate) fn push_scope(
        &mut self,
        opts: &CommandLineOptionsRef<'v, '_>,
    ) -> buck2_error::Result<()> {
        let CommandLineOptionsRef {
            relative_to,
            absolute_prefix,
            absolute_suffix,
            parent,
            ignore_artifacts: _,
            salt_hidden: _,
            delimiter,
            format,
            prepend,
            quote,
            replacements,
        } = opts;

        let string_opts = StringOptions {
            delimiter: delimiter.map(|s| (s.as_str(), DelimiterJoinState::Unstarted)),
            format: format.map(|s| s.as_str()),
            prepend: prepend.map(|s| s.as_str()),
            quote: *quote,
            replacements: FormatRegex::make(*replacements),
        };

        let artifact_opts = ArtifactOptions {
            relative_to: relative_to
                .as_ref()
                .map(|r| unpack_relative_origin(r.0, r.1))
                .transpose()?
                .map(Arc::new),
            absolute_prefix: absolute_prefix.map(|s| s.as_str()),
            absolute_suffix: absolute_suffix.map(|s| s.as_str()),
            parent: *parent,
        };

        self.merge_and_push_scope(string_opts, artifact_opts);
        Ok(())
    }

    /// Equivalent to `push_scope` with just a delimiter set, but doesn't require constructing starlark values
    pub fn push_scope_delimiter(&mut self, delimiter: &'v str) {
        self.merge_and_push_scope(
            StringOptions {
                delimiter: Some((delimiter, DelimiterJoinState::Unstarted)),
                ..Default::default()
            },
            DEFAULT_ARTIFACT_OPTIONS,
        )
    }

    /// Equivalent to `push_scope`, but doesn't require constructing starlark values
    pub fn push_scope_format(&mut self, format: &'v str) {
        self.merge_and_push_scope(
            StringOptions {
                format: Some(format),
                ..Default::default()
            },
            DEFAULT_ARTIFACT_OPTIONS,
        )
    }

    /// Equivalent to `push_scope`, but doesn't require constructing starlark values
    pub fn push_scope_relative_to(&mut self, relative_to: ProjectRelativePathBuf) {
        self.merge_and_push_scope(
            StringOptions::default(),
            ArtifactOptions {
                relative_to: Some(Arc::new(RelativeToOption::Path(relative_to))),
                ..DEFAULT_ARTIFACT_OPTIONS
            },
        )
    }

    fn merge_and_push_scope(
        &mut self,
        string_opts: StringOptions<'v>,
        artifact_opts: ArtifactOptions<'v>,
    ) {
        match &mut self.0 {
            CommandLineBuilderInner::Hash { h, .. } => {
                Op::PushScope(&string_opts, &artifact_opts).strong_hash(h);
            }
            CommandLineBuilderInner::Regular(b) => {
                b.merge_and_push_scope(string_opts, artifact_opts)
            }
        }
    }

    pub fn pop_scope(&mut self) {
        match &mut self.0 {
            CommandLineBuilderInner::Hash { h, .. } => {
                Op::PopScope.strong_hash(h);
            }
            CommandLineBuilderInner::Regular(b) => b.pop_scope(),
        }
    }

    pub fn push_str(&mut self, s: &str) {
        self.push_string(s.to_owned());
    }

    pub fn push_string(&mut self, s: String) {
        match &mut self.0 {
            CommandLineBuilderInner::Hash { h, .. } => {
                Op::PushStr(&s).strong_hash(h);
            }
            CommandLineBuilderInner::Regular(b) => b.write_str(s),
        }
    }

    pub fn push_artifact(&mut self, artifact: &Artifact) -> buck2_error::Result<()> {
        match &mut self.0 {
            CommandLineBuilderInner::Hash { h } => {
                // The identity only; a key built on the hash covers the artifact path mapping
                // that rendering resolves content-based paths through.
                Op::PushArtifact(artifact).strong_hash(h);
                Ok(())
            }
            CommandLineBuilderInner::Regular(b) => {
                let p = artifact.resolve_path(b.fs.fs(), b.artifact_path_mapping.get(artifact))?;
                b.write_project_path(p)
            }
        }
    }

    pub fn push_output_artifact(&mut self, artifact: &Artifact) -> buck2_error::Result<()> {
        match &mut self.0 {
            CommandLineBuilderInner::Hash { h } => {
                Op::PushOutputArtifact(artifact).strong_hash(h);
                Ok(())
            }
            CommandLineBuilderInner::Regular(b) => {
                let p = artifact.resolve_path(
                    b.fs.fs(),
                    Some(&ContentBasedPathHash::for_output_artifact()),
                )?;
                b.write_project_path(p)
            }
        }
    }

    pub(crate) fn push_next_write_to_file_macro_path(&mut self) -> buck2_error::Result<()> {
        match &mut self.0 {
            CommandLineBuilderInner::Hash { h, .. } => {
                Op::PushNextWriteToFileMacroPath.strong_hash(h);
                Ok(())
            }
            CommandLineBuilderInner::Regular(b) => {
                let Some(write_to_file_macro_paths) = &mut b.write_to_file_macro_paths else {
                    return Err(CommandLineOptionsError::WriteToFileMacroNotSupported.into());
                };
                let p = write_to_file_macro_paths.pop().ok_or_else(|| {
                    buck2_error::internal_error!("Inconsistent number of macro artifacts")
                })?;
                b.write_project_path(p)
            }
        }
    }

    pub fn push_cell_path(&mut self, path: CellPath) -> buck2_error::Result<()> {
        match &mut self.0 {
            CommandLineBuilderInner::Hash { h } => {
                Op::PushCellPath(&path).strong_hash(h);
                Ok(())
            }
            CommandLineBuilderInner::Regular(b) => {
                let p = b.fs.fs().resolve_cell_path(path)?;
                b.write_project_path(p)
            }
        }
    }

    pub fn push_project_path(&mut self, path: ProjectRelativePathBuf) -> buck2_error::Result<()> {
        match &mut self.0 {
            CommandLineBuilderInner::Hash { h, .. } => {
                Op::PushProjectPath(&path).strong_hash(h);
                Ok(())
            }
            CommandLineBuilderInner::Regular(r) => r.write_project_path(path),
        }
    }
}

impl<'v> CommandLineBuilderRegular<'v, '_> {
    fn merge_and_push_scope(
        &mut self,
        string_opts: StringOptions<'v>,
        artifact_opts: ArtifactOptions<'v>,
    ) {
        self.string_options_stack.push(string_opts);

        let prev = self
            .artifact_options_stack
            .last()
            .unwrap_or(&DEFAULT_ARTIFACT_OPTIONS);
        let artifact_opts = ArtifactOptions {
            relative_to: artifact_opts
                .relative_to
                .or_else(|| prev.relative_to.dupe()),
            absolute_prefix: artifact_opts.absolute_prefix.or(prev.absolute_prefix),
            absolute_suffix: artifact_opts.absolute_suffix.or(prev.absolute_suffix),
            parent: artifact_opts.parent + prev.parent,
        };
        self.artifact_options_stack.push(artifact_opts);
    }

    fn pop_scope(&mut self) {
        self.artifact_options_stack.pop();
        if let Some(string_options) = self.string_options_stack.pop() {
            if let Some((_, delim_state)) = string_options.delimiter {
                self.write_str(delim_state.finish());
            }
        }
    }

    fn write_project_path(&mut self, mut path: ProjectRelativePathBuf) -> buck2_error::Result<()> {
        let ArtifactOptions {
            relative_to,
            absolute_prefix,
            absolute_suffix,
            parent,
        } = self
            .artifact_options_stack
            .last()
            .unwrap_or(&DEFAULT_ARTIFACT_OPTIONS);

        let components = path.iter().count();
        if (components as u32) < *parent {
            return Err(
                CommandLineOptionsError::TooManyParentCalls(path.to_string(), *parent).into(),
            );
        }
        for _ in 0..*parent {
            path.pop();
        }

        let mut rendered = if let Some(relative_to) = relative_to {
            let p = finish_relative_to_path(relative_to, self.fs.fs(), self.artifact_path_mapping)?
                .as_forward_relative_path()
                .as_relative_path()
                .relative(path.as_forward_relative_path().as_relative_path());
            path_format(&p, self.fs.path_separator()).into_owned()
        } else if self.absolute {
            // Note that for obvious reasons we ignore `absolute` in the case where there's a
            // `relative_to` provided
            path_format_absolute(&path, self.fs.path_separator(), self.fs.fs().fs()).into_owned()
        } else {
            path_format(path.as_ref(), self.fs.path_separator()).into_owned()
        };

        if let Some(absolute_prefix) = absolute_prefix {
            rendered.insert_str(0, absolute_prefix);
        }
        if let Some(absolute_suffix) = absolute_suffix {
            rendered.push_str(absolute_suffix);
        }

        self.write_str(rendered);
        Ok(())
    }

    fn write_str(&mut self, v: String) {
        Self::write_str_inner(v, self.sink, &mut self.string_options_stack);
    }

    fn write_str_inner(
        v: String,
        sink: &mut dyn CommandLineSink,
        string_options: &mut [StringOptions<'v>],
    ) {
        let mut cur = v;
        // See below for why we do this
        let mut string_options_iter = string_options.iter_mut();
        while let Some(options) = string_options_iter.next_back() {
            for (reg, rep) in options.replacements.iter() {
                let new = match reg {
                    FormatRegex::Starlark(reg) => reg.replace_all(cur.as_ref(), rep),
                    FormatRegex::Owned(reg) => reg.replace_all(cur.as_ref(), *rep),
                };
                if new.as_ref() != cur {
                    cur = new.into_owned();
                }
            }
            if let Some(format) = options.format {
                cur = format.replace("{}", &cur);
            }
            if let Some(QuoteStyle::Shell) = options.quote {
                let quoted = shlex_quote(&cur);
                cur = quoted.into_owned();
            }
            // `prepend` has pretty different semantics between the cases where there's a delimiter
            // in the same options and when there isn't
            if let Some((delim, delim_state)) = &mut options.delimiter {
                if let Some(prepend) = options.prepend {
                    cur.insert_str(0, prepend);
                }
                delim_state.push(cur, delim);
                // We won't know how to finish applying the options on anything lower in the stack
                // until we finish accumulating this string, so we exit now; we'll handle the lower
                // options when popping this scope
                return;
            } else {
                if let Some(prepend) = options.prepend {
                    // We need to write both `prepend` and `cur` into the command line, however any
                    // options lower in the stack than the current one still need to be applied. We
                    // implement via recursion. That's not ideal, but note that repeated use of
                    // `prepend` results in exponential growth of the command line, so its not very easy
                    // to avoid either
                    Self::write_str_inner(
                        prepend.to_owned(),
                        sink,
                        string_options_iter.as_mut_slice(),
                    );
                }
            }
        }
        sink.push_arg(Cow::Owned(cur));
    }
}

/// Returns the path rendered in a way that is appropriate for use in command lines.
pub fn path_format<'p>(path: &'p RelativePath, sep: PathSeparatorKind) -> Cow<'p, str> {
    let parts = path.components().count();
    if parts == 0 {
        // In command lines, the empty path is the current directory, so use that instead
        // so we don't have to deal with empty strings being implicit current directory.
        Cow::Borrowed(".")
    } else if parts == 1 {
        // If the path isn't empty, but doesn't have any path separators, add `./` This
        // ensures that if we have an executable relative to the repo root, we can
        // execute it (since `execvp`'s behavior is dependent on the presence of a `/`
        // in the path).
        let mut res = String::with_capacity(path.as_str().len() + 2);
        res.push('.');
        res.push(match sep {
            PathSeparatorKind::Unix => '/',
            PathSeparatorKind::Windows => '\\',
        });
        res.push_str(path.as_str());
        Cow::Owned(res)
    } else {
        match sep {
            PathSeparatorKind::Unix => Cow::Borrowed(path.as_str()),
            PathSeparatorKind::Windows => Cow::Owned(path.as_str().replace('/', "\\")),
        }
    }
}

pub(crate) fn path_format_absolute<'p>(
    path: &'p ProjectRelativePath,
    sep: PathSeparatorKind,
    root: &ProjectRoot,
) -> Cow<'p, str> {
    let mut p = root.root().to_path_buf();
    p.extend(path.iter());
    // FIXME(JakobDegen): Seriously?
    let s = p.to_string_lossy();
    // FIXME(JakobDegen): This is pretty confused. `PathBuf` and our absolute path types
    // use the host's path separators. Fix, and also write a test, and also fix the path
    // types to make these sorts of bugs not possible
    if sep == PathSeparatorKind::Windows {
        Cow::Owned(s.as_ref().replace('/', "\\"))
    } else {
        Cow::Owned(s.into_owned())
    }
}

#[cfg(test)]
mod tests {
    use buck2_core::execution_types::executor_config::PathSeparatorKind;
    use buck2_fs::paths::RelativePath;

    use super::*;

    #[test]
    fn test_path_format() {
        let unix = PathSeparatorKind::Unix;
        let windows = PathSeparatorKind::Windows;

        assert_eq!(path_format(RelativePath::empty(), unix), ".");
        assert_eq!(path_format(RelativePath::empty(), windows), ".");
        assert_eq!(path_format(RelativePath::unchecked_new("a"), unix), "./a");
        assert_eq!(
            path_format(RelativePath::unchecked_new("a"), windows),
            ".\\a"
        );
        assert_eq!(path_format(RelativePath::unchecked_new("a/b"), unix), "a/b");
        assert_eq!(
            path_format(RelativePath::unchecked_new("a/b"), windows),
            "a\\b"
        );
    }
}
