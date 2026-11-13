/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_hash::BuckMutMap;
use buck2_node::attrs::configured_attr::ConfiguredAttr;
use buck2_node::attrs::display::AttrDisplayWithContextExt;
use buck2_node::attrs::inspect_options::AttrInspectOptions;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_query::query::environment::QueryTarget;
use flatbuffers::FlatBufferBuilder;
use flatbuffers::WIPOffset;

use crate::ActionEntryData;
use crate::ChangedFilesEntryData;

mod fbs {
    pub use crate::explain_generated::explain::Action;
    pub use crate::explain_generated::explain::ActionArgs;
    pub use crate::explain_generated::explain::Build;
    pub use crate::explain_generated::explain::BuildArgs;
    pub use crate::explain_generated::explain::CodePointer;
    pub use crate::explain_generated::explain::CodePointerArgs;
    pub use crate::explain_generated::explain::ConfiguredTargetLabel;
    pub use crate::explain_generated::explain::ConfiguredTargetLabelArgs;
    pub use crate::explain_generated::explain::ConfiguredTargetNode;
    pub use crate::explain_generated::explain::ConfiguredTargetNodeArgs;
}

enum AttrField {
    Other,
    StringList(Vec<String>),
    StringDict(Vec<(String, String)>),
}

struct TargetData {
    node: ConfiguredTargetNode,
    actions: Vec<ActionEntryData>,
    changed_files: Vec<String>,
}

pub(crate) fn gen_fbs(
    data: Vec<ConfiguredTargetNode>,
    actions: Vec<(String, ActionEntryData)>,
    changed_files: Vec<ChangedFilesEntryData>,
) -> buck2_error::Result<FlatBufferBuilder<'static>> {
    // associate actions and changed files with targets when possible
    let (target_data, other_actions_data, other_changed_files) = {
        // These are in case we need to debug orphan actions or changed files
        let mut actions_data = vec![];
        let mut files_changed_data = vec![];

        let mut data: Vec<_> = data
            .into_iter()
            .map(|node| TargetData {
                node,
                actions: vec![],
                changed_files: vec![],
            })
            .collect();

        let mut node_map: BuckMutMap<String, &mut TargetData> = BuckMutMap::default();
        for node in data.iter_mut() {
            let key = node.node.label().to_string();
            node_map.insert(key, node);
        }

        // Actions
        for (target, entry) in actions.into_iter() {
            if let Some(target_data) = node_map.get_mut(&target) {
                target_data.actions.push(entry);
            } else {
                actions_data.push(entry);
            }
        }

        // Changed files
        for entry in changed_files.into_iter() {
            for target in entry.targets.into_iter() {
                if let Some(target_data) = node_map.get_mut(&target) {
                    target_data.changed_files.push(entry.path.clone());
                } else {
                    files_changed_data.push(entry.path.clone());
                }
            }
        }
        (data, actions_data, files_changed_data)
    };

    let mut builder = FlatBufferBuilder::new();

    let targets: Result<Vec<_>, _> = target_data
        .iter()
        .map(|node| target_to_fbs(&mut builder, node))
        .collect();
    let targets = builder.create_vector(&targets?);

    let other_actions: Vec<_> = other_actions_data
        .iter()
        .map(|node| action_to_fbs(&mut builder, node))
        .collect();
    let other_actions = builder.create_vector(&other_actions);

    let other_changed_files: Vec<_> = other_changed_files
        .iter()
        .map(|path| builder.create_shared_string(path))
        .collect();
    let other_changed_files = builder.create_vector(&other_changed_files);

    let build = fbs::Build::create(
        &mut builder,
        &fbs::BuildArgs {
            targets: Some(targets),
            other_actions: Some(other_actions),
            other_changed_files: Some(other_changed_files),
        },
    );
    builder.finish(build, None);
    Ok(builder)
}

fn target_to_fbs<'a>(
    builder: &'_ mut FlatBufferBuilder<'static>,
    data: &'_ TargetData,
) -> buck2_error::Result<WIPOffset<fbs::ConfiguredTargetNode<'a>>> {
    let node = &data.node;

    let actions = {
        let actions: Vec<flatbuffers::WIPOffset<fbs::Action<'_>>> = data
            .actions
            .iter()
            .map(|action| action_to_fbs(builder, action))
            .collect();
        Some(builder.create_vector(&actions))
    };

    let changed_files = {
        let changed_files: Vec<flatbuffers::WIPOffset<&str>> = data
            .changed_files
            .iter()
            .map(|path| builder.create_shared_string(path))
            .collect();
        Some(builder.create_vector(&changed_files))
    };

    // special attrs
    let name = builder.create_shared_string(&node.name());
    let target_label = get_target_label(builder, node);

    let body = node.body()?;
    let oncall = body.oncall().map(|v| builder.create_shared_string(v));
    let type_ = builder.create_shared_string(node.rule_type().name());
    let package = builder.create_shared_string(&body.buildfile_path().to_string());
    let target_configuration =
        builder.create_shared_string(&node.target_configuration().to_string());
    let execution_platform = builder.create_shared_string(&body.execution_platform()?.id());
    let deps = {
        let res = &node
            .deps()
            .iter()
            .map(|d| get_target_label(builder, d))
            .collect::<Vec<WIPOffset<fbs::ConfiguredTargetLabel>>>();
        builder.create_vector(res)
    };

    let code_pointer = body
        .root_location()
        .map(|l| fbs::CodePointerArgs {
            file_path: Some(builder.create_shared_string(&l.file)),
            line: l.line as i32,
        })
        .as_ref()
        .map(|r| fbs::CodePointer::create(builder, r));

    let srcs = body
        .get("srcs", AttrInspectOptions::DefinedOnly)
        .map(|v| match categorize(v.value) {
            AttrField::StringList(v) => v.len(),
            AttrField::StringDict(v) => v.len(),
            AttrField::Other => 0,
        })
        .unwrap_or(0) as i64;

    let target = fbs::ConfiguredTargetNode::create(
        builder,
        &fbs::ConfiguredTargetNodeArgs {
            name: Some(name),
            // special attrs
            label: Some(target_label),
            type_: Some(type_),
            deps: Some(deps),
            package: Some(package),
            oncall,
            target_configuration: Some(target_configuration),
            execution_platform: Some(execution_platform),
            srcs,
            code_pointer,
            actions,
            changed_files,
        },
    );
    Ok(target)
}

fn action_to_fbs<'a>(
    builder: &mut FlatBufferBuilder<'static>,
    action: &ActionEntryData,
) -> WIPOffset<fbs::Action<'a>> {
    let category = action
        .category
        .as_ref()
        .map(|c| builder.create_shared_string(c));
    let identifier = action
        .identifier
        .as_ref()
        .map(|c| builder.create_shared_string(c));
    let execution_kind = action
        .execution_kind
        .as_ref()
        .map(|c| builder.create_shared_string(c));
    let repros = {
        let list = action
            .repros
            .iter()
            .map(|v| builder.create_shared_string(v))
            .collect::<Vec<WIPOffset<&str>>>();
        Some(builder.create_vector(&list))
    };
    fbs::Action::create(
        builder,
        &fbs::ActionArgs {
            category,
            failed: action.failed,
            repros,
            identifier,
            input_files_bytes: action.input_files_bytes.map(|v| v as i32),
            execution_kind,
            affected_by_file_changes: action.affected_by_file_changes,
        },
    )
}

fn get_target_label<'a>(
    builder: &mut FlatBufferBuilder<'static>,
    node: &ConfiguredTargetNode,
) -> WIPOffset<fbs::ConfiguredTargetLabel<'a>> {
    let label = &node.label();
    let target_label = builder.create_shared_string(&label.unconfigured().to_string());
    let cfg = builder.create_shared_string(&label.cfg().to_string());
    let exec_cfg = label
        .exec_cfg()
        .as_ref()
        .map(|c| builder.create_shared_string(&c.to_string()));
    fbs::ConfiguredTargetLabel::create(
        builder,
        &fbs::ConfiguredTargetLabelArgs {
            target_label: Some(target_label),
            cfg: Some(cfg),
            exec_cfg,
        },
    )
}

fn categorize(a: ConfiguredAttr) -> AttrField {
    match a {
        ConfiguredAttr::List(v) => {
            let mut list = vec![];
            v.0.iter().for_each(|v| {
                match v {
                    ConfiguredAttr::String(v) => list.push(v.0.to_string()),
                    _ => list.push(
                        v.as_display_no_ctx()
                            .to_string()
                            .trim_matches('"')
                            .to_owned(),
                    ), // TODO iguridi: make a "printer_for_explain" for attrs
                }
            });
            AttrField::StringList(list)
        }
        ConfiguredAttr::Tuple(v) => {
            let mut list = vec![];
            v.0.iter().for_each(|v| {
                match v {
                    ConfiguredAttr::String(v) => list.push(v.0.to_string()),
                    _ => list.push(
                        v.as_display_no_ctx()
                            .to_string()
                            .trim_matches('"')
                            .to_owned(),
                    ), // TODO iguridi: make a "printer_for_explain" for attrs
                }
            });
            AttrField::StringList(list)
        }
        ConfiguredAttr::Dict(v) => {
            let string_pairs: Vec<_> =
                v.0.iter()
                    .map(|(k, v)| match (k, v) {
                        (ConfiguredAttr::String(k), ConfiguredAttr::String(v)) => {
                            (k.0.to_string(), v.0.to_string())
                        }
                        _ => (
                            k.as_display_no_ctx()
                                .to_string()
                                .trim_matches('"')
                                .to_owned(),
                            v.as_display_no_ctx()
                                .to_string()
                                .trim_matches('"')
                                .to_owned(),
                        ), // TODO iguridi: make a "printer_for_explain" for attrs
                    })
                    .collect();
            AttrField::StringDict(string_pairs)
        }
        ConfiguredAttr::OneOf(v, _) => categorize(*v),
        _ => AttrField::Other,
    }
}

#[cfg(test)]
mod tests {
    use buck2_core::configuration::data::ConfigurationData;
    use buck2_core::execution_types::execution::ExecutionPlatform;
    use buck2_core::execution_types::execution::ExecutionPlatformResolution;
    use buck2_core::execution_types::executor_config::CommandExecutorConfig;
    use buck2_core::package::package_relative_path::PackageRelativePath;
    use buck2_core::target::label::label::TargetLabel;
    use buck2_interpreter_for_build::call_stack::StarlarkCallStackWrapper;
    use buck2_node::attrs::arena::OwnedCoercedAttr;
    use buck2_node::attrs::attr::Attribute;
    use buck2_node::attrs::attr_type::AttrType;
    use buck2_node::attrs::coerced_path::CoercedPath;
    use buck2_node::call_stack::StarlarkCallStack;
    use buck2_util::arc_str::PackedStr;
    use dupe::Dupe;
    use starlark::codemap::FileSpan;
    use starlark::errors::Frame;
    use starlark::eval::CallStack;

    use super::*;
    pub use crate::explain_generated::explain::Build;

    #[test]
    fn test_srcs_count() {
        let data = gen_data(vec![(
            "srcs",
            Attribute::new_const(None, "", AttrType::list(AttrType::source(false))),
            OwnedCoercedAttr::new_with(|b| {
                let srcs = ["foo/bar", "foo/bar2"]
                    .iter()
                    .map(|p| {
                        b.encode_source_file(&CoercedPath::File(PackedStr::new(
                            PackageRelativePath::new(p).unwrap(),
                        )))
                    })
                    .collect::<buck2_error::Result<Vec<_>>>()?;
                b.encode_list(&srcs)
            })
            .unwrap(),
        )]);

        let fbs = gen_fbs(data, vec![], vec![]).unwrap();
        let fbs = fbs.finished_data();
        let build = flatbuffers::root::<Build>(fbs).unwrap();
        let target = build.targets().unwrap().get(0);

        assert_things(target, build);
        assert_eq!(target.srcs(), 2);
    }

    fn assert_things(target: fbs::ConfiguredTargetNode<'_>, build: fbs::Build<'_>) {
        // special attrs
        let label = target.label().unwrap();
        assert!(label.cfg().unwrap().contains("<testing>#"));
        assert_eq!(label.target_label().unwrap(), "cell//pkg:foo");
        assert_eq!(target.name(), Some("foo"));
        assert_eq!(target.type_(), Some("foo_lib"));
        assert_eq!(target.package(), Some("cell//pkg:BUCK"));
        assert_eq!(target.oncall(), None);
        assert_eq!(target.execution_platform(), Some("cell//pkg:bar"));
        assert_eq!(
            target.code_pointer().unwrap().file_path(),
            Some("cell/pkg/BUCK")
        );
        assert_eq!(target.code_pointer().unwrap().line(), 0);
        assert!(target.deps().unwrap().is_empty());

        let target2 = build.targets().unwrap().get(1);
        assert_eq!(
            target2.label().unwrap().target_label(),
            Some("cell//pkg:baz"),
        );
    }

    fn gen_data(
        attrs: Vec<(
            &str,
            buck2_node::attrs::attr::Attribute,
            buck2_node::attrs::arena::OwnedCoercedAttr,
        )>,
    ) -> Vec<ConfiguredTargetNode> {
        // Setup data
        let target_label = TargetLabel::testing_parse("cell//pkg:foo");
        let configured_target_label = target_label.configure(ConfigurationData::testing_new());

        let execution_platform_resolution = {
            let platform_label = TargetLabel::testing_parse("cell//pkg:bar");
            let platform = ExecutionPlatform::platform(
                platform_label,
                ConfigurationData::testing_new(),
                CommandExecutorConfig::testing_local(),
            );
            ExecutionPlatformResolution::new_for_testing(Some(platform), Vec::new())
        };

        let target = ConfiguredTargetNode::testing_new(
            configured_target_label,
            "foo_lib",
            execution_platform_resolution.dupe(),
            attrs,
            Some(StarlarkCallStack::new(StarlarkCallStackWrapper(
                CallStack {
                    frames: vec![Frame {
                        name: "foo".to_owned(),
                        location: Some(FileSpan::new(
                            "cell/pkg/BUCK".to_owned(),
                            "source".to_owned(),
                        )),
                    }],
                },
            ))),
        );

        let target_label2 = TargetLabel::testing_parse("cell//pkg:baz");
        let configured_target_label2 = target_label2.configure(ConfigurationData::testing_new());
        let target2 = ConfiguredTargetNode::testing_new(
            configured_target_label2,
            "foo_lib",
            execution_platform_resolution,
            vec![],
            None,
        );
        vec![target, target2]
    }
}
