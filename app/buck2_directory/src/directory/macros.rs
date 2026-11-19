/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

macro_rules! impl_fingerprinted_directory {
    (
        $this: ident
    ) => {
        impl<L, H> $crate::directory::fingerprinted_directory::FingerprintedDirectory<L, H>
            for $this<L, H>
        where
            H: DirectoryDigest,
        {
            type FingerprintedDirectoryRef<'a>
                = <Self as $crate::directory::directory::Directory<L, H>>::DirectoryRef<'a>
            where
                Self: Sized + 'a,
                L: 'a;

            fn as_fingerprinted_ref<'a>(&'a self) -> Self::FingerprintedDirectoryRef<'a>
            where
                Self: Sized + 'a,
            {
                self.as_ref()
            }

            fn fingerprint(&self) -> &H {
                $this::fingerprint(self)
            }

            fn exhaustiveness_hash(&self) -> $crate::directory::exhaustiveness::ExhaustivenessHash {
                $this::exhaustiveness_hash(self)
            }

            fn size(&self) -> u64 {
                $this::size(self)
            }
        }

        impl<L, H> PartialEq for $this<L, H>
        where
            H: DirectoryDigest,
        {
            fn eq(&self, other: &Self) -> bool {
                self.fingerprint() == other.fingerprint()
                    && self.exhaustiveness_hash() == other.exhaustiveness_hash()
            }
        }

        impl<L, H> Eq for $this<L, H> where H: DirectoryDigest {}

        /// Hashes what `PartialEq` above compares.
        impl<L, H> strong_hash::StrongHash for $this<L, H>
        where
            H: DirectoryDigest + strong_hash::StrongHash,
        {
            fn strong_hash<S: ::std::hash::Hasher>(&self, state: &mut S) {
                self.fingerprint().strong_hash(state);
                self.exhaustiveness_hash().strong_hash(state);
            }
        }
    };
}

pub(super) use impl_fingerprinted_directory;
