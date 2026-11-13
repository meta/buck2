/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::hash::Hasher;

use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::configured_node_ref::ConfiguredTargetNodeRefNode;
use buck2_node::nodes::configured_node_ref::ConfiguredTargetNodeRefNodeDeps;
use buck2_query::query::graph::dfs::dfs_postorder;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use dupe::Dupe;
use siphasher::sip128::Hasher128;
use siphasher::sip128::SipHasher24;
use strong_hash::StrongHash;

use crate::target_hash::Blake3Adapter;
use crate::target_hash::BuckTargetHasher;

#[derive(Clone, Dupe, derive_more::Display)]
pub enum ConfiguredTargetHash {
    #[display("{:032x}", _0)]
    SipHash(u128),
    #[display("{}", blake3::Hash::from(*_0))]
    Blake3([u8; blake3::OUT_LEN]),
}

impl ConfiguredTargetHash {
    fn hash_into<H: Hasher>(&self, hasher: &mut H) {
        match self {
            Self::SipHash(hash) => hasher.write_u128(*hash),
            Self::Blake3(hash) => hasher.write(hash),
        }
    }
}

trait ConfiguredTargetHasher: BuckTargetHasher {
    fn finish_target_hash(&self) -> ConfiguredTargetHash;
}

impl ConfiguredTargetHasher for SipHasher24 {
    fn finish_target_hash(&self) -> ConfiguredTargetHash {
        ConfiguredTargetHash::SipHash(self.finish128().as_u128())
    }
}

impl ConfiguredTargetHasher for Blake3Adapter {
    fn finish_target_hash(&self) -> ConfiguredTargetHash {
        ConfiguredTargetHash::Blake3(*self.finalize().as_bytes())
    }
}

#[derive(Default)]
struct ConfiguredTargetHashMap {
    hashes: BuckMutMap<ConfiguredTargetLabel, ConfiguredTargetHash>,
}

impl ConfiguredTargetHashMap {
    fn insert(&mut self, label: ConfiguredTargetLabel, hash: ConfiguredTargetHash) {
        self.hashes.insert(label, hash);
    }

    fn get(&self, label: &ConfiguredTargetLabel) -> Option<&ConfiguredTargetHash> {
        self.hashes.get(label)
    }

    fn contains_key(&self, label: &ConfiguredTargetLabel) -> bool {
        self.hashes.contains_key(label)
    }
}

pub(crate) struct ConfiguredTargetHashes {
    hashes: ConfiguredTargetHashMap,
}

pub(crate) struct ConfiguredTargetHashOptions {
    pub(crate) recursive: bool,
    pub(crate) use_fast_hash: bool,
    pub(crate) require_hash_change_deps: BuckMutSet<TargetLabel>,
}

impl ConfiguredTargetHashes {
    /// Hashes each requested configured node at most once, including reachable dependencies in
    /// recursive mode.
    pub(crate) fn compute(
        roots: &TargetSet<ConfiguredTargetNode>,
        options: &ConfiguredTargetHashOptions,
    ) -> buck2_error::Result<Self> {
        if options.use_fast_hash {
            Self::compute_with_hasher::<SipHasher24>(roots, options)
        } else {
            Self::compute_with_hasher::<Blake3Adapter>(roots, options)
        }
    }

    fn compute_with_hasher<H: ConfiguredTargetHasher>(
        roots: &TargetSet<ConfiguredTargetNode>,
        options: &ConfiguredTargetHashOptions,
    ) -> buck2_error::Result<Self> {
        let mut hashes = ConfiguredTargetHashMap::default();

        if options.recursive {
            dfs_postorder::<ConfiguredTargetNodeRefNode>(
                roots.iter().map(ConfiguredTargetNodeRefNode::new),
                ConfiguredTargetNodeRefNodeDeps,
                |node| {
                    let node = node.to_node();
                    let hash = match node.forward_target() {
                        Some(actual) => Self::dependency_hash(&node, actual.label(), &hashes)?,
                        None => Self::hash_node::<H>(&node, &hashes, options)?,
                    };
                    hashes.insert(node.label().dupe(), hash);
                    Ok(())
                },
            )?;
        } else {
            for node in roots {
                if hashes.contains_key(node.label()) {
                    continue;
                }

                let hash = Self::hash_node::<H>(node, &hashes, options)?;
                hashes.insert(node.label().dupe(), hash);
            }
        }

        Ok(Self { hashes })
    }

    fn hash_node<H: ConfiguredTargetHasher>(
        node: &ConfiguredTargetNode,
        hashes: &ConfiguredTargetHashMap,
        options: &ConfiguredTargetHashOptions,
    ) -> buck2_error::Result<ConfiguredTargetHash> {
        let mut hasher = H::new();
        node.body()?
            .target_hash_without_configured_labels(&mut hasher);

        let depends_on = options
            .require_hash_change_deps
            .contains(node.label().unconfigured())
            || node.deps().iter().any(|dep| {
                options
                    .require_hash_change_deps
                    .contains(dep.label().unconfigured())
            });
        depends_on.strong_hash(&mut hasher);

        if options.recursive {
            hasher.write_u64(node.deps().len() as u64);
            for dep in node.deps() {
                dep.label().unconfigured().strong_hash(&mut hasher);
                Self::dependency_hash(node, dep.label(), hashes)?.hash_into(&mut hasher);
            }
        }

        Ok(hasher.finish_target_hash())
    }

    fn dependency_hash(
        node: &ConfiguredTargetNode,
        dep: &ConfiguredTargetLabel,
        hashes: &ConfiguredTargetHashMap,
    ) -> buck2_error::Result<ConfiguredTargetHash> {
        hashes.get(dep).map(Dupe::dupe).ok_or_else(|| {
            ConfiguredTargetHashError::DependencyCycle(dep.to_string(), node.label().to_string())
                .into()
        })
    }

    pub(crate) fn get(
        &self,
        label: &ConfiguredTargetLabel,
    ) -> buck2_error::Result<ConfiguredTargetHash> {
        self.hashes.get(label).map(Dupe::dupe).ok_or_else(|| {
            ConfiguredTargetHashError::MissingRequestedTargetHash(label.to_string()).into()
        })
    }
}

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Input)]
enum ConfiguredTargetHashError {
    #[error(
        "Found a dependency `{0}` of configured target `{1}` which has not been hashed yet. This may indicate a dependency cycle in the configured graph."
    )]
    DependencyCycle(String, String),
    #[error("Hash for requested configured target `{0}` was not computed")]
    MissingRequestedTargetHash(String),
}

#[cfg(test)]
mod tests {
    use buck2_core::configuration::data::ConfigurationData;
    use buck2_core::execution_types::execution::ExecutionPlatformResolution;
    use buck2_node::attrs::arena::OwnedCoercedAttr;
    use buck2_node::attrs::attr::Attribute;
    use buck2_node::attrs::attr_type::AttrType;

    use super::*;

    #[test]
    fn hash_display() {
        assert_eq!("0".repeat(32), ConfiguredTargetHash::SipHash(0).to_string());
        assert_eq!(
            "f".repeat(32),
            ConfiguredTargetHash::SipHash(u128::MAX).to_string()
        );
        assert_eq!(
            "0".repeat(64),
            ConfiguredTargetHash::Blake3([0; blake3::OUT_LEN]).to_string()
        );
        assert_eq!(
            "f".repeat(64),
            ConfiguredTargetHash::Blake3([u8::MAX; blake3::OUT_LEN]).to_string()
        );
    }

    #[test]
    fn hash_uses_full_hasher_output() {
        let input = b"target";
        let mut fast = SipHasher24::new();
        fast.write(input);
        assert_eq!(
            format!("{:032x}", fast.finish128().as_u128()),
            fast.finish_target_hash().to_string()
        );

        let mut strong = Blake3Adapter::new();
        strong.write(input);
        assert_eq!(
            blake3::hash(input).to_string(),
            strong.finish_target_hash().to_string()
        );
    }

    #[test]
    fn dependency_hash_uses_full_digest() {
        let digest = blake3::hash(b"dependency");
        let mut hasher = Blake3Adapter::new();
        ConfiguredTargetHash::Blake3(*digest.as_bytes()).hash_into(&mut hasher);
        assert_eq!(
            blake3::hash(digest.as_bytes()).to_string(),
            hasher.finish_target_hash().to_string()
        );
    }

    fn configured_node(
        label: TargetLabel,
        cfg: ConfigurationData,
        value: &str,
    ) -> ConfiguredTargetNode {
        ConfiguredTargetNode::testing_new(
            label.configure(cfg),
            "test_rule",
            ExecutionPlatformResolution::new_for_testing(None, Vec::new()),
            vec![(
                "value",
                Attribute::new_const(None, "", AttrType::string()),
                OwnedCoercedAttr::new_with(|b| b.encode_string(value)).unwrap(),
            )],
            None,
        )
    }

    #[test]
    fn forward_targets_are_hashed_only_in_recursive_mode() -> buck2_error::Result<()> {
        let label = TargetLabel::testing_parse("cell//pkg:target");
        let inner = configured_node(label.dupe(), ConfigurationData::testing_new(), "value");
        let inner_label = inner.label().dupe();
        let outer_label = label.configure(ConfigurationData::unspecified());
        let outer = ConfiguredTargetNode::new_forward(outer_label.dupe(), inner)?;
        let roots = TargetSet::from_iter([outer]);

        for recursive in [false, true] {
            let hashes = ConfiguredTargetHashes::compute(
                &roots,
                &ConfiguredTargetHashOptions {
                    recursive,
                    use_fast_hash: true,
                    require_hash_change_deps: BuckMutSet::default(),
                },
            )?;

            hashes.get(&outer_label)?;
            assert_eq!(hashes.hashes.contains_key(&inner_label), recursive);
            if recursive {
                assert_eq!(
                    hashes.get(&outer_label)?.to_string(),
                    hashes.get(&inner_label)?.to_string(),
                );
            }
        }

        Ok(())
    }
}
