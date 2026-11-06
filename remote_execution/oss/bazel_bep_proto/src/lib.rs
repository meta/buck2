/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

pub mod blaze {
    tonic::include_proto!("blaze");

    pub mod invocation_policy {
        tonic::include_proto!("blaze.invocation_policy");
    }

    pub mod strategy_policy {
        tonic::include_proto!("blaze.strategy_policy");
    }
}

pub mod build_event_stream {
    tonic::include_proto!("build_event_stream");
}

pub mod command_line {
    tonic::include_proto!("command_line");
}

pub mod devtools {
    pub mod build {
        pub mod lib {
            pub mod packages {
                pub mod metrics {
                    tonic::include_proto!("devtools.build.lib.packages.metrics");
                }
            }
        }
    }
}

pub mod failure_details {
    tonic::include_proto!("failure_details");
}

pub mod options {
    tonic::include_proto!("options");
}
