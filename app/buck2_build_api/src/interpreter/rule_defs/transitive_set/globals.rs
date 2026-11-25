/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use starlark::environment::GlobalsBuilder;

use crate::interpreter::rule_defs::transitive_set::FrozenTransitiveSetDefinition;
use crate::interpreter::rule_defs::transitive_set::transitive_set::TransitiveSet;
use crate::interpreter::rule_defs::transitive_set::transitive_set_args_projection::TransitiveSetArgsProjection;
use crate::interpreter::rule_defs::transitive_set::transitive_set_json_projection::TransitiveSetJsonProjection;
use crate::interpreter::rule_defs::transitive_set::traversal::TransitiveSetProjectionTraversal;
use crate::interpreter::rule_defs::transitive_set::traversal::TransitiveSetTraversal;

pub fn register_transitive_set_types(globals: &mut GlobalsBuilder) {
    globals.type_symbol::<TransitiveSet>();
    globals.type_symbol::<TransitiveSetArgsProjection>();
    globals.type_symbol_as::<FrozenTransitiveSetDefinition>("TransitiveSetDefinition");
    globals.type_symbol::<TransitiveSetJsonProjection>();
    globals.type_symbol::<TransitiveSetTraversal>();
    globals.type_symbol::<TransitiveSetProjectionTraversal>();
}
