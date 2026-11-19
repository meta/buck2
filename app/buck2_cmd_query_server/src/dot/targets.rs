/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_hash::BuckMutInsertionMap;
use buck2_query::query::environment::AttrFmtOptions;
use buck2_query::query::environment::QueryTarget;
use buck2_query::query::environment::QueryTargets;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use regex::RegexSet;

use crate::dot::DotDigraph;
use crate::dot::DotEdge;
use crate::dot::DotNode;
use crate::dot::DotNodeAttrs;
use crate::query::query_target_ext::QueryCommandTarget;

pub(crate) struct DotTargetGraphNode<'a, T: QueryTarget>(&'a T, &'a DotTargetGraph<T>);

/// A simple adapter for creating a DotDiGraph for a TargetSet.
pub(crate) struct DotTargetGraph<T: QueryTarget> {
    pub(crate) targets: TargetSet<T>,
    pub(crate) attributes: Option<RegexSet>,
}

impl<'a, T: QueryCommandTarget> DotDigraph<'a> for DotTargetGraph<T> {
    type Node = DotTargetGraphNode<'a, T>;

    fn name(&self) -> &str {
        "result_graph"
    }

    fn for_each_node<F: FnMut(&Self::Node) -> buck2_error::Result<()>>(
        &'a self,
        mut f: F,
    ) -> buck2_error::Result<()> {
        if self.attributes.is_some() {
            T::prefetch_bodies(&self.targets)?;
        }
        for node in self.targets.iter() {
            f(&DotTargetGraphNode(node, self))?;
        }
        Ok(())
    }

    fn for_each_edge<F: FnMut(&DotEdge) -> buck2_error::Result<()>>(
        &'a self,
        node: &Self::Node,
        mut f: F,
    ) -> buck2_error::Result<()> {
        for dep in node.0.deps() {
            // Only include edges to other nodes within the subgraph.
            if self.targets.contains(dep) {
                f(&DotEdge {
                    from: &node.0.node_key().to_string(),
                    to: &dep.to_string(),
                })?;
            }
        }
        Ok(())
    }
}

impl<T: QueryCommandTarget> DotNode for DotTargetGraphNode<'_, T> {
    fn attrs(&self) -> buck2_error::Result<DotNodeAttrs> {
        let extra = match &self.1.attributes {
            Some(attr_regex) => {
                let mut extra = BuckMutInsertionMap::new();
                QueryTargets::for_all_attrs::<buck2_error::Error, _, _>(
                    self.0,
                    |attr_name, attr_value| {
                        if attr_regex.is_match(attr_name) {
                            extra.insert(
                                format!("buck_{attr_name}"),
                                format!(
                                    "{}",
                                    self.0.attr_display(
                                        attr_value,
                                        AttrFmtOptions {
                                            exclude_quotes: true,
                                        },
                                    )
                                ),
                            );
                        }
                        Ok(())
                    },
                )?;
                extra
            }
            None => BuckMutInsertionMap::new(),
        };
        Ok(DotNodeAttrs {
            style: Some("filled".to_owned()),
            color: Some("#DFECDF".to_owned()),
            extra,
            ..DotNodeAttrs::default()
        })
    }

    fn id(&self) -> String {
        self.0.node_key().to_string()
    }
}
