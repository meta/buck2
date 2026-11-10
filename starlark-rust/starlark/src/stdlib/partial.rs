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

use std::fmt;
use std::fmt::Display;

use allocative::Allocative;
use hashbrown::HashTable;
use starlark_derive::NoSerialize;
use starlark_derive::starlark_module;
use starlark_derive::starlark_value;
use starlark_syntax::slice_vec_ext::SliceExt;
use starlark_syntax::slice_vec_ext::VecExt;
use starlark_syntax::value_error;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::collections::symbol::symbol::Symbol;
use crate::environment::GlobalsBuilder;
use crate::eval::Arguments;
use crate::eval::Evaluator;
use crate::eval::runtime::arguments::ArgNames;
use crate::eval::runtime::arguments::ArgumentsFull;
use crate::eval::runtime::rust_loc::rust_loc;
use crate::pagable::starlark_deserialize::StarlarkDeserialize;
use crate::pagable::starlark_deserialize::StarlarkDeserializeContext;
use crate::pagable::starlark_serialize::StarlarkSerialize;
use crate::pagable::starlark_serialize::StarlarkSerializeContext;
use crate::starlark_complex_value;
use crate::values::Freeze;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::Heap;
use crate::values::StarlarkValue;
use crate::values::StringValue;
use crate::values::Trace;
use crate::values::Value;
use crate::values::ValueTyped;
use crate::values::dict::DictRef;
use crate::values::function::FUNCTION_TYPE;
use crate::values::types::tuple::value::Tuple;

/// Build an index keyed by each symbol's hash, mapping to its position in the
/// input. Shared between [`partial`] construction and
/// [`PartialGen::starlark_deserialize`], where `names_index` is skipped on the
/// wire and rebuilt from the symbols.
fn build_names_index<'a>(symbols: impl IntoIterator<Item = &'a Symbol>) -> HashTable<usize> {
    let symbols: Vec<&Symbol> = symbols.into_iter().collect();
    let mut names_index = HashTable::with_capacity(symbols.len());
    for (i, k) in symbols.iter().enumerate() {
        names_index.insert_unique(k.hash(), i, |i| symbols[*i].hash());
    }
    names_index
}

#[starlark_module]
pub fn partial(builder: &mut GlobalsBuilder) {
    /// Construct a partial application. In almost all cases it is simpler to use a `lamdba`.
    fn partial<'v>(
        #[starlark(require = pos)] func: Value<'v>,
        #[starlark(args)] args: Value<'v>,
        #[starlark(kwargs)] kwargs: DictRef<'v>,
        heap: Heap<'v>,
    ) -> anyhow::Result<ValueTyped<'v, Partial<'v>>> {
        debug_assert!(Tuple::from_value(args).is_some());
        let names: Vec<_> = kwargs
            .keys()
            .map(|x| {
                let x = StringValue::new(x).unwrap();
                (
                    // We duplicate string here.
                    // If this becomes hot, we should do better.
                    Symbol::new_hashed(x.as_str_hashed()),
                    x,
                )
            })
            .collect();
        let names_index = build_names_index(names.iter().map(|(s, _)| s));
        Ok(heap.alloc_representation(Partial {
            func,
            pos: args,
            named: kwargs.values().collect(),
            names,
            names_index,
        }))
    }
}

#[derive(Debug, Trace, NoSerialize, ProvidesStaticType, Allocative)]
#[repr(C)]
struct Partial<'v> {
    func: Value<'v>,
    // Always references a tuple.
    pos: Value<'v>,
    named: Vec<Value<'v>>,
    names: Vec<(Symbol, StringValue<'v>)>,
    names_index: HashTable<usize>,
}

impl<'v> Partial<'v> {
    fn pos_content(&self) -> &'v [Value<'v>] {
        Tuple::from_value(self.pos).unwrap().content()
    }
}

impl<'v> Display for Partial<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "partial({}, *[", self.func)?;
        for (i, v) in self.pos_content().iter().enumerate() {
            if i != 0 {
                write!(f, ",")?;
            }
            v.fmt(f)?;
        }
        write!(f, "], **{{")?;
        for (i, (k, v)) in self.names.iter().zip(self.named.iter()).enumerate() {
            if i != 0 {
                write!(f, ",")?;
            }
            write!(f, "{}:", k.0.as_str())?;
            v.to_value().fmt(f)?;
        }
        write!(f, "}})")
    }
}

starlark_complex_value!(Partial);

impl<'v> Freeze<'v> for Partial<'v> {
    type Frozen<'fv> = Partial<'fv>;
    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        Ok(Partial {
            func: self.func.freeze(freezer)?,
            pos: freezer.freeze(self.pos)?,
            named: self.named.try_map(|x| x.freeze(freezer))?,
            names: self
                .names
                .into_try_map(|(s, x)| Ok((s, x.freeze(freezer)?)))?,
            names_index: self.names_index,
        })
    }
}

impl<'v> StarlarkSerialize for Partial<'v> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        self.func.starlark_serialize(ctx)?;
        self.pos.starlark_serialize(ctx)?;
        self.named.starlark_serialize(ctx)?;
        self.names.starlark_serialize(ctx)?;
        // names_index: skipped, rebuilt from names on deserialize.
        Ok(())
    }
}

impl<'v> StarlarkDeserialize<'v> for Partial<'v> {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'v>,
    ) -> crate::Result<Self> {
        let func = Value::starlark_deserialize(ctx)?;
        let pos = Value::starlark_deserialize(ctx)?;
        let named = Vec::<Value>::starlark_deserialize(ctx)?;
        let names: Vec<(Symbol, StringValue)> = Vec::starlark_deserialize(ctx)?;
        // Rebuild names_index from names.
        let names_index = build_names_index(names.iter().map(|(s, _)| s));
        Ok(Partial {
            func,
            pos,
            named,
            names,
            names_index,
        })
    }
}

#[starlark_value(type = FUNCTION_TYPE)]
impl<'v> StarlarkValue<'v> for Partial<'v> {
    fn name_for_call_stack(&self, _me: Value<'v>) -> String {
        "partial".to_owned()
    }

    fn invoke(
        &self,
        _me: Value<'v>,
        args: &Arguments<'v, '_>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<Value<'v>> {
        // apply the partial arguments first, then the remaining arguments I was given

        let self_pos = self.pos_content();

        for (symbol, _) in args.0.names.names() {
            if self
                .names_index
                .find(symbol.hash(), |i| &self.names[*i].0 == symbol)
                .is_some()
            {
                return Err(value_error!(
                    "partial() got multiple values for argument `{}`",
                    symbol.as_str(),
                ));
            }
        }

        eval.alloca_concat(self_pos, args.0.pos, |pos, eval| {
            eval.alloca_concat(&self.named, args.0.named, |named, eval| {
                eval.alloca_concat(&self.names, args.0.names.names(), |names, eval| {
                    let params = Arguments(ArgumentsFull {
                        pos,
                        named,
                        names: ArgNames::new_unique(names),
                        args: args.0.args,
                        kwargs: args.0.kwargs,
                    });
                    self.func
                        .to_value()
                        .invoke_with_loc(Some(rust_loc!().at()), &params, eval)
                })
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::assert;

    fn eq(expected: &str, expr: &str) {
        let sum = r#"
def sum(a, b, *args, **kwargs):
    # print("a=%s b=%s args=%s kwargs=%s" % (a, b, args, kwargs))
    # TODO(nga): fix typecheck.
    args = noop((a, b)) + args
    return [args, kwargs]
"#;

        assert::eq(expected, &format!("{sum}{expr}"));
    }

    #[test]
    fn test_simple() {
        eq(
            "[(1, 2, 3), {'other': True, 'third': None}]",
            "(partial(sum, 1, other=True))(2, 3, third=None)",
        );
    }

    #[test]
    fn test_star_to_partial() {
        eq(
            "[(1, 2, 3), {'other': True, 'third': None}]",
            "(partial(sum, *[1], **{'other': True}))(2, 3, third=None)",
        );
    }

    #[test]
    fn test_start_to_returned_func() {
        eq(
            "[(1, 2, 3), {'other': True, 'third': None}]",
            "(partial(sum, other=True))(*[1, 2, 3], **{'third': None})",
        );
    }

    #[test]
    fn test_no_args_to_partial() {
        eq(
            "[(1, 2, 3), {'other': True, 'third': None}]",
            "(partial(sum))(1, 2, 3, third=None, **{'other': True})",
        );
    }

    #[test]
    fn test_typecheck_bug() {
        assert::pass(
            r#"
def accept_callable(f: typing.Callable): pass

def test():
    accept_callable(partial(list, []))
"#,
        );
    }
}
