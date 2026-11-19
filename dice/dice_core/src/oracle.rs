/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The justification judgment of `incrementality.md` §3.2, computed by brute force over a
//! recorded history: the oracle the fuzzer checks lookups against. Soundness (§3.3) is
//! unconditional, so the oracle is exact whatever certificates were written, honest or not.

use std::collections::HashMap;
use std::collections::HashSet;

use crate::cert::Cert;
use crate::env::Premise;
use crate::ids::EpsilonToken;
use crate::ids::Key;
use crate::ids::Revision;
use crate::ids::Version;

/// An asserted thing: a key, or a key's untracked input.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Atom {
    Key(Key),
    Untracked(Key),
}

/// A revision of either kind of atom, widened to one type.
type Rev = u64;

struct VersionRecord {
    parent: Option<Version>,
    assertions: Vec<(Atom, Rev)>,
}

struct CertRecord {
    key: Key,
    revision: Rev,
    premises: Vec<(Atom, Rev)>,
}

/// The recorded history `H` and the written certificates `W` (§3.1).
pub(crate) struct Model {
    versions: HashMap<Version, VersionRecord>,
    written: Vec<CertRecord>,
}

impl Model {
    /// An empty history.
    pub(crate) fn new() -> Self {
        Model {
            versions: HashMap::new(),
            written: Vec::new(),
        }
    }

    pub(crate) fn record_version(
        &mut self,
        v: Version,
        parent: Option<Version>,
        assertions: Vec<(Atom, Rev)>,
    ) {
        let previous = self
            .versions
            .insert(v, VersionRecord { parent, assertions });
        assert!(previous.is_none(), "{v} recorded twice");
    }

    pub(crate) fn record_write(&mut self, cert: &Cert<Vec<Premise>>) {
        let mut premises: Vec<(Atom, Rev)> = cert
            .premises()
            .map(|p| (Atom::Key(p.key), rev_of(p.revision)))
            .collect();
        premises.push((Atom::Untracked(cert.key), eps_of(cert.epsilon)));
        self.written.push(CertRecord {
            key: cert.key,
            revision: rev_of(cert.revision),
            premises,
        });
    }

    pub(crate) fn has_version(&self, v: Version) -> bool {
        self.versions.contains_key(&v)
    }

    /// `asserted(x, v)` (§3.1): the nearest assertion of `atom` on the path from `v` to its
    /// root, with the root's implicit initial assertion for untracked inputs.
    fn asserted(&self, atom: Atom, v: Version) -> Option<Rev> {
        let mut cur = Some(v);
        while let Some(version) = cur {
            let record = self.versions.get(&version).expect("unrecorded version");
            if let Some((_, r)) = record.assertions.iter().rev().find(|(a, _)| *a == atom) {
                return Some(*r);
            }
            cur = record.parent;
        }
        match atom {
            Atom::Untracked(_) => Some(eps_of(EpsilonToken::INITIAL)),
            Atom::Key(_) => None,
        }
    }

    /// Everything justified at `v` (§3.2): the least relation closed under (Assert) and (Cert).
    pub(crate) fn justified(&self, v: Version) -> HashSet<(Atom, Rev)> {
        let mut facts: HashSet<(Atom, Rev)> = HashSet::new();
        let mut atoms: HashSet<Atom> = HashSet::new();
        for record in self.versions.values() {
            for (atom, _) in &record.assertions {
                atoms.insert(*atom);
            }
        }
        for cert in &self.written {
            atoms.insert(Atom::Untracked(cert.key));
            for (atom, _) in &cert.premises {
                atoms.insert(*atom);
            }
        }
        for atom in atoms {
            if let Some(r) = self.asserted(atom, v) {
                facts.insert((atom, r));
            }
        }
        loop {
            let before = facts.len();
            for cert in &self.written {
                let holds = cert
                    .premises
                    .iter()
                    .all(|(atom, r)| facts.contains(&(*atom, *r)));
                if holds {
                    facts.insert((Atom::Key(cert.key), cert.revision));
                }
            }
            if facts.len() == before {
                return facts;
            }
        }
    }

    pub(crate) fn asserted_atom(&self, atom: Atom, v: Version) -> Option<Rev> {
        self.asserted(atom, v)
    }

    pub(crate) fn is_justified(&self, key: Key, revision: Revision, v: Version) -> bool {
        self.justified(v)
            .contains(&(Atom::Key(key), rev_of(revision)))
    }
}

pub(crate) fn rev_of(r: Revision) -> Rev {
    r.as_u32() as Rev
}

pub(crate) fn eps_of(e: EpsilonToken) -> Rev {
    e.as_u64()
}
