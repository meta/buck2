/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::HashMap;
use std::sync::Arc;

use dice::BranchId;
use dice::DetectCycles;
use dice::Dice;
use dice::DiceTransaction;
use dice_examples::supply_chain::Company;
use dice_examples::supply_chain::Cost;
use dice_examples::supply_chain::CostUpdater;
use dice_examples::supply_chain::Resource;
use dice_examples::supply_chain::Setup;

async fn setup(companies: Vec<Company>) -> anyhow::Result<(Arc<Dice>, BranchId, DiceTransaction)> {
    let dice = Dice::builder().build(DetectCycles::Enabled);
    let root = dice.new_root().await;
    let mut ctx = dice.updater(root);
    ctx.init_state()?;
    ctx.commit().await;

    let mut ctx = dice.updater(root);
    ctx.add_companies(companies).await?;

    Ok((dice, root, ctx.commit().await))
}

#[tokio::test]
async fn test_no_resources() -> Result<(), Arc<anyhow::Error>> {
    let (dice, root, ctx) = setup(vec![Company {
        name: Arc::new("hello world".to_owned()),
        makes: HashMap::new(),
    }])
    .await?;

    assert_eq!(None, ctx.ctx().resource_cost(&Resource::Wood).await?);
    let mut ctx = dice.updater(root);
    let success = ctx
        .change_company_resource_cost("hello world", &Resource::Stick, 5)
        .await;
    assert!(success.is_err());
    Ok(())
}

#[tokio::test]
async fn test_other_resource() -> Result<(), Arc<anyhow::Error>> {
    let (_, _, ctx) = setup(vec![Company {
        name: Arc::new("hello world".to_owned()),
        makes: [(Resource::Wood, 2)].iter().cloned().collect(),
    }])
    .await?;

    assert_eq!(None, ctx.ctx().resource_cost(&Resource::Stick).await?);
    assert_eq!(Some(2), ctx.ctx().resource_cost(&Resource::Wood).await?);
    Ok(())
}

#[tokio::test]
async fn test_simple() -> Result<(), Arc<anyhow::Error>> {
    let (_, _, ctx) = setup(vec![
        Company {
            name: Arc::new("Steve".to_owned()),
            makes: [
                (Resource::Plank, 2),
                (Resource::Wood, 1),
                (Resource::Stick, 4),
                (Resource::Cobblestone, 7),
            ]
            .iter()
            .cloned()
            .collect(),
        },
        Company {
            name: Arc::new("Alex".to_owned()),
            makes: [(Resource::Pickaxe, 10), (Resource::CraftingTable, 5)]
                .iter()
                .cloned()
                .collect(),
        },
    ])
    .await?;

    let expected = [
        (Resource::Wood, 1),
        (Resource::Cobblestone, 7),
        (Resource::Plank, 1 + 2),
        (Resource::Stick, 2 * (1 + 2) + 4),
        (Resource::Pickaxe, 3 * 7 + 2 * (2 * (1 + 2) + 4) + 10),
        (Resource::CraftingTable, 4 * (1 + 2) + 5),
    ];

    for (resource, cost) in &expected {
        assert_eq!(
            Some(*cost),
            ctx.ctx().resource_cost(resource).await?,
            "Testing {resource}",
        );
    }
    Ok(())
}

#[tokio::test]
async fn test_complex() -> Result<(), Arc<anyhow::Error>> {
    let (_, _, ctx) = setup(vec![
        Company {
            name: Arc::new("Steve".to_owned()),
            makes: [
                (Resource::Plank, 2),
                (Resource::Wood, 1),
                (Resource::Stick, 4),
                (Resource::Cobblestone, 7),
            ]
            .iter()
            .cloned()
            .collect(),
        },
        Company {
            name: Arc::new("Alex".to_owned()),
            makes: [(Resource::Pickaxe, 10), (Resource::CraftingTable, 5)]
                .iter()
                .cloned()
                .collect(),
        },
        Company {
            name: Arc::new("Bob".to_owned()),
            makes: [
                (Resource::Pickaxe, 5),
                (Resource::Plank, 1),
                (Resource::Stick, 2),
            ]
            .iter()
            .cloned()
            .collect(),
        },
        Company {
            name: Arc::new("Raj".to_owned()),
            makes: [(Resource::Stick, 1)].iter().cloned().collect(),
        },
    ])
    .await?;

    let expected = [
        (Resource::Wood, 1),
        (Resource::Cobblestone, 7),
        (Resource::Plank, 1 + 1),
        (Resource::Stick, 2 * (1 + 1) + 1),
        (Resource::Pickaxe, 3 * 7 + 2 * (2 * (1 + 1) + 1) + 5),
        (Resource::CraftingTable, 4 * (1 + 1) + 5),
    ];

    for (resource, cost) in &expected {
        assert_eq!(
            Some(*cost),
            ctx.ctx().resource_cost(resource).await?,
            "Testing {resource}",
        );
    }
    Ok(())
}

#[tokio::test]
async fn test_change_cost() -> Result<(), Arc<anyhow::Error>> {
    let (dice, root, _) = setup(vec![
        Company {
            name: Arc::new("Steve".to_owned()),
            makes: [
                (Resource::Plank, 2),
                (Resource::Wood, 1),
                (Resource::Stick, 4),
                (Resource::Cobblestone, 7),
            ]
            .iter()
            .cloned()
            .collect(),
        },
        Company {
            name: Arc::new("Alex".to_owned()),
            makes: [(Resource::Pickaxe, 10), (Resource::CraftingTable, 5)]
                .iter()
                .cloned()
                .collect(),
        },
    ])
    .await?;

    let mut ctx = dice.updater(root);
    ctx.change_company_resource_cost("Steve", &Resource::Stick, 2)
        .await
        .map_err(|e| Arc::new(anyhow::anyhow!(e)))?;

    let ctx = ctx.commit().await;

    assert_eq!(
        Some(3 * 7 + 2 * (2 * (1 + 2) + 2) + 10),
        ctx.ctx().resource_cost(&Resource::Pickaxe).await?
    );

    let mut ctx = dice.updater(root);
    let update_success = ctx
        .change_company_resource_cost("Steve", &Resource::Pickaxe, 5)
        .await;

    assert!(update_success.is_err());

    Ok(())
}
