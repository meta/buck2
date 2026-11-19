/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use async_trait::async_trait;
use buck2_downward_api::DownwardApi;
use buck2_hash::BuckMutMap;
use tracing::Level;

pub struct BuckTestDownwardApi;

#[async_trait]
impl DownwardApi for BuckTestDownwardApi {
    async fn console(&self, _level: Level, msg: String) -> buck2_error::Result<()> {
        // TODO(brasselsprouts): use the level and hook it up with our superconsole
        eprintln!("{}", msg);
        Ok(())
    }

    async fn log(&self, _level: Level, _msg: String) -> buck2_error::Result<()> {
        unimplemented!("TODO(bobyf)")
    }

    async fn external(&self, _data: BuckMutMap<String, String>) -> buck2_error::Result<()> {
        unimplemented!("need buck event stream to implement")
    }
}
