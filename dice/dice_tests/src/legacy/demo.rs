/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

/////////////////////////////////////////////////////////////////////
// DEMO USAGE
// Reading a file is based on the value in that file, plus the file encoding

use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use allocative::Allocative;
use async_trait::async_trait;
use derive_more::Display;
use dice::BranchId;
use dice::DetectCycles;
use dice::Dice;
use dice::DiceComputations;
use dice::DiceKeyDyn;
use dice::DiceTransactionUpdater;
use dice::EqualityBehavior;
use dice::InjectedKey;
use dice::Key;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use dupe::ResultDupedExt;
use pagable::Pagable;
use pagable::pagable_typetag;
use tempfile::NamedTempFile;

#[derive(Debug, Clone, Dupe, PartialEq, Allocative)]
enum Encoding {
    Utf8,
    Ascii,
}

#[derive(Clone, Dupe, Debug, Display, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("{:?}", self)]
#[pagable_typetag(DiceKeyDyn)]
struct EncodingConfig();

impl InjectedKey for EncodingConfig {
    type Value = Encoding;

    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        dice::NoValueSerialize::<Self::Value>::new()
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }
}

struct Encodings<'c, 'd>(&'c mut DiceComputations<'d>);

impl Encodings<'_, '_> {
    async fn get(&mut self) -> Result<Encoding, Arc<anyhow::Error>> {
        self.0
            .compute(&EncodingConfig())
            .await
            .map_err(|e| Arc::new(anyhow::anyhow!(e)))
            .duped()
    }
}

trait HasEncodings<'d> {
    fn encodings<'c>(&'c mut self) -> Encodings<'c, 'd>;
}

impl<'d> HasEncodings<'d> for DiceComputations<'d> {
    fn encodings<'c>(&'c mut self) -> Encodings<'c, 'd> {
        Encodings(self)
    }
}

trait SetEncodings {
    fn set_encodings(&mut self, enc: Encoding) -> anyhow::Result<()>;
}

impl SetEncodings for DiceTransactionUpdater {
    fn set_encodings(&mut self, enc: Encoding) -> anyhow::Result<()> {
        Ok(self.changed_to(vec![(EncodingConfig(), enc)])?)
    }
}

struct Filesystem<'c, 'd>(&'c mut DiceComputations<'d>);

#[derive(Clone, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("File({})", _0.display())]
#[pagable_typetag(DiceKeyDyn)]
struct File(#[pagable(flatten_serde)] PathBuf);

#[async_trait]
impl Key for File {
    type Value = Result<Arc<String>, Arc<anyhow::Error>>;
    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        dice::NoValueSerialize::<Self::Value>::new()
    }
    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        let encoding = ctx.encodings().get().await?;

        let s = fs::read_to_string(&self.0).unwrap();

        Ok(Arc::new(match encoding {
            Encoding::Utf8 => s,
            Encoding::Ascii => s.replace(":-)", "smile"),
        }))
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| match (x, y) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        })
    }
}

impl Filesystem<'_, '_> {
    async fn read_file(&mut self, file: &Path) -> Result<Arc<String>, Arc<anyhow::Error>> {
        self.0
            .compute(&File(file.to_path_buf()))
            .await
            .map_err(|e| Arc::new(anyhow::anyhow!(e)))?
            .dupe()
    }
}

trait HasFilesystem<'d> {
    fn filesystem<'c>(&'c mut self) -> Filesystem<'c, 'd>;
}

impl<'d> HasFilesystem<'d> for DiceComputations<'d> {
    fn filesystem<'c>(&'c mut self) -> Filesystem<'c, 'd> {
        Filesystem(self)
    }
}

trait SetFilesystem {
    fn filesystem_changed(&mut self, file: &Path) -> anyhow::Result<()>;
}

impl SetFilesystem for DiceTransactionUpdater {
    fn filesystem_changed(&mut self, file: &Path) -> anyhow::Result<()> {
        Ok(self.changed(vec![File(file.to_path_buf())])?)
    }
}

#[tokio::test]
async fn demo() -> anyhow::Result<()> {
    let temp = NamedTempFile::new().unwrap();
    let f = PathBuf::from(temp.path());

    let dice = Dice::builder().build(DetectCycles::Enabled);
    let root = dice.new_root().await;

    let mut ctx = dice.updater(root);
    ctx.set_encodings(Encoding::Utf8)?;
    ctx.commit().await;

    let set = |x: &str| fs::write(&f, x).unwrap();

    async fn get(x: &str, dice: &Arc<Dice>, root: BranchId, f: &Path) {
        let contents = dice
            .updater(root)
            .commit()
            .await
            .ctx()
            .filesystem()
            .read_file(f)
            .await
            .unwrap();
        assert_eq!(*contents, x)
    }

    set(":-)");

    get(":-)", &dice, root, &f).await;

    // doesn't change because I didn't dirty it
    set("hello :-)");

    get(":-)", &dice, root, &f).await;

    let mut ctx = dice.updater(root);
    ctx.filesystem_changed(&f)?;
    ctx.commit().await;
    get("hello :-)", &dice, root, &f).await;

    let mut ctx = dice.updater(root);
    ctx.set_encodings(Encoding::Ascii)?;
    ctx.commit().await;
    get("hello smile", &dice, root, &f).await;

    Ok(())
}
