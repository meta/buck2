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
use std::collections::VecDeque;
use std::future::Future;
use std::hash::Hash;
use std::io::Write;
use std::pin::Pin;
use std::sync::Arc;

use allocative::Allocative;
use buck2_artifact::actions::key::ActionKey;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_core::build_file_path::BuildFilePath;
use buck2_core::cells::CellResolver;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::content_hash::ContentBasedPathHash;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_core::package::PackageLabel;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_execute::artifact::fs::ExecutorFs;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutSet;
use buck2_node::attrs::configured_attr::ConfiguredAttr;
use buck2_query::query::environment::QueryTarget;
use buck2_query::query::graph::node::LabeledNode;
use buck2_query::query::graph::node::NodeKey;
use buck2_util::late_binding::LateBinding;
use derivative::Derivative;
use dice::DiceComputations;
use dupe::Dupe;
use either::Either;
use internment::ArcIntern;
use pagable::Pagable;
use ref_cast::RefCast;
use serde::Serialize;
use starlark::values::Heap;
use starlark::values::Value;
use variant_name::VariantName;

use crate::actions::RegisteredAction;
use crate::analysis::AnalysisResult;
use crate::artifact_groups::TransitiveSetProjectionKey;
use crate::interpreter::rule_defs::cmd_args::ArtifactPathMapper;
use crate::interpreter::rule_defs::provider::collection::FrozenProviderCollectionValue;

#[derive(Debug, derive_more::Display, RefCast, Serialize, Allocative, Pagable)]
#[repr(transparent)]
#[serde(transparent)]
pub struct OwnedActionAttr(pub String);

#[derive(Debug, derive_more::Display, RefCast, Serialize, Allocative)]
#[repr(transparent)]
#[serde(transparent)]
pub struct ActionAttr(pub(crate) str);

impl ActionAttr {
    pub(crate) fn new(x: &str) -> &Self {
        ActionAttr::ref_cast(x)
    }

    pub fn to_owned(&self) -> OwnedActionAttr {
        OwnedActionAttr(self.0.to_owned())
    }
}

#[derive(Debug, Eq, PartialEq, Hash)]
pub struct SetProjectionInputsData {
    key: TransitiveSetProjectionKey,
    pub direct: Vec<ActionQueryNodeRef>,
    pub children: Vec<SetProjectionInputs>,
}

impl SetProjectionInputsData {
    pub fn key(&self) -> &TransitiveSetProjectionKey {
        &self.key
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct SetProjectionInputs {
    pub node: ArcIntern<SetProjectionInputsData>,
}

// ArcIntern doesn't impl Dupe, but is cheap to copy, so we need to explicitly impl it.
impl Dupe for SetProjectionInputs {}

impl SetProjectionInputs {
    pub fn new(
        key: TransitiveSetProjectionKey,
        direct: Vec<ActionQueryNodeRef>,
        children: Vec<SetProjectionInputs>,
    ) -> Self {
        Self {
            node: ArcIntern::new(SetProjectionInputsData {
                key,
                direct,
                children,
            }),
        }
    }
}

#[derive(Debug)]
pub enum ActionInput {
    ActionKey(ActionQueryNodeRef),
    IndirectInputs(SetProjectionInputs),
}

/// Both the fields are enums here. Ideally they wouldn't be, but since the query interface wants a
/// &NodeRef (and that is rather hard to change), we pull this out.
///
/// Not pagable: it carries a `RegisteredAction` and an `AnalysisResult`. Anything holding one
/// fails page-out with an error and stays resident.
#[derive(Debug, Clone, Dupe, Allocative, pagable::PagableUnsupported)]
pub struct ActionQueryNode {
    key: ActionQueryNodeRef,
    #[allocative(skip)] // TODO(@wendyy) we should derive allocative for action-related structs
    data: ActionQueryNodeData,
}

// SAFETY: `_assert_send_sync` requires every field to be `Send + Sync`. Spelled out
// so that auto-trait proofs for aquery futures stop here; see `dice::DiceComputations`.
unsafe impl Send for ActionQueryNode {}
unsafe impl Sync for ActionQueryNode {}

fn _assert_send_sync(ActionQueryNode { key, data }: &ActionQueryNode) {
    fn assert<T: Send + Sync>(_: &T) {}
    assert(key);
    assert(data);
}

impl PartialEq for ActionQueryNode {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for ActionQueryNode {}

#[derive(Debug, Clone, Dupe)]
pub enum ActionQueryNodeData {
    /// A node's analysis. This doesn't do anything on its own (it's a bit like a forward node in
    /// Cquery!), but it can be used to pass it to functions. When traversing deps, this will be
    /// ignored, since aquery is for queries on actions, and not targts.
    Analysis(AnalysisData),

    /// An actual action.
    Action(ActionData),
}

impl ActionQueryNode {
    pub fn new_action(
        action: Arc<RegisteredAction>,
        deps: Vec<ActionInput>,
        fs: ArtifactFs,
    ) -> Self {
        Self {
            key: ActionQueryNodeRef::Action(action.key().dupe()),
            data: ActionQueryNodeData::Action(ActionData {
                action,
                deps: Arc::new(deps),
                fs,
            }),
        }
    }

    pub fn new_analysis(target: ConfiguredProvidersLabel, analysis: AnalysisResult) -> Self {
        Self {
            key: ActionQueryNodeRef::Analysis(target.dupe()),
            data: ActionQueryNodeData::Analysis(AnalysisData { target, analysis }),
        }
    }

    pub fn action(&self) -> Option<&Arc<RegisteredAction>> {
        match &self.data {
            ActionQueryNodeData::Analysis(..) => None,
            ActionQueryNodeData::Action(data) => Some(&data.action),
        }
    }

    pub fn analysis_opt(&self) -> Option<&AnalysisData> {
        match &self.data {
            ActionQueryNodeData::Analysis(a) => Some(a),
            ActionQueryNodeData::Action(..) => None,
        }
    }

    pub fn data(&self) -> &ActionQueryNodeData {
        &self.data
    }

    pub fn key(&self) -> &ActionQueryNodeRef {
        &self.key
    }
}

impl LabeledNode for ActionQueryNode {
    type Key = ActionQueryNodeRef;

    fn node_key(&self) -> &Self::Key {
        &self.key
    }
}

#[derive(Derivative, Clone, Dupe)]
#[derivative(Debug)]
pub struct AnalysisData {
    target: ConfiguredProvidersLabel,
    #[derivative(Debug = "ignore")]
    analysis: AnalysisResult,
}

impl AnalysisData {
    pub fn providers(&self) -> buck2_error::Result<FrozenProviderCollectionValue> {
        self.analysis.lookup_inner(&self.target)
    }

    pub fn analysis_result(&self) -> &AnalysisResult {
        &self.analysis
    }

    pub fn target(&self) -> &ConfiguredProvidersLabel {
        &self.target
    }
}

pub struct AqueryArtifactPathMapper {
    aquery_placeholder: ContentBasedPathHash,
}

impl ArtifactPathMapper for AqueryArtifactPathMapper {
    fn get(&self, _artifact: &Artifact) -> Option<&ContentBasedPathHash> {
        Some(&self.aquery_placeholder)
    }
}

#[derive(Derivative, Clone, Dupe)]
#[derivative(Debug)]
pub struct ActionData {
    action: Arc<RegisteredAction>,
    deps: Arc<Vec<ActionInput>>,
    #[derivative(Debug = "ignore")]
    fs: ArtifactFs,
}

impl ActionData {
    /// The action's inputs with the tset structure preserved, unlike the flattened
    /// `QueryTarget::deps()` view.
    pub fn inputs(&self) -> &[ActionInput] {
        &self.deps
    }

    fn attrs(&self) -> buck2_error::Result<BuckMutInsertionMap<String, String>> {
        let mut attrs = self.action.action().aquery_attributes(
            &ExecutorFs::new(
                &self.fs,
                self.action.execution_config().options.path_separator,
            ),
            &AqueryArtifactPathMapper {
                aquery_placeholder: ContentBasedPathHash::AqueryPlaceholder,
            },
        )?;
        attrs.insert(
            "buck.executor_configuration".to_owned(),
            self.action.execution_config().executor.to_string(),
        );
        if let Some(group) = self.action.execution_group() {
            attrs.insert("buck.exec_group".to_owned(), group.to_string());
        }
        attrs.insert(
            "buck.all_outputs_are_content_based".to_owned(),
            self.action
                .action()
                .all_outputs_are_content_based()
                .to_string(),
        );
        attrs.insert(
            "buck.all_inputs_are_eligible_for_dedupe".to_owned(),
            self.action
                .action()
                .all_inputs_are_eligible_for_dedupe()
                .to_string(),
        );

        let all_ineligible = self.action.action().all_ineligible_for_dedup_inputs();
        if !all_ineligible.is_empty() {
            attrs.insert(
                "buck.all_ineligible_for_dedup_inputs".to_owned(),
                all_ineligible.join(", "),
            );
        }

        Ok(attrs)
    }
}

#[derive(
    Debug,
    Clone,
    Hash,
    Eq,
    PartialEq,
    derive_more::Display,
    Dupe,
    Allocative
)]
pub enum ActionQueryNodeRef {
    Analysis(ConfiguredProvidersLabel),
    Action(ActionKey),
}

#[derive(Copy, Clone)]
pub enum PackageLabelOption {
    PackageLabel(PackageLabel),
    TransitionAttr,
}

impl NodeKey for ActionQueryNodeRef {}

impl ActionQueryNodeRef {
    pub fn require_action(&self) -> buck2_error::Result<&ActionKey> {
        match self {
            Self::Analysis(a) => Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Tier0,
                "Not an action: {}",
                a
            )),
            Self::Action(a) => Ok(a),
        }
    }
}

impl QueryTarget for ActionQueryNode {
    type Attr<'a> = ActionAttr;

    fn rule_type(&self) -> Cow<'_, str> {
        match &self.data {
            ActionQueryNodeData::Analysis(..) => Cow::Borrowed("analysis"),
            ActionQueryNodeData::Action(a) => {
                Cow::Owned(a.action.kind().variant_name().to_ascii_lowercase())
            }
        }
    }

    fn name(&self) -> Cow<'_, str> {
        Cow::Owned(self.node_key().to_string())
    }

    /// Return the path to the buildfile that defines this target, e.g. `fbcode//foo/bar/TARGETS`
    fn buildfile_path(&self) -> buck2_error::Result<&BuildFilePath> {
        // TODO(cjhopman): In addition to implementing this, we should be able to return an buck2_error::Error here rather than panicking.
        unimplemented!("buildfile not yet implemented in aquery")
    }

    fn deps<'a>(&'a self) -> impl Iterator<Item = &'a Self::Key> + Send + 'a {
        // When traversing deps in aquery, we do *not* traverse deps for the target nodes, since
        // those are just for literals
        let action = match &self.data {
            ActionQueryNodeData::Action(action) => action,
            ActionQueryNodeData::Analysis(..) => return Either::Left(std::iter::empty()),
        };

        Either::Right(iter_action_inputs(&action.deps))
    }

    fn exec_deps<'a>(&'a self) -> impl Iterator<Item = &'a Self::Key> + Send + 'a {
        std::iter::empty()
    }

    fn target_deps<'a>(&'a self) -> impl Iterator<Item = &'a Self::Key> + Send + 'a {
        self.deps()
    }

    fn configuration_deps<'a>(&'a self) -> impl Iterator<Item = &'a Self::Key> + Send + 'a {
        std::iter::empty()
    }

    fn toolchain_deps<'a>(&'a self) -> impl Iterator<Item = &'a Self::Key> + Send + 'a {
        // TODO(ezgi): implement toolchain deps for aquery
        std::iter::empty()
    }

    fn attr_any_matches(
        attr: &Self::Attr<'_>,
        filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
    ) -> buck2_error::Result<bool> {
        filter(&attr.0)
    }

    fn special_attrs_for_each<
        E: From<buck2_error::Error>,
        F: FnMut(&str, &Self::Attr<'_>) -> Result<(), E>,
    >(
        &self,
        mut _func: F,
    ) -> Result<(), E> {
        Ok(())
    }

    fn attrs_for_each<
        E: From<buck2_error::Error>,
        F: FnMut(&str, &Self::Attr<'_>) -> Result<(), E>,
    >(
        &self,
        mut func: F,
    ) -> Result<(), E> {
        func("kind", ActionAttr::new(&self.rule_type()))?;

        let action = match &self.data {
            ActionQueryNodeData::Action(action) => action,
            ActionQueryNodeData::Analysis(..) => return Ok(()),
        };

        func(
            "category",
            ActionAttr::new(action.action.category().as_str()),
        )?;
        func(
            "identifier",
            ActionAttr::new(action.action.identifier().unwrap_or("")),
        )?;

        // inputs and outputs are not supported for aquery

        for (k, v) in action.attrs()? {
            func(&k, ActionAttr::new(&v))?;
        }
        Ok(())
    }

    fn defined_attrs_for_each<
        E: From<buck2_error::Error>,
        F: FnMut(&str, &Self::Attr<'_>) -> Result<(), E>,
    >(
        &self,
        func: F,
    ) -> Result<(), E> {
        self.attrs_for_each(func)
    }

    fn map_attr<R, F: FnMut(Option<&Self::Attr<'_>>) -> R>(
        &self,
        key: &str,
        mut func: F,
    ) -> buck2_error::Result<R> {
        let mut res = None;

        self.attrs_for_each(|k, attr| {
            if k == key {
                res = Some(func(Some(attr)));
            }
            Ok::<(), buck2_error::Error>(())
        })?;
        Ok(match res {
            Some(v) => v,
            None => func(None),
        })
    }

    fn inputs_for_each<E: From<buck2_error::Error>, F: FnMut(CellPath) -> Result<(), E>>(
        &self,
        mut _func: F,
    ) -> Result<(), E> {
        // TODO(cjhopman): In addition to implementing this, we should be able to return an buck2_error::Error here rather than panicking.
        unimplemented!("inputs not yet implemented in aquery")
    }

    fn map_any_attr<R, F: FnMut(Option<&Self::Attr<'_>>) -> R>(
        &self,
        key: &str,
        func: F,
    ) -> buck2_error::Result<R> {
        // aquery doesn't have special attrs, so this is the same as map_attr
        self.map_attr(key, func)
    }
}

pub fn iter_action_inputs<'a>(
    deps: &'a [ActionInput],
) -> impl Iterator<Item = &'a ActionQueryNodeRef> + Send + 'a {
    struct Iter<'a> {
        visited: BuckMutSet<&'a SetProjectionInputs>,
        queue: VecDeque<&'a SetProjectionInputs>,
    }

    impl<'a> Iter<'a> {
        fn new<From: Iterator<Item = &'a SetProjectionInputs>>(iter: From) -> Self {
            let mut visited = BuckMutSet::default();
            let mut queue = VecDeque::new();
            for it in iter {
                if visited.insert(it) {
                    queue.push_back(it);
                }
            }
            Self { visited, queue }
        }
    }

    impl<'a> Iterator for Iter<'a> {
        type Item = &'a SetProjectionInputs;

        fn next(&mut self) -> Option<Self::Item> {
            self.queue.pop_front().inspect(|node| {
                for child in &*node.node.children {
                    if self.visited.insert(child) {
                        self.queue.push_back(child);
                    }
                }
            })
        }
    }

    let direct = deps.iter().filter_map(|input| match input {
        ActionInput::ActionKey(action_key) => Some(action_key),
        ActionInput::IndirectInputs(..) => None,
    });

    let indirect = Iter::new(deps.iter().filter_map(|input| match input {
        ActionInput::ActionKey(..) => None,
        ActionInput::IndirectInputs(val) => Some(val),
    }));

    let indirect = Iter::new(indirect);

    direct.chain(indirect.flat_map(|v| v.node.direct.iter()))
}

pub static FIND_MATCHING_ACTION: LateBinding<
    for<'c> fn(
        &'c mut DiceComputations,
        // Working dir.
        &'c ProjectRelativePath,
        // target cfg info (target platform, cli modifiers)
        &'c GlobalCfgOptions,
        &'c AnalysisResult,
        // short_path
        ForwardRelativePathBuf,
    ) -> Pin<
        Box<dyn Future<Output = buck2_error::Result<Option<ActionQueryNode>>> + Send + 'c>,
    >,
> = LateBinding::new("FIND_MATCHING_ACTION");

/// Hook to link printer in `buck2_server_commands` from `buck2_cmd_audit_server`.
pub static PRINT_ACTION_NODE: LateBinding<
    for<'a> fn(
        stdout: &'a mut (dyn Write + Send),
        action: ActionQueryNode,
        json: bool,
        output_attributes: &'a [String],
        cell_resolver: &'a CellResolver,
    ) -> Pin<Box<dyn Future<Output = buck2_error::Result<()>> + Send + 'a>>,
> = LateBinding::new("PRINT_ACTION_NODE");

/// Use of "configured_attr_to_value" in `buck2_transition` from `buck2_analysis`.
pub static CONFIGURED_ATTR_TO_VALUE: LateBinding<
    for<'v> fn(
        this: &ConfiguredAttr,
        pkg: PackageLabelOption,
        heap: Heap<'v>,
    ) -> buck2_error::Result<Value<'v>>,
> = LateBinding::new("CONFIGURED_ATTR_TO_VALUE");
