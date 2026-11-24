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

use std::collections::HashMap;

use dupe::Dupe;

use crate::docs::DocItem;
use crate::docs::DocModule;
use crate::docs::DocType;
use crate::docs::markdown::LayoutRenderConfig;
use crate::docs::markdown::render_implemented_by;
use crate::docs::markdown::render_implements;
use crate::typing::Ty;
use crate::typing::TyBasic;
use crate::typing::TyStarlarkValue;
use crate::typing::TyUser;
use crate::typing::predicate::TyPredicate;
use crate::typing::ty::TypeRenderConfig;

pub struct RenderConfig {
    pub type_config: TypeRenderConfig,
    pub layout_config: LayoutRenderConfig,
}

pub struct DocModuleInfo<'a> {
    pub module: &'a DocModule,
    pub name: String,
    /// A prefix to attach to all of the pages rendered from this module
    pub page_path: String,
}

impl<'a> DocModuleInfo<'a> {
    fn into_page_renders(&self) -> Vec<PageRender<'a>> {
        Self::traverse_inner(self.module, &self.name, &self.page_path)
    }

    fn traverse_inner(
        docs: &'a DocModule,
        module_name: &str,
        base_path: &str,
    ) -> Vec<PageRender<'a>> {
        let mut result = vec![];

        result.push(PageRender {
            page: DocPageRef::Module(docs),
            path: base_path.to_owned(),
            name: module_name.to_owned(),
            ty: None,
        });

        for (name, doc) in &docs.members {
            let path = if base_path.is_empty() {
                name.to_owned()
            } else {
                format!("{base_path}/{name}")
            };
            match doc {
                DocItem::Module(doc_module) => {
                    result.extend(Self::traverse_inner(doc_module, name, &path))
                }
                DocItem::Type(doc_type) => result.push(PageRender {
                    page: DocPageRef::Type(doc_type),
                    path,
                    name: name.to_owned(),
                    ty: Some(doc_type.ty.dupe()),
                }),

                DocItem::Member(_) => (),
            }
        }

        result
    }
}

/// A reference to a page to render
/// DocsRender will have all the PageRender it needs to render the docs
/// Since types and some modules are owned by other modules, we need to use the reference here
enum DocPageRef<'a> {
    Module(&'a DocModule),
    Type(&'a DocType),
}

/// A single page to render
struct PageRender<'a> {
    page: DocPageRef<'a>,
    path: String,
    name: String,
    /// The type of the page, if it is a type page. This is used to get the link to the type.
    ty: Option<Ty>,
}

impl<'a> PageRender<'a> {
    /// `relations` is the section of a type's page that says how the type and the predicate
    /// types of the pages rendered with it relate.
    fn render_markdown(&self, relations: Option<String>, render_config: &RenderConfig) -> String {
        match self.page {
            DocPageRef::Module(doc_module) => {
                doc_module.render_markdown_page_for_multipage_render(&self.name, render_config)
            }
            DocPageRef::Type(doc_type) => doc_type.render_markdown_page_for_multipage_render(
                &self.name,
                relations,
                render_config,
            ),
        }
    }

    /// The type of this page, if a mention of the type in a signature is a link to the page: a
    /// type that prints as its name, which is then the text of the link. A page can be of a
    /// type that prints as more than that, as a function type prints as its signature, and a
    /// mention of such a type is left as it prints.
    fn linked_ty(&self) -> Option<&Ty> {
        let ty = self.ty.as_ref()?;
        let linked = match ty.iter_union() {
            [TyBasic::StarlarkValue(_)] => true,
            [TyBasic::Custom(c)] => c.as_name().is_some_and(|name| name == c.to_string()),
            _ => false,
        };
        linked.then_some(ty)
    }

    /// The predicate type this is the page of, if it is the page of one.
    fn predicate(&self) -> Option<&TyPredicate> {
        match self.ty.as_ref()?.iter_union() {
            [TyBasic::Custom(c)] => c.0.as_any().downcast_ref::<TyPredicate>(),
            _ => None,
        }
    }

    /// The nominal type of this page's type: the type itself, or the base of a `TyUser`, whose
    /// values the values of the `TyUser` are. The type of a buck2 provider is such a `TyUser`.
    fn nominal(&self) -> Option<TyStarlarkValue> {
        match self.ty.as_ref()?.iter_union() {
            [TyBasic::StarlarkValue(x)] => Some(*x),
            [TyBasic::Custom(c)] => c.0.as_any().downcast_ref::<TyUser>().map(TyUser::base),
            _ => None,
        }
    }
}

/// Renders the contents into a multi-page tree structure
///
/// The output will contain page-paths like ``, `type1`, `mod1`, and `mod1/type2`, each mapped to
/// the contents of that page. That means that some of the paths may be prefixes of each other,
/// which will need consideration if this is to be materialized to a filesystem.
struct MultipageRender<'a> {
    page_renders: Vec<PageRender<'a>>,
    linked_ty_mapper: Option<fn(&str, &str) -> String>,
    render_config: RenderConfig,
}

impl<'a> MultipageRender<'a> {
    /// Create a new MultipageRender from a list of DocModuleInfo, and an optional function to map a type path to a linkable path
    /// If the function is not provided, the type will not be linkable
    /// linked_ty_mapper is used to map the **type path** and **type name** to a linkable element in the markdown
    fn new(
        docs: Vec<DocModuleInfo<'a>>,
        linked_ty_mapper: Option<fn(&str, &str) -> String>,
        render_signature_at_bottom: bool,
    ) -> Self {
        let mut res = vec![];

        for doc in docs {
            res.extend(doc.into_page_renders());
        }
        let mut type_render_config = TypeRenderConfig::Default;
        if let Some(linked_ty_mapper) = linked_ty_mapper {
            let mut ty_to_path_map = HashMap::new();
            for page in res.iter() {
                if let Some(ty) = page.linked_ty() {
                    ty_to_path_map.insert(ty.dupe(), page.path.clone());
                }
            }

            let render_linked_ty = move |ty: &TyBasic| {
                ty_to_path_map
                    .get(&Ty::basic(ty.dupe()))
                    .map(|type_path| linked_ty_mapper(type_path, &ty.to_string()))
            };

            type_render_config = TypeRenderConfig::LinkedType {
                render_linked_ty: Box::new(render_linked_ty),
            };
        }
        Self {
            page_renders: res,
            linked_ty_mapper,
            render_config: RenderConfig {
                type_config: type_render_config,
                layout_config: if render_signature_at_bottom {
                    LayoutRenderConfig::SignatureAtBottom
                } else {
                    LayoutRenderConfig::Default
                },
            },
        }
    }

    /// Render the docs into a map of markdown paths to markdown content
    fn render_markdown_pages(&self) -> HashMap<String, String> {
        self.page_renders
            .iter()
            .map(|page| {
                (
                    page.path.clone(),
                    page.render_markdown(self.relations(page), &self.render_config),
                )
            })
            .collect()
    }

    /// A page by its name, as a link to it where types are linked.
    fn page_ref(&self, page: &PageRender<'a>) -> String {
        match self.linked_ty_mapper {
            Some(linked_ty_mapper) => linked_ty_mapper(&page.path, &page.name),
            None => format!("`{}`", page.name),
        }
    }

    /// How the type of `page` and the predicate types among the pages relate. A predicate
    /// type's page lists the pages of the types that are of it, a type's page the predicate
    /// types it is of.
    ///
    /// There is no list of the members of a predicate type to read this from. Each type says
    /// which predicate types it is of, and a predicate type names its upstream members, so
    /// both lists are found by asking that of every pair of pages. A member that has no page
    /// here is not listed, but for an upstream one, which the predicate type knows by name.
    fn relations(&self, page: &PageRender<'a>) -> Option<String> {
        if let Some(predicate) = page.predicate() {
            let mut members: Vec<(String, String)> = self
                .page_renders
                .iter()
                .filter(|member| member.nominal().is_some_and(|ty| predicate.has_member(ty)))
                .map(|member| (member.name.clone(), self.page_ref(member)))
                .collect();
            let documented = |ty: &TyStarlarkValue| {
                self.page_renders
                    .iter()
                    .any(|member| member.nominal() == Some(*ty))
            };
            members.extend(
                predicate
                    .predicate()
                    .upstream_members()
                    .into_iter()
                    .filter(|ty| !documented(ty))
                    .map(|ty| (ty.to_string(), format!("`{ty}`"))),
            );
            members.sort();
            let members: Vec<String> = members.into_iter().map(|(_name, member)| member).collect();
            render_implemented_by(&members)
        } else if let Some(ty) = page.nominal() {
            // The predicate types that have a page, which are the ones that can be linked.
            let mut predicates: Vec<(String, String)> = self
                .page_renders
                .iter()
                .filter(|other| other.predicate().is_some_and(|p| p.has_member(ty)))
                .map(|other| (other.name.clone(), self.page_ref(other)))
                .collect();
            // The ones the type says it is of that have no page.
            predicates.extend(
                ty.implemented_predicates()
                    .into_iter()
                    .filter(|predicate| {
                        !self.page_renders.iter().any(|other| {
                            other
                                .predicate()
                                .is_some_and(|p| p.predicate() == *predicate)
                        })
                    })
                    .map(|predicate| (predicate.name().to_owned(), format!("`{predicate}`"))),
            );
            predicates.sort();
            let predicates: Vec<String> = predicates.into_iter().map(|(_name, p)| p).collect();
            render_implements(&predicates)
        } else {
            None
        }
    }
}

/// Renders the contents into a multi-page tree structure
///
/// The output will contain page-paths like ``, `type1`, `mod1`, and `mod1/type2`, each mapped to
/// the contents of that page. That means that some of the paths may be prefixes of each other,
/// which will need consideration if this is to be materialized to a filesystem.
///
/// It accepts a list of DocModuleInfo, and an optional function linked_ty_mapper
/// linked_ty_mapper is used to map the **type path** and **type name** to a linkable element in the markdown
pub fn render_markdown_multipage(
    modules_infos: Vec<DocModuleInfo<'_>>,
    linked_ty_mapper: Option<fn(&str, &str) -> String>,
    render_signature_at_bottom: bool,
) -> HashMap<String, String> {
    let multipage_render =
        MultipageRender::new(modules_infos, linked_ty_mapper, render_signature_at_bottom);
    multipage_render.render_markdown_pages()
}
