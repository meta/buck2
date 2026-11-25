/*
 * Copyright 2018 The Starlark in Rust Authors.
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

use crate::environment::GlobalsBuilder;
use crate::stdlib::funcs::min_max::register_min_max;
use crate::stdlib::funcs::other::register_other;
use crate::stdlib::funcs::zip::register_zip;
use crate::values::bool::globals::register_bool;
use crate::values::bytes::StarlarkBytes;
use crate::values::float::StarlarkFloat;
use crate::values::int::view::IntView;
use crate::values::none::globals::register_none;
use crate::values::range::Range;
use crate::values::string::globals::register_str;
use crate::values::tuple::globals::register_tuple;
use crate::values::types::dict::globals::register_dict;
use crate::values::types::list::globals::register_list;
use crate::values::types::num::globals::register_num;

pub(crate) fn register_globals(globals: &mut GlobalsBuilder) {
    globals.type_symbol::<StarlarkBytes>();
    register_list(globals);
    register_tuple(globals);
    register_dict(globals);
    register_bool(globals);
    register_none(globals);
    register_str(globals);
    globals.type_symbol::<Range>();
    globals.type_symbol::<IntView>();
    register_num(globals);
    globals.type_symbol::<StarlarkFloat>();
    register_min_max(globals);
    register_zip(globals);
    register_other(globals);
}
