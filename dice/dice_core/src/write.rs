/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! `write` (`incrementality.md` §5.2).

use crate::arc::Arc;
use crate::cert::Cert;
use crate::env::Env;
use crate::ids::BranchId;
use crate::ids::Key;
use crate::ids::Version;
use crate::resolve::Resolved;
use crate::slot::Claim;
use crate::slot::Window;
use crate::state::CoreState;

/// What a write did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct WriteOutcome {
    /// The branches that received a claim from the write, ancestors first. Branches that came
    /// to resolve the certificate by inheritance are not listed.
    pub installed: Vec<BranchId>,
}

impl WriteOutcome {
    /// Whether the certificate is retained anywhere as a result of the write. A certificate that
    /// is not may still be retained from an earlier write.
    pub fn installed_anywhere(&self) -> bool {
        !self.installed.is_empty()
    }
}

/// The cascade obligation a claim installed at a branch creates for one child (§5.2 step 2).
enum Cascade<E: Env> {
    /// The child keeps resolving as it did, through its own copy of the outgoing claim
    /// (step 2.6 for a previously valid child).
    ReHome { child: BranchId, claim: Claim<E> },
    /// The child newly resolves the certificate: it is attached by edges alone if the
    /// certificate covers all of it (step 2.5), else it gets an empty claim (step 2.6).
    FillInOrShadow {
        child: BranchId,
        candidate: Option<Arc<Cert<E::Premises>>>,
    },
}

impl<E: Env> CoreState<E> {
    /// `write(cert)` (§5.2).
    ///
    /// Panics if the key has been asserted: no key is both asserted and certified.
    pub fn write(&mut self, cert: Arc<Cert<E::Premises>>) -> WriteOutcome {
        let key = cert.key;
        assert!(
            !self.is_asserted(key),
            "{key:?} is certified after being asserted; a key is never both"
        );
        let mut outcome = WriteOutcome::default();
        // Branch ids increase from parent to child, so id order is ancestor-first.
        for index in 0..self.branches.len() {
            let b = BranchId::from_index(index);
            if !self.is_live(b) || self.attached(key, b).is_some() {
                continue;
            }
            let cover = self.cover(b, &cert);
            let Some((window, attribute)) = self.pick_window(b, &cover) else {
                continue;
            };
            if let Some(existing) = self.slot(key, b).and_then(|slot| slot.claim.as_ref())
                && existing.window.reach() >= window.reach()
            {
                continue;
            }
            self.install(
                key,
                b,
                Claim {
                    cert: cert.clone(),
                    window,
                    attribute,
                },
            );
            outcome.installed.push(b);
        }
        outcome
    }

    fn install(&mut self, key: Key, b: BranchId, claim: Claim<E>) {
        for cascade in self.cascades(key, b, &claim) {
            self.discharge(key, &claim, cascade);
        }
        if claim.window.is_open() {
            self.register_edges(b, &claim.cert);
        }
        self.set_claim(key, b, Some(claim));
    }

    fn cascades(&self, key: Key, b: BranchId, new: &Claim<E>) -> Vec<Cascade<E>> {
        let mut cascades = Vec::new();
        for &(child, fork_seq) in self.branch(b).children.as_slice() {
            if self
                .slot(key, child)
                .is_some_and(|slot| slot.claim.is_some())
            {
                continue;
            }
            let after_valid = new.window.covers(fork_seq);
            let cascade = match self.resolve(key, Version::new(b, fork_seq)) {
                Resolved::Claim { claim, .. } => {
                    if after_valid && Cert::same_trace(&claim.cert, &new.cert) {
                        continue;
                    }
                    Cascade::ReHome {
                        child,
                        claim: Claim {
                            cert: claim.cert.clone(),
                            window: Window::open(self.branch(child).first),
                            attribute: claim.attribute,
                        },
                    }
                }
                Resolved::Unknown { candidate } => {
                    if !after_valid {
                        continue;
                    }
                    Cascade::FillInOrShadow {
                        child,
                        candidate: candidate.cloned(),
                    }
                }
                Resolved::Asserted { .. } => unreachable!("asserted keys are never written"),
            };
            cascades.push(cascade);
        }
        cascades
    }

    fn discharge(&mut self, key: Key, new: &Claim<E>, cascade: Cascade<E>) {
        match cascade {
            Cascade::ReHome { child, claim } => self.set_claim(key, child, Some(claim)),
            Cascade::FillInOrShadow { child, candidate } => {
                let first = self.branch(child).first;
                let cover = self.cover(child, &new.cert);
                let covers_all = cover.len() == 1
                    && cover[0].interval.from == first
                    && cover[0].interval.until.is_none();
                if covers_all {
                    self.register_edges(child, &new.cert);
                    let grandchildren: Vec<BranchId> = self
                        .branch(child)
                        .children
                        .iter()
                        .map(|(g, _)| *g)
                        .collect();
                    for grandchild in grandchildren {
                        if self
                            .slot(key, grandchild)
                            .is_some_and(|slot| slot.claim.is_some())
                        {
                            continue;
                        }
                        self.discharge(
                            key,
                            new,
                            Cascade::FillInOrShadow {
                                child: grandchild,
                                candidate: candidate.clone(),
                            },
                        );
                    }
                } else {
                    self.set_claim(
                        key,
                        child,
                        Some(Claim {
                            cert: candidate.unwrap_or_else(|| new.cert.clone()),
                            window: Window::empty(first),
                            // A shadow resolves Unknown, and only Valid resolutions hand
                            // attributes out, so this one is never seen.
                            attribute: E::EMPTY,
                        }),
                    );
                }
            }
        }
    }
}
