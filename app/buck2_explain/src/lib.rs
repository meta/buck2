/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fs;
use std::io::Cursor;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use buck2_core::buck2_env;
use buck2_fs::paths::abs_path::AbsPathBuf;

#[allow(unsafe_op_in_unsafe_fn)]
#[allow(unused_imports)]
#[allow(unused_extern_crates)]
#[allow(clippy::extra_unused_lifetimes)]
mod explain_generated;
mod flatbuffers;
mod output_format_flatbuffers;
#[allow(unsafe_op_in_unsafe_fn)]
#[allow(unused_imports)]
#[allow(unused_extern_crates)]
#[allow(clippy::extra_unused_lifetimes)]
mod output_format_generated;
use buck2_common::manifold::Bucket;
use buck2_common::manifold::ManifoldClient;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_query::query::environment::QueryTarget;
use buck2_query::query::syntax::simple::eval::set::TargetSet;

const HTML_PLACEHOLDER: &str = "XXDATAXX";

#[derive(Default)]
pub struct ActionEntryData {
    // TODO iguridi: add more interesting action fields e.g. duration
    pub category: Option<String>,
    pub identifier: Option<String>,
    pub failed: bool,
    pub repros: Vec<String>,
    pub execution_kind: Option<String>,
    pub input_files_bytes: Option<u64>,
    pub affected_by_file_changes: bool,
}

pub struct ChangedFilesEntryData {
    pub path: String,
    pub targets: Vec<String>,
}

pub async fn main(
    data: Vec<ConfiguredTargetNode>,
    executed_actions: Vec<(String, ActionEntryData)>,
    changed_files: Vec<ChangedFilesEntryData>,
    output: Option<&AbsPathBuf>,
    fbs_dump: Option<&AbsPathBuf>,
    manifold_path: Option<&str>,
) -> buck2_error::Result<()> {
    let fbs = flatbuffers::gen_fbs(data, executed_actions, changed_files)?;

    let fbs = fbs.finished_data();

    let html_out = inline_fbs(fbs, fbs_dump, include_str!("explain.html"))?;

    let mut cursor = Cursor::new(html_out.as_bytes());

    if let Some(o) = output {
        fs::write(o, &html_out)?
    };

    if let Some(p) = manifold_path {
        // TODO iguridi: write and upload concurrently
        let manifold = ManifoldClient::new().await?;

        manifold
            .read_and_upload(Bucket::EVENT_LOGS, p, Default::default(), &mut cursor)
            .await?;
    }

    Ok(())
}

pub fn output_format<T: QueryTarget>(data: TargetSet<T>) -> buck2_error::Result<String> {
    let fbs = output_format_flatbuffers::gen_fbs(data)?;
    let fbs = fbs.finished_data();
    let html_out = inline_fbs(fbs, None, include_str!("output_format.html"))?;
    Ok(html_out)
}

pub fn inline_fbs(
    fbs: &[u8],
    fbs_dump: Option<&AbsPathBuf>,
    html_in: &str,
) -> buck2_error::Result<String> {
    let base64 = STANDARD.encode(fbs);
    // For dev purposes, dump the base64 encoded flatbuffer to a file
    if let Some(fbs_dump) = fbs_dump {
        fs::write(fbs_dump, &base64)?;
    }
    let env = buck2_env!("BUCK2_DUMP_FBS", applicability = testing)?;
    if let Some(fbs_dump) = env {
        fs::write(fbs_dump, &base64)?;
    }
    if !html_in.contains(HTML_PLACEHOLDER) {
        return Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::Explain,
            "HTML template is not valid"
        ));
    }

    Ok(html_in.replace(HTML_PLACEHOLDER, &base64))
}
