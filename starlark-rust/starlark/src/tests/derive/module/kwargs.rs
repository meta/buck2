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

use starlark_derive::starlark_module;
use starlark_map::small_map::SmallMap;

use crate as starlark;
use crate::assert::Assert;
use crate::environment::GlobalsBuilder;
use crate::values::UnpackValue;
use crate::values::ValueOf;
use crate::values::dict::DictType;
use crate::values::types::int::view::IntView;

#[starlark_module]
fn test_kwargs_module(globals: &mut GlobalsBuilder) {
    fn pos_kwargs<'v>(
        #[starlark(require = pos)] a: u32,
        #[starlark(require = pos)] b: bool,
        #[starlark(kwargs)] kwargs: ValueOf<'v, DictType<String, IntView<'v>>>,
    ) -> anyhow::Result<String> {
        let kwargs = kwargs
            .try_items::<String, IntView>()
            .map(|entry| {
                let (k, v) = entry?;
                Ok((k, u64::unpack_value_err(v.to_value())?))
            })
            .collect::<crate::Result<SmallMap<_, _>>>()
            .map_err(|e| e.into_anyhow())?;
        Ok(format!("a={a} b={b} kwargs={kwargs:?}"))
    }

    fn pos_named_kwargs<'v>(
        #[starlark(require = pos)] a: u32,
        #[starlark(require = named)] b: bool,
        #[starlark(kwargs)] kwargs: ValueOf<'v, DictType<String, IntView<'v>>>,
    ) -> anyhow::Result<String> {
        let kwargs = kwargs
            .try_items::<String, IntView>()
            .map(|entry| {
                let (k, v) = entry?;
                Ok((k, u64::unpack_value_err(v.to_value())?))
            })
            .collect::<crate::Result<SmallMap<_, _>>>()
            .map_err(|e| e.into_anyhow())?;
        Ok(format!("a={a} b={b} kwargs={kwargs:?}"))
    }
}

#[test]
fn test_kwargs() {
    let mut a = Assert::new();
    a.globals_add(test_kwargs_module);
    a.eq(
        r#"'a=1 b=true kwargs={"x": 3}'"#,
        "pos_kwargs(1, True, x=3)",
    );
    a.eq(
        r#"'a=1 b=true kwargs={"x": 3}'"#,
        "pos_named_kwargs(1, b=True, x=3)",
    );
}
