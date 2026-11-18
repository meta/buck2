/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//!
//! 'Provider's are advertised data exposed by rules. These contain typed data
//! that can be requested. The data may be materialized lazily or eagerly.
//!
//! 'ProvidersLabel's are labels/keys that uniquely map to a specific set of
//! 'Provider's exposed by a rule. There are two versions of these labels, one
//! with 'Configuration's attached, and one without.
//!
//! For example, one might imagine a `thrift_rule` exposing itself as various
//! languages, for which specific languages can be referred to via depends as
//! `some_rule[java]` or `some_rule[cxx]`.

pub mod id;
pub mod label;
