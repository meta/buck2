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
//! A supply chain simulation built on top of DICE.
//! * Each company has a name and manufactures a list of resources.
//! * Each resource has a recipe associated with it.
//!
//! When a company is asked to produce a resource:
//! * It looks up the recipe of sub-resources.
//! * For each sub-resource, it finds the cheapest manufacturer.
//! * It then return a quote of the cost.
//!
//! The cost of a resource is the sum of each item cost + a company specific flat fee.

use std::collections::HashMap;
use std::sync::Arc;

use allocative::Allocative;
use async_trait::async_trait;
use derive_more::Display;
use dice::DiceComputations;
use dice::DiceKeyDyn;
use dice::DiceTransactionUpdater;
use dice::EqualityBehavior;
use dice::InjectedKey;
use dice::Key;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use futures::FutureExt;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use pagable::Pagable;
use pagable::pagable_typetag;
use ref_cast::RefCast;

#[derive(Display, Debug, Hash, Eq, Clone, PartialEq, Dupe, Allocative, Pagable)]
pub enum Resource {
    Wood,
    Plank,
    Stick,
    Cobblestone,
    Pickaxe,
    CraftingTable,
}

pub struct Recipe {
    /// The number of items this recipe produces (currently unused)
    pub produces: u8,
    /// A list of the resources (and how many) are required.
    pub ingredients: &'static [(u8, Resource)],
}

impl Recipe {
    /// Singleton for a primitive resource.
    const PRIMITIVE: Self = Self {
        produces: 1,
        ingredients: &[],
    };
    pub const WOOD: Self = Self::PRIMITIVE;
    pub const COBBLESTONE: Self = Self::PRIMITIVE;
    pub const PLANK: Self = Self {
        produces: 4,
        ingredients: &[(1, Resource::Wood)],
    };
    pub const STICK: Self = Self {
        produces: 4,
        ingredients: &[(2, Resource::Plank)],
    };
    pub const PICKAXE: Self = Self {
        produces: 1,
        ingredients: &[(3, Resource::Cobblestone), (2, Resource::Stick)],
    };
    pub const CRAFTING_TABLE: Self = Self {
        produces: 1,
        ingredients: &[(4, Resource::Plank)],
    };
}

impl Resource {
    pub const RESOURCES: [Resource; 6] = [
        Self::Wood,
        Self::Plank,
        Self::Stick,
        Self::Cobblestone,
        Self::Pickaxe,
        Self::CraftingTable,
    ];
    pub fn recipe(&self) -> Recipe {
        match self {
            Self::Wood => Recipe::WOOD,
            Self::Plank => Recipe::PLANK,
            Self::Stick => Recipe::STICK,
            Self::Cobblestone => Recipe::COBBLESTONE,
            Self::Pickaxe => Recipe::PICKAXE,
            Self::CraftingTable => Recipe::CRAFTING_TABLE,
        }
    }
}

/// Each company manufactures a set of resources for a given upcharge
#[derive(Eq, Clone, PartialEq, Allocative)]
pub struct Company {
    pub name: Arc<String>,
    /// A mapping between resource the company makes and its flat upcharge
    pub makes: HashMap<Resource, u16>,
}

#[async_trait]
pub trait Setup {
    /// Must be called before any companies are added.
    /// Sets the state of the resource map.
    fn init_state(&mut self) -> anyhow::Result<()>;
    /// Adds a list of companies and maps them to their resources.
    async fn add_companies(&mut self, companies: Vec<Company>) -> anyhow::Result<()>;
}

#[async_trait]
impl Setup for DiceTransactionUpdater {
    fn init_state(&mut self) -> anyhow::Result<()> {
        self.changed_to(
            Resource::RESOURCES
                .iter()
                .map(|resource| (LookupResource(resource.dupe()), Arc::new(vec![]))),
        )?;

        Ok(())
    }

    async fn add_companies(&mut self, companies: Vec<Company>) -> anyhow::Result<()> {
        let mut resource_to_company_local = HashMap::new();

        // convert company to insertion ready format
        let insertion_ready_companies = companies
            .into_iter()
            .map(|company| {
                let lookup = LookupCompany(company.name.dupe());

                // construct a resource => company lookups mapping across all companies
                for resource in company.makes.keys() {
                    resource_to_company_local
                        .entry(resource.dupe())
                        .or_insert_with(Vec::new)
                        .push(lookup.clone());
                }

                (lookup, Arc::new(company))
            })
            .collect::<Vec<_>>();

        self.changed_to(insertion_ready_companies)?;

        // split iterators to talk about resources and companies separately
        let (resources, companies): (Vec<_>, Vec<_>) = resource_to_company_local
            .into_iter()
            .map(|(k, v)| (LookupResource(k), v))
            .unzip();

        // get the remote resources => company mapping
        let state = self.existing_state().await;
        let remote_resources =
            dice_futures::join::join_all(state.ctx().compute_many(
                resources.iter().map(|res| {
                    DiceComputations::declare_closure(async |ctx| ctx.compute(res).await)
                }),
            ))
            .await;

        // combine remote company list with local company list for reach resource
        let joined: Vec<_> = resources
            .into_iter()
            .zip(companies)
            .zip(remote_resources)
            .map(|((resource, mut local_companies), remote_companies)| {
                local_companies.append(&mut (**remote_companies?).clone());

                Ok((resource, Arc::new(local_companies)))
            })
            .collect::<anyhow::Result<_>>()?;

        self.changed_to(joined)?;

        Ok(())
    }
}

#[async_trait]
pub trait Cost {
    /// Find the cheapest manufacturing cost for a resource
    async fn resource_cost(
        &mut self,
        resource: &Resource,
    ) -> Result<Option<u16>, Arc<anyhow::Error>>;
}

#[async_trait]
pub trait CostUpdater {
    /// Change the upcharge of a company for a resource.
    async fn change_company_resource_cost(
        &mut self,
        company: &str,
        resource: &Resource,
        new_price: u16,
    ) -> anyhow::Result<()>;
}

async fn lookup_company_resource_cost(
    ctx: &mut DiceComputations<'_>,
    company: &LookupCompany,
    resource: &Resource,
) -> Result<Option<u16>, Arc<anyhow::Error>> {
    #[derive(Display, Debug, Hash, Eq, Clone, Dupe, PartialEq, Allocative, Pagable)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct LookupCompanyResourceCost(LookupCompany, Resource);
    #[async_trait]
    impl Key for LookupCompanyResourceCost {
        fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
            dice::NoValueSerialize::<Self::Value>::new()
        }

        type Value = Result<Option<u16>, Arc<anyhow::Error>>;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            let company = ctx
                .compute(&self.0)
                .await
                .map_err(|e| Arc::new(anyhow::anyhow!(e)))?;
            let recipe = self.1.recipe();

            let upcharge = company.makes.get(&self.1);
            if upcharge.is_none() {
                return Ok(None);
            }

            // get the unit cost for each resource needed to make item
            let mut futs: FuturesUnordered<_> = ctx
                .compute_many(recipe.ingredients.iter().map(|(required, resource)| {
                    DiceComputations::declare_closure(async |ctx| {
                        ctx.resource_cost(resource)
                            .map(|res| {
                                Ok::<_, Arc<anyhow::Error>>(res?.map(|x| x * *required as u16))
                            })
                            .await
                    })
                }))
                .into_iter()
                .collect();

            let mut sum = 0;
            while let Some(x) = futs.next().await {
                if let Some(x) = x? {
                    sum += x;
                } else {
                    return Ok(None);
                }
            }

            Ok(Some(sum + upcharge.unwrap()))
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| match (x, y) {
                (Ok(x), Ok(y)) => x == y,
                _ => false,
            })
        }
    }
    let key = LookupCompanyResourceCost(company.clone(), resource.dupe());

    ctx.compute(&key)
        .await
        .map_err(|e| Arc::new(anyhow::anyhow!(e)))?
        .dupe()
}

#[async_trait]
impl Cost for DiceComputations<'_> {
    async fn resource_cost(
        &mut self,
        resource: &Resource,
    ) -> Result<Option<u16>, Arc<anyhow::Error>> {
        #[derive(
            Display, Debug, Hash, Eq, Dupe, Clone, PartialEq, RefCast, Allocative, Pagable
        )]
        #[repr(transparent)]
        #[pagable_typetag(DiceKeyDyn)]
        struct LookupResourceCost(Resource);
        #[async_trait]
        impl Key for LookupResourceCost {
            fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
                dice::NoValueSerialize::<Self::Value>::new()
            }

            type Value = Result<Option<u16>, Arc<anyhow::Error>>;

            async fn compute(
                &self,
                ctx: &mut DiceComputations,
                _cancellations: &CancellationContext,
            ) -> Self::Value {
                let companies = ctx
                    .compute(LookupResource::ref_cast(&self.0))
                    .await
                    .map_err(|e| Arc::new(anyhow::anyhow!(e)))?;

                let costs = dice_futures::join::join_all(ctx.compute_many(companies.iter().map(
                    |company| {
                        DiceComputations::declare_closure(async |ctx| {
                            lookup_company_resource_cost(ctx, company, &self.0).await
                        })
                    },
                )))
                .await;

                Ok(costs
                    .into_iter()
                    .collect::<Result<Vec<_>, Arc<_>>>()?
                    .into_iter()
                    .min()
                    .flatten())
            }

            fn equality_behavior() -> EqualityBehavior<Self::Value> {
                EqualityBehavior::Compare(|x, y| match (x, y) {
                    (Ok(x), Ok(y)) => x == y,
                    _ => false,
                })
            }
        }

        self.compute(LookupResourceCost::ref_cast(resource))
            .await
            .map_err(|e| Arc::new(anyhow::anyhow!(e)))?
            .dupe()
    }
}

#[async_trait]
impl CostUpdater for DiceTransactionUpdater {
    async fn change_company_resource_cost(
        &mut self,
        company: &str,
        resource: &Resource,
        new_price: u16,
    ) -> anyhow::Result<()> {
        let company_lookup = LookupCompany(Arc::new(company.to_owned()));
        let state = self.existing_state().await;
        let old_company = state.compute(&company_lookup).await?;
        let mut new_company = (**old_company).clone();
        let old_price = new_company.makes.get_mut(resource).ok_or_else(|| {
            anyhow::anyhow!("Tried to update cost for a resource company does not make")
        })?;
        *old_price = new_price;

        self.changed_to(vec![(company_lookup, Arc::new(new_company))])?;

        Ok(())
    }
}

#[derive(
    Display, Debug, Hash, Eq, Clone, Dupe, PartialEq, RefCast, Allocative, Pagable
)]
#[repr(transparent)]
#[pagable_typetag(DiceKeyDyn)]
pub struct LookupCompany(pub Arc<String>);
impl InjectedKey for LookupCompany {
    type Value = Arc<Company>;
    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        dice::NoValueSerialize::<Self::Value>::new()
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }
}

#[derive(
    Display, Debug, Hash, Eq, Dupe, Clone, PartialEq, RefCast, Allocative, Pagable
)]
#[repr(transparent)]
#[pagable_typetag(DiceKeyDyn)]
pub struct LookupResource(pub Resource);
impl InjectedKey for LookupResource {
    type Value = Arc<Vec<LookupCompany>>;
    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        dice::NoValueSerialize::<Self::Value>::new()
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }
}
