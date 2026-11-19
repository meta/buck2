/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_query::query::environment::QueryTarget;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use flatbuffers::FlatBufferBuilder;
use flatbuffers::WIPOffset;

mod fbs {
    pub use crate::output_format_generated::output_format::Build;
    pub use crate::output_format_generated::output_format::BuildArgs;
    pub use crate::output_format_generated::output_format::Node;
    pub use crate::output_format_generated::output_format::NodeArgs;
}

pub(crate) fn gen_fbs<T: QueryTarget>(
    data: TargetSet<T>,
) -> buck2_error::Result<FlatBufferBuilder<'static>> {
    let mut builder = FlatBufferBuilder::new();

    let targets: Result<Vec<_>, _> = data
        .iter()
        .map(|node| target_to_fbs(&mut builder, node))
        .collect();
    let mut targets = targets?;
    targets.reverse(); // so root node is first
    let targets = builder.create_vector(&targets);

    let build = fbs::Build::create(
        &mut builder,
        &fbs::BuildArgs {
            targets: Some(targets),
        },
    );
    builder.finish(build, None);
    Ok(builder)
}

fn target_to_fbs<'a, T: QueryTarget>(
    builder: &'_ mut FlatBufferBuilder<'static>,
    node: &'_ T,
) -> buck2_error::Result<WIPOffset<fbs::Node<'a>>> {
    let target_label = builder.create_shared_string(&node.node_key().to_string());

    let type_ = builder.create_shared_string(node.rule_type().as_ref());
    let deps = {
        let res = &node
            .deps()
            .map(|d| builder.create_shared_string(&d.to_string()))
            .collect::<Vec<WIPOffset<&'_ str>>>();
        builder.create_vector(res)
    };

    let target = fbs::Node::create(
        builder,
        &fbs::NodeArgs {
            label: Some(target_label),
            type_: Some(type_),
            deps: Some(deps),
        },
    );
    Ok(target)
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
    use buck2_node::nodes::configured::ConfiguredTargetNode;
    use buck2_query::query::syntax::simple::eval::set::TargetSet;
    use buck2_util::arc_str::PackedStr;
    use dupe::Dupe;
    use starlark::codemap::FileSpan;
    use starlark::errors::Frame;
    use starlark::eval::CallStack;

    use super::*;
    pub use crate::output_format_generated::output_format::Build;

    #[test]
    fn test_targets() {
        let data: TargetSet<ConfiguredTargetNode> = gen_data(vec![(
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

        let fbs = gen_fbs(data).unwrap();
        let fbs = fbs.finished_data();
        let build = flatbuffers::root::<Build>(fbs).unwrap();
        let target = build.targets().unwrap().get(0);

        assert_things(target, build);
    }

    fn assert_things(target: fbs::Node<'_>, build: fbs::Build<'_>) {
        assert!(
            target
                .label()
                .unwrap()
                .contains("cell//pkg:baz (<testing>#")
        );
        assert_eq!(target.type_(), Some("foo_lib"));
        assert!(target.deps().unwrap().is_empty());

        let target2 = build.targets().unwrap().get(1);
        assert!(
            target2
                .label()
                .unwrap()
                .contains("cell//pkg:foo (<testing>#")
        );
    }

    fn gen_data(
        attrs: Vec<(
            &str,
            buck2_node::attrs::attr::Attribute,
            buck2_node::attrs::arena::OwnedCoercedAttr,
        )>,
    ) -> TargetSet<ConfiguredTargetNode> {
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

        let mut set = TargetSet::<ConfiguredTargetNode>::new();
        set.insert(target);
        set.insert(target2);
        set
    }
}
