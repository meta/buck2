/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Write;

#[cfg(fbcode_build)]
use buck2_explain::output_format;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_query::query::environment::QueryTarget;
use buck2_query::query::syntax::simple::eval::set::TargetSet;

#[allow(dead_code)]
pub(crate) struct HtmlTargetGraph {
    pub(crate) targets: TargetSet<ConfiguredTargetNode>,
    // TODO iguridi: add attributes
}

pub(crate) struct Html {}

impl Html {
    pub(crate) async fn render<W: Write, T: QueryTarget>(
        graph: TargetSet<T>,
        mut w: W,
        trace_id: String,
    ) -> buck2_error::Result<()> {
        let res;
        #[cfg(fbcode_build)]
        {
            use std::io::Cursor;

            use buck2_common::manifold::Bucket;
            use buck2_common::manifold::ManifoldClient;

            let html_out = output_format(graph)?;
            let mut cursor = Cursor::new(html_out.as_bytes());
            let manifold_path = format!("flat/{trace_id}-graph.html");
            let manifold = ManifoldClient::new().await?;

            manifold
                .read_and_upload(
                    Bucket::EVENT_LOGS,
                    &manifold_path,
                    Default::default(),
                    &mut cursor,
                )
                .await?;
            res = format!(
                "\nView html in your browser: https://interncache-all.fbcdn.net/manifold/buck2_logs/{manifold_path} (requires VPN/lighthouse)\n"
            );
        }
        #[cfg(not(fbcode_build))]
        {
            let _unused = (graph, trace_id);
            res = "Not implemented".to_owned();
        }

        writeln!(w, "{res}")?;
        Ok(())
    }
}
