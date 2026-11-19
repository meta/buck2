/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use debugserver_types as ds;

use crate::dap::library::events::*;
use crate::dap::library::requests::*;
use crate::dap::library::stream::*;

pub(crate) struct DapService {
    _private: (),
}

impl DapService {
    pub(crate) fn run<T: DebugServer>(f: impl FnOnce(Client) -> T) {
        server(f(Client::new()));
    }
}

fn server(dap: impl DebugServer) {
    log_begin();

    // Because of the eval we're running in, we probably can't see panics.
    // So mirror them to the log file.
    let orig_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            log(&format!("Panic occurred: {s:?}"));
        } else {
            log("Panic occurred: Unknown message");
        }
        orig_hook(panic_info);
    }));

    log("DEBUG ADAPTER STARTING");
    while let Some(recv) = read() {
        let r: ds::Request = serde_json::from_value(recv).unwrap();
        assert_eq!(r.type_, "request");
        let resp = dispatch(&dap, &r);
        send(serde_json::to_value(resp).unwrap());

        if r.command == "disconnect" {
            break;
        }
    }
    log("DEBUG ADAPTER STOPPING");
}
