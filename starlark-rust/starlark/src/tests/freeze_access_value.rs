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

use crate::values::Freeze;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::Value;
use crate::values::list::AllocList;
use crate::values::list::ListRef;

struct Test<V> {
    field: V,
}

impl<'v> Freeze<'v> for Test<Value<'v>> {
    type Frozen<'fv> = Test<Value<'fv>>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let test = Test {
            field: self.field.freeze(freezer)?,
        };
        let members = ListRef::from_value(test.field).unwrap();
        assert_eq!(members[0].unpack_num().unwrap().as_int().unwrap(), 1);
        assert_eq!(members[1].unpack_num().unwrap().as_int().unwrap(), 2);
        Ok(test)
    }
}

#[test]
fn test() -> anyhow::Result<()> {
    Freezer::testing_temp(|heap, freezer| {
        let list = heap.alloc(AllocList(vec![1i32, 2i32])).to_value();
        let t = Test { field: list };
        list.freeze(freezer)?;
        t.freeze(freezer)?;
        anyhow::Ok(())
    })
}
