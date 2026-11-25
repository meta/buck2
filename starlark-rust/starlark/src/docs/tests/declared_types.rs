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

//! The docs of declared types: a predicate type's page lists the documented types that are of
//! it, a type's page lists the predicate types it is of, a recursive type's page says what the
//! type is, and a signature links their names.

use std::collections::HashMap;
use std::sync::OnceLock;

use allocative::Allocative;
use dupe::Dupe;
use either::Either;
use starlark_derive::NoSerialize;
use starlark_derive::StarlarkPagable;
use starlark_derive::starlark_module;
use starlark_derive::starlark_value;
use starlark_map::small_map::SmallMap;
use starlark_syntax::golden_test_template::golden_test_template;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::docs::DocItem;
use crate::docs::DocMember;
use crate::docs::DocModule;
use crate::docs::DocProperty;
use crate::docs::DocType;
use crate::docs::markdown::render_doc_item_no_link;
use crate::docs::multipage::DocModuleInfo;
use crate::docs::multipage::render_markdown_multipage;
use crate::environment::Globals;
use crate::environment::GlobalsBuilder;
use crate::starlark_predicate_type;
use crate::starlark_recursive_type;
use crate::starlark_simple_value;
use crate::typing::ParamSpec;
use crate::typing::Ty;
use crate::typing::TyStarlarkValue;
use crate::typing::TyUser;
use crate::typing::TyUserParams;
use crate::values::StarlarkValue;
use crate::values::ValueOf;
use crate::values::list::StarlarkList;
use crate::values::starlark_value_as_type::StarlarkValueAsType;
use crate::values::string::StarlarkStr;
use crate::values::types::type_instance_id::TypeInstanceId;

/// What something with a weight is used through.
trait Weighs {
    fn grams(&self) -> i32;
}

starlark_predicate_type! {
    /// Something that can be weighed.
    ///
    /// A crate can, and a string weighs as much as it is long.
    StarlarkWeighable<'v>;
    name "Weighable";
    get &'v dyn Weighs;
    upstream_member StarlarkStr;
    upstream_member Thimble;
}

starlark_recursive_type! {
    /// What a scale takes: something that can be weighed, or a list of these.
    StarlarkLoad<'v>;
    name "Load";
    is Either<StarlarkWeighable, StarlarkList<'v, StarlarkLoad>>;
}

impl Weighs for StarlarkStr {
    fn grams(&self) -> i32 {
        self.len() as i32
    }
}

/// A member that the declaration names, and that has no page.
#[derive(
    Debug,
    derive_more::Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("thimble")]
struct Thimble;
starlark_simple_value!(Thimble);

#[starlark_value(type = "thimble")]
impl<'v> StarlarkValue<'v> for Thimble {}

impl Weighs for Thimble {
    fn grams(&self) -> i32 {
        3
    }
}

/// A member that says so in its block.
#[derive(
    Debug,
    derive_more::Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("crate")]
struct Crate;
starlark_simple_value!(Crate);

impl Weighs for Crate {
    fn grams(&self) -> i32 {
        1000
    }
}

#[starlark_value(type = "crate")]
impl<'v> StarlarkValue<'v> for Crate {
    #[starlark(implements = StarlarkWeighable)]
    fn weighable(this: &Self) -> &'v dyn Weighs {
        this
    }
}

/// A member whose documented type is a type of its own made of the member's values, as the
/// type of a provider is.
#[derive(
    Debug,
    derive_more::Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("parcel")]
struct Parcel;
starlark_simple_value!(Parcel);

impl Weighs for Parcel {
    fn grams(&self) -> i32 {
        250
    }
}

#[starlark_value(type = "parcel")]
impl<'v> StarlarkValue<'v> for Parcel {
    fn get_type_starlark_repr() -> Ty {
        static TY: OnceLock<Ty> = OnceLock::new();
        TY.get_or_init(|| {
            Ty::custom(
                TyUser::new(
                    "Parcel".to_owned(),
                    TyStarlarkValue::new::<Parcel>(),
                    TypeInstanceId::for_test(&"parcel"),
                    TyUserParams::default(),
                )
                .unwrap(),
            )
        })
        .dupe()
    }

    #[starlark(implements = StarlarkWeighable)]
    fn weighable(this: &Self) -> &'v dyn Weighs {
        this
    }
}

/// Not a member.
#[derive(
    Debug,
    derive_more::Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("cloud")]
struct Cloud;
starlark_simple_value!(Cloud);

#[starlark_value(type = "cloud")]
impl<'v> StarlarkValue<'v> for Cloud {}

#[starlark_module]
fn scale(builder: &mut GlobalsBuilder) {
    const Weighable: StarlarkValueAsType<StarlarkWeighable> = StarlarkWeighable::global();

    const Load: StarlarkValueAsType<StarlarkLoad> = StarlarkLoad::global();

    /// The weight of one thing.
    fn weigh<'v>(thing: ValueOf<'v, StarlarkWeighable>) -> starlark::Result<i32> {
        Ok(thing.grams())
    }

    /// The weight of everything on the scale.
    fn weigh_all<'v>(load: ValueOf<'v, StarlarkLoad>) -> starlark::Result<i32> {
        let _unused = load;
        Ok(0)
    }

    fn pack<'v>(thing: ValueOf<'v, Crate>) -> starlark::Result<ValueOf<'v, Parcel>> {
        let _unused = thing;
        Err(starlark::Error::new_other(anyhow::anyhow!("not packed")))
    }

    builder.type_symbol_as::<Crate>("Crate");
    builder.type_symbol_as::<Parcel>("Parcel");
    builder.type_symbol_as::<Cloud>("Cloud");
}

fn link(path: &str, type_name: &str) -> String {
    format!("<a to=\"/path/to/{path}\">{type_name}</a>")
}

/// The pages of the scale's globals, together with the standard ones so that `str` has a page.
fn pages(linked: bool) -> HashMap<String, String> {
    let scale = GlobalsBuilder::new().with(scale).build().documentation();
    let standard = Globals::standard().documentation();
    render_markdown_multipage(
        vec![
            DocModuleInfo {
                module: &scale,
                name: "scale".to_owned(),
                page_path: "".to_owned(),
            },
            DocModuleInfo {
                module: &standard,
                name: "standard".to_owned(),
                page_path: "standard".to_owned(),
            },
        ],
        if linked { Some(link) } else { None },
        false,
    )
}

fn golden_pages(linked: bool) {
    let pages = pages(linked);
    let dir = if linked {
        "declared_types_linked"
    } else {
        "declared_types"
    };
    for page in ["", "Weighable", "Load", "Crate", "Parcel", "Cloud"] {
        let name = if page.is_empty() { "scale" } else { page };
        golden_test_template(
            &format!("src/docs/tests/golden/{dir}/{name}.golden.md"),
            &pages[page],
        );
    }
}

#[test]
fn declared_types_pages() {
    golden_pages(false);
}

#[test]
fn declared_types_pages_linked() {
    golden_pages(true);
}

/// An upstream member's page says what it is of as any member's does. The type itself does
/// not know, the predicate type does. The section ends the page, after the type's methods.
#[test]
fn upstream_member_page_lists_the_predicate_type() {
    let pages = pages(true);
    assert!(
        pages["standard/str"].ends_with(
            "## Implements\n\nA value of this type is accepted where one of these is wanted.\n\n\
             - <a to=\"/path/to/Weighable\">Weighable</a>"
        ),
        "{}",
        pages["standard/str"]
    );
    assert!(!pages["standard/int"].contains("## Implements"));
}

/// A predicate type that has no page among those rendered is still named, with nothing to link
/// to: on the page of a type that says it is of it, and in a signature that mentions it.
#[test]
fn predicate_type_without_a_page_is_named() {
    let scale = GlobalsBuilder::new()
        .with(scale)
        .build()
        .documentation()
        .filter(|(name, _)| name != "Weighable");
    let pages = render_markdown_multipage(
        vec![DocModuleInfo {
            module: &scale,
            name: "scale".to_owned(),
            page_path: "".to_owned(),
        }],
        Some(link),
        false,
    );
    assert!(
        pages["Crate"].ends_with(
            "## Implements\n\nA value of this type is accepted where one of these is wanted.\n\n\
             - `Weighable`"
        ),
        "{}",
        pages["Crate"]
    );
    assert!(
        pages[""].contains("<code>def weigh(thing: Weighable) -> int</code>"),
        "{}",
        pages[""]
    );
}

/// A page whose type prints as more than its name is not linked from a signature: the text of
/// the link would be the whole type.
#[test]
fn type_that_prints_as_more_than_its_name_is_not_linked() {
    let callback = Ty::function(ParamSpec::empty(), Ty::none());
    let docs = DocModule {
        docs: None,
        members: SmallMap::from_iter([
            (
                "Callback".to_owned(),
                DocItem::Type(DocType {
                    docs: None,
                    members: SmallMap::new(),
                    ty: callback.dupe(),
                    constructor: None,
                }),
            ),
            (
                "on_done".to_owned(),
                DocItem::Member(DocMember::Property(DocProperty {
                    docs: None,
                    typ: callback,
                })),
            ),
        ]),
    };
    let pages = render_markdown_multipage(
        vec![DocModuleInfo {
            module: &docs,
            name: "callbacks".to_owned(),
            page_path: "".to_owned(),
        }],
        Some(link),
        false,
    );
    assert!(pages.contains_key("Callback"));
    assert!(
        pages[""].contains("<code>on_done: def() -> None</code>"),
        "{}",
        pages[""]
    );
}

/// The lists are of the pages rendered together. One item rendered alone has none, and a
/// recursive type still says what it is.
#[test]
fn one_item_has_no_lists() {
    let globals = GlobalsBuilder::new().with(scale).build();
    let render =
        |name: &str| render_doc_item_no_link(name, &globals.get(name).unwrap().documentation());
    assert!(!render("Crate").contains("## Implements"));
    assert!(!render("Weighable").contains("## Implemented by"));
    assert!(
        render("Load").contains("## Definition\n\n```python\nLoad = list[Load] | Weighable\n```"),
        "{}",
        render("Load")
    );
}
