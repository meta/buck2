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

use crate as starlark;
use crate::values::Freeze;
use crate::values::FreezeError;
use crate::values::FreezeResult;
use crate::values::Freezer;

struct FreezeSentinel {
    frozen: bool,
}

impl<'v> Freeze<'v> for FreezeSentinel {
    type Frozen<'fv> = Self;

    fn freeze<'fv>(self, _: &Freezer<'v, 'fv>) -> FreezeResult<Self> {
        assert!(!self.frozen);
        Ok(Self { frozen: true })
    }
}

#[derive(Freeze)]
#[freeze(validator = check_froze_before_validating)]
struct Test {
    sentinel: FreezeSentinel,
}

fn check_froze_before_validating(test: &Test) -> anyhow::Result<()> {
    // Accessing fields on a Starlark value before we call freeze() on it may fail (because we
    // read the forward not what it points to), so we check that validators receive frozen data.
    assert!(test.sentinel.frozen);
    Ok(())
}

#[test]
fn test() -> anyhow::Result<()> {
    let t = Test {
        sentinel: FreezeSentinel { frozen: false },
    };
    Freezer::testing_temp(|_heap, freezer| t.freeze(freezer).map(drop))?;
    Ok(())
}
