/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! This crate defines the interactions between Buck and the test executor via a test protocol.
//!
//! # Test Executor Protocol
//! The protocol defines a bi-directional set of api calls that allows buck and the test executor
//! to interact. Buck will provide a stream of test specifications for each test rule to the
//! test executor as soon as the rule is done building. The test executor will then provide a
//! stream of execution plans for Buck to execute as actions. The results of execution will be
//! passed back to the test executor. When the test executor has formed the conclusion of a test,
//! it will signal the test result back to Buck. The interaction continues until Buck signals that
//! there are no more test rules, and that the test executor signals there's no more tasks to
//! execute.
//!
//! External test executors are expected to implement the trait `TestExecutor`. Test executors
//! will be able to interact with Buck via the `TestOrchestrator` trait.

pub mod convert;
pub mod data;
pub mod grpc;
pub mod protocol;
