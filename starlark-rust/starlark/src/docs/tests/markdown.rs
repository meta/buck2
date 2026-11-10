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

use allocative::Allocative;
use derive_more::Display;
use itertools::Itertools;
use serde::Serialize;
use starlark::starlark_simple_value;
use starlark_derive::NoSerialize;
use starlark_derive::StarlarkPagable;
use starlark_derive::starlark_module;
use starlark_derive::starlark_value;
use starlark_syntax::golden_test_template::golden_test_template;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::assert;
use crate::docs::DocItem;
use crate::docs::DocMember;
use crate::docs::DocType;
use crate::docs::markdown::render_doc_item_no_link;
use crate::docs::multipage::DocModuleInfo;
use crate::docs::multipage::render_markdown_multipage;
use crate::environment::Globals;
use crate::environment::GlobalsBuilder;
use crate::environment::Methods;
use crate::environment::MethodsBuilder;
use crate::values::Heap;
use crate::values::StarlarkValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::dict::DictType;
use crate::values::list::ListType;
use crate::values::none::NoneType;
use crate::values::tuple::TupleType;
use crate::values::types::int::view::IntView;

fn docs_golden_test(test_file_name: &str, doc: DocItem) -> String {
    assert!(test_file_name.ends_with(".golden.md"));
    assert!(!test_file_name.contains('/'));

    let output = render_doc_item_no_link("name", &doc);

    golden_test_template(&format!("src/docs/tests/golden/{test_file_name}"), &output);

    output
}

const STARLARK_CODE: &str = r#"
"""
This is the summary of the module's docs

Some extra details can go here,
    and indentation is kept as expected
"""

def f1(a, b: str, c: int = 5, *, d: str = "some string", **kwargs) -> list[str]:
    """
    Summary line goes here

    Args:
        a: The docs for a
        b: The docs for b
        c: The docs for c, but these
           go onto two lines
        **kwargs: Docs for the keyword args

    Returns:
        A string repr of the args
    """
    return [str((a, b, c, d, repr(kwargs)))]

def f2(a, *args: list[str]):
    """
    This is a function with *args, and no return type

    Args:
        *args: Only doc this arg
    """
    return None

def f3(a: str) -> str:
    return a

def f4(a: str) -> str:
    """ This is a docstring with no 'Args:' section """
    return a

# Not public, so shouldn't show up
def _do_not_export():
    pass
"#;

#[derive(
    Debug,
    derive_more::Display,
    ProvidesStaticType,
    Allocative,
    NoSerialize,
    StarlarkPagable
)]
#[display("magic")]
struct Magic;

starlark_simple_value!(Magic);

#[starlark_value(type = "magic")]
impl<'v> StarlarkValue<'v> for Magic {}

/// These are where the module docs go
#[starlark_module]
#[starlark_types(Obj as Obj)]
fn module(builder: &mut GlobalsBuilder) {
    const MAGIC: i32 = 42;

    /// Docs for func1
    ///
    /// # Arguments
    ///     * `foo`: Docs for foo
    ///
    /// # Returns
    /// The string 'func1'
    fn func1(foo: String) -> anyhow::Result<String> {
        let _ignore = foo;
        Ok("func1".to_owned())
    }

    fn func2() -> anyhow::Result<String> {
        Ok("func2".to_owned())
    }

    /// A function with only positional arguments.
    ///
    /// And a slightly longer description. With some example code:
    ///
    /// ```python
    /// Magic(1)
    /// ```
    ///
    /// And some assertions:
    ///
    /// ```rust
    /// # starlark::assert::all_true(r#"
    /// 1 == 1
    /// # "#);
    /// ```
    #[starlark(as_type = Magic)]
    fn Magic(
        #[starlark(require = pos)] a1: i32,
        #[starlark(require = pos)] a2: Option<i32>,
        #[starlark(require = pos, default = 1)] step: i32,
    ) -> anyhow::Result<String> {
        let _ = (a1, a2, step);
        Ok("func3".to_owned())
    }

    fn with_defaults<'v>(
        #[starlark(default = ValueOf::empty_list())] explicit_default: ValueOf<
            'v,
            ListType<String>,
        >,
        hidden_default: Option<ValueOf<'v, ListType<String>>>,
        #[starlark(default = "my_default")] string_default: &str,
    ) -> anyhow::Result<NoneType> {
        let _unused = (explicit_default, hidden_default, string_default);
        Ok(NoneType)
    }

    fn pos_either_named<'v>(
        #[starlark(require = pos)] a: i32,
        b: i32,
        #[starlark(require = named)] c: i32,
        #[starlark(require = named, default = 0, doc_hidden)] undocumented: i32,
        heap: Heap<'v>,
    ) -> anyhow::Result<ValueOf<'v, Magic>> {
        let _unused = (a, b, c, undocumented);
        Ok(ValueOf::alloc(heap, Magic))
    }
}

#[starlark_module]
fn submodule(builder: &mut GlobalsBuilder) {
    fn notypes<'v>(a: Value<'v>) -> anyhow::Result<Value<'v>> {
        Ok(a)
    }

    fn starlark_args<'v>(
        #[starlark(args)] args: ValueOf<'v, TupleType<String>>,
    ) -> anyhow::Result<NoneType> {
        let _ignore = args;
        Ok(NoneType)
    }

    fn starlark_kwargs<'v>(
        #[starlark(kwargs)] kwargs: ValueOf<'v, DictType<String, IntView<'v>>>,
    ) -> anyhow::Result<NoneType> {
        let _ignore = kwargs;
        Ok(NoneType)
    }

    fn new_obj<'v>(heap: Heap<'v>) -> anyhow::Result<ValueOf<'v, Obj>> {
        Ok(ValueOf::alloc(heap, Obj))
    }
}

fn get_globals() -> Globals {
    GlobalsBuilder::new()
        .with(module)
        .with_namespace("submod", submodule)
        .build()
}

#[derive(
    ProvidesStaticType,
    Debug,
    Display,
    Allocative,
    Serialize,
    StarlarkPagable
)]
#[display("obj")]
struct Obj;

starlark_simple_value!(Obj);

starlark::methods_static!(OBJ_METHODS = object);

#[starlark_value(type = "obj")]
impl<'v> StarlarkValue<'v> for Obj {
    fn get_methods() -> Option<&'static Methods> {
        Some(OBJ_METHODS.methods())
    }
}

/// These are where the module docs go
#[starlark_module]
fn object(builder: &mut MethodsBuilder) {
    /// Docs for attr1
    #[starlark(attribute)]
    fn attr1<'v>(this: Value<'v>) -> starlark::Result<String> {
        Ok("attr1".to_owned())
    }

    #[starlark(attribute)]
    fn attr2<'v>(this: Value<'v>) -> starlark::Result<String> {
        Ok("attr2".to_owned())
    }

    /// Docs for func1
    ///
    /// # Arguments
    ///     * `foo`: Docs for foo
    ///
    /// # Returns
    /// The string 'func1'
    fn func1<'v>(this: Value<'v>, foo: String) -> anyhow::Result<String> {
        let _ignore = (this, foo);
        Ok("func1".to_owned())
    }

    fn func2<'v>(this: Value<'v>) -> anyhow::Result<String> {
        let _ = this;
        Ok("func2".to_owned())
    }

    /// Needs to be escaped when rendered in markdown.
    fn __exported__<'v>(this: Value<'v>) -> anyhow::Result<NoneType> {
        let _ = this;
        Ok(NoneType)
    }
}

#[test]
fn golden_docs_starlark() {
    let res = docs_golden_test(
        "starlark.golden.md",
        DocItem::Module(assert::pass_module(STARLARK_CODE).documentation()),
    );
    assert!(!res.contains("_do_not_export"));
}

#[test]
fn native_docs_module() {
    let res = docs_golden_test(
        "native.golden.md",
        DocItem::Module(get_globals().documentation()),
    );
    assert!(!res.contains("starlark::assert::all_true"));
    assert!(res.contains(r#"string_default: str = "my_default"#));
}

#[test]
fn doc_hidden_param_is_accepted_but_undocumented() {
    let mut a = assert::Assert::new();
    a.globals(get_globals());
    a.pass("pos_either_named(1, 2, c = 3, undocumented = 4)");

    let docs = get_globals().documentation();
    let Some(DocItem::Member(DocMember::Function(f))) = docs.members.get("pos_either_named") else {
        panic!("`pos_either_named` should be documented as a function");
    };
    assert!(f.params.doc_params().all(|p| p.name != "undocumented"));
}

fn test_globals_docs_render(with_linked_type: bool, render_signature_at_bottom: bool) {
    let global = get_globals().documentation();
    let modules_info = DocModuleInfo {
        module: &global,
        name: "globals".to_owned(),
        page_path: "".to_owned(),
    };
    fn linked_ty_mapper(path: &str, type_name: &str) -> String {
        format!("<a to=\"/path/to/{path}\">{type_name}</a>")
    }
    let linked_ty_mapper: Option<fn(&str, &str) -> String> = if with_linked_type {
        Some(linked_ty_mapper)
    } else {
        None
    };
    let res = render_markdown_multipage(
        vec![modules_info],
        linked_ty_mapper,
        render_signature_at_bottom,
    );
    let subfolder_name = match (with_linked_type, render_signature_at_bottom) {
        (true, true) => "multipage_linked_type_and_render_signature_at_bottom",
        (true, false) => "multipage_linked_type",
        (false, true) => "multipage_render_signature_at_bottom",
        (false, false) => "multipage",
    };
    let expected_keys = vec!["", "Magic", "Obj", "submod"];
    assert_eq!(&res.keys().sorted().collect::<Vec<_>>(), &expected_keys);
    for (k, v) in res {
        let k = if k.is_empty() { "globals" } else { &k };
        golden_test_template(
            &format!("src/docs/tests/golden/{subfolder_name}/{k}.golden.md"),
            &v,
        );
    }
}

#[test]
fn globals_render_default() {
    test_globals_docs_render(false, false);
}

#[test]
fn globals_render_default_with_linked_type() {
    test_globals_docs_render(true, false);
}

#[test]
fn globals_render_signature_at_bottom() {
    test_globals_docs_render(false, true);
}

#[test]
fn globals_render_signature_at_bottom_with_linked_type() {
    test_globals_docs_render(true, true);
}

#[test]
fn golden_docs_object() {
    let docs = DocType::from_starlark_value::<Obj>();
    let res = docs_golden_test("object.golden.md", DocItem::Type(docs));

    assert!(res.contains(r#"name.__exported__"#));
}
