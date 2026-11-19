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

//! Lowest level stream communication as JSON.
//! Because DAP debugging is hard, we write everything we see to stdout (for the protocol)
//! AND stderr (for debugging).

use std::env;
use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::io::Read;
use std::io::Write;
use std::path::PathBuf;

use serde_json::Value;

// Debugging anything through DAP is a nightmare, because VS Code doesn't surface any logs.
// Therefore, do the hacky thing of putting logs next to the binary.
fn log_file() -> PathBuf {
    let mut res = env::current_exe().unwrap();
    res.set_extension("dap.log");
    res
}

// The directory next to the binary may be read-only, so logging is best effort.
pub(crate) fn log_begin() {
    File::create(log_file()).ok();
}

pub(crate) fn log(x: &str) {
    if let Ok(mut file) = OpenOptions::new().append(true).open(log_file()) {
        file.write_all(format!("{x}\n").as_bytes()).ok();
    }
}

pub(crate) fn send(x: Value) {
    let s = x.to_string();
    log(&format!("SEND: {s}"));
    print!("Content-Length: {}\r\n\r\n{}", s.len(), s);
    io::stdout().flush().unwrap()
}

/// `None` once the client has closed its end of the pipe.
pub(crate) fn read() -> Option<Value> {
    let mut s = String::new();
    if io::stdin().read_line(&mut s).unwrap() == 0 {
        return None;
    }
    let len: usize = s
        .strip_prefix("Content-Length: ")
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    io::stdin().read_line(&mut s).unwrap();
    let mut res = vec![0u8; len];
    io::stdin().lock().read_exact(&mut res).unwrap();
    let s = String::from_utf8_lossy(&res);
    log(&format!("RECV: {s}"));
    Some(serde_json::from_str(&s).unwrap())
}
