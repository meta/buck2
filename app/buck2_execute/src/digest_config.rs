/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;

use allocative::Allocative;
use buck2_common::cas_digest::CasDigestConfig;
use buck2_common::cas_digest::CasDigestConfigError;
use buck2_common::cas_digest::DigestAlgorithm;
use buck2_common::file_ops::metadata::FileMetadata;
use derivative::Derivative;
use dice::DiceData;
use dice::DiceDataBuilder;
use dupe::Dupe;
use ref_cast::RefCast;
use static_interner::Intern;

use crate::directory::ActionDirectoryBuilder;
use crate::directory::ActionSharedDirectory;
use crate::directory::INTERNER;
use crate::directory::ReDirectorySerializer;

/// This configuration describes how to interpret digests received from a RE backend.
#[derive(
    Copy,
    Clone,
    Dupe,
    Debug,
    Allocative,
    Hash,
    Eq,
    PartialEq,
    pagable::Pagable
)]
pub struct DigestConfig {
    inner: Intern<DigestConfigInner>,
}

impl DigestConfig {
    pub fn testing_default() -> Self {
        Self {
            inner: DIGEST_CONFIG_INTERNER
                .intern(DigestConfigInner::new(CasDigestConfig::testing_default())),
        }
    }

    /// Values are interned so identical configs share a single allocation.
    pub fn leak_new(
        algorithms: Vec<DigestAlgorithm>,
        preferred_source_algorithm: Option<DigestAlgorithm>,
    ) -> Result<Self, CasDigestConfigError> {
        let inner = DIGEST_CONFIG_INTERNER.intern(DigestConfigInner::new(
            CasDigestConfig::leak_new(algorithms, preferred_source_algorithm)?,
        ));
        Ok(Self { inner })
    }

    pub fn cas_digest_config(&self) -> CasDigestConfig {
        self.inner.cas_digest_config
    }

    pub fn empty_file(&self) -> FileMetadata {
        self.inner.empty_file.dupe()
    }

    pub fn as_directory_serializer(&self) -> &ReDirectorySerializer {
        ReDirectorySerializer::ref_cast(&self.inner.cas_digest_config)
    }

    /// The empty directory as artifact content: an exhaustive listing of nothing, i.e. "this
    /// directory exists and contains exactly nothing".
    pub fn empty_directory(&self) -> ActionSharedDirectory {
        self.inner.empty_directory.dupe()
    }
}

impl fmt::Display for DigestConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.cas_digest_config(), f)
    }
}

static_interner::interner!(
    DIGEST_CONFIG_INTERNER,
    buck2_hash::BuckHasher,
    DigestConfigInner
);

#[derive(Debug, Allocative, Derivative, Eq, pagable::Pagable)]
#[derivative(Hash, PartialEq)]
struct DigestConfigInner {
    cas_digest_config: CasDigestConfig,
    #[derivative(PartialEq = "ignore", Hash = "ignore")]
    empty_directory: ActionSharedDirectory,
    #[derivative(PartialEq = "ignore", Hash = "ignore")]
    empty_file: FileMetadata,
}

impl DigestConfigInner {
    fn new(cas_digest_config: CasDigestConfig) -> Self {
        let empty_directory = ActionDirectoryBuilder::empty_exhaustive()
            .fingerprint(&ReDirectorySerializer { cas_digest_config })
            .shared(&*INTERNER);
        let empty_file = FileMetadata::empty(cas_digest_config);

        Self {
            cas_digest_config,
            empty_directory,
            empty_file,
        }
    }
}

pub trait HasDigestConfig {
    fn get_digest_config(&self) -> DigestConfig;
}

pub trait SetDigestConfig {
    fn set_digest_config(&mut self, digest_config: DigestConfig);
}

impl HasDigestConfig for DiceData {
    fn get_digest_config(&self) -> DigestConfig {
        self.get::<DigestConfig>()
            .expect("digest config should be set")
            .dupe()
    }
}

impl SetDigestConfig for DiceDataBuilder {
    fn set_digest_config(&mut self, digest_config: DigestConfig) {
        self.set(digest_config)
    }
}

impl SetDigestConfig for DiceData {
    fn set_digest_config(&mut self, digest_config: DigestConfig) {
        self.set(digest_config)
    }
}
