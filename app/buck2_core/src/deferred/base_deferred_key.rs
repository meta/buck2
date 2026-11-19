/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::any::Any;
use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::fmt::Debug;
use std::fmt::Display;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::Arc;

use allocative::Allocative;
use buck2_data::ToProtoMessage;
use buck2_data::action_key_owner::BaseDeferredKeyProto;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use buck2_util::size_assert;
use cmp_any::PartialEqAny;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;
use pagable::typetag::PagableTagged;
use strong_hash::StrongHash;

use crate::content_hash::ContentBasedPathHash;
use crate::fs::buck_out_path::BuckOutPathKind;
use crate::fs::project_rel_path::ProjectRelativePath;
use crate::fs::project_rel_path::ProjectRelativePathBuf;
use crate::global_cfg_options::GlobalCfgOptions;
use crate::target::configured_target_label::ConfiguredTargetLabel;
use crate::target::name::EQ_SIGN_SUBST;

#[pagable_typetag]
pub trait BaseDeferredKeyDyn:
    PagableTagged + Debug + Display + Any + Allocative + Send + Sync + 'static
{
    fn eq_token(&self) -> PartialEqAny<'_>;
    fn hash(&self) -> u64;
    fn strong_hash(&self) -> u64;
    fn make_hashed_path(
        &self,
        base: &ProjectRelativePath,
        prefix: &ForwardRelativePath,
        action_key: Option<&str>,
        path: &ForwardRelativePath,
        path_resolution_method: BuckOutPathKind,
        content_hash: Option<&ContentBasedPathHash>,
    ) -> buck2_error::Result<ProjectRelativePathBuf>;
    /// Fake label for anon targets, `None` for BXL.
    fn configured_label(&self) -> Option<ConfiguredTargetLabel>;
    /// Short rule type name for anon targets, `None` for BXL.
    fn rule_type_name(&self) -> Option<&str>;
    fn to_proto(&self) -> BaseDeferredKeyProto;
    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync>;
    /// bxl anon target or bxl dynamic action node or bxl itself will return cfg else None
    fn global_cfg_options(&self) -> Option<GlobalCfgOptions>;
}

#[derive(Debug, derive_more::Display, Dupe, Clone, Allocative, Pagable)]
pub struct BaseDeferredKeyBxl(pub Arc<dyn BaseDeferredKeyDyn>);

impl PartialEq for BaseDeferredKeyBxl {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq_token() == other.0.eq_token()
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Tier0)]
pub enum PathResolutionError {
    #[error("Tried to resolve a content-based path {0} without providing the content hash!")]
    ContentBasedPathWithNoContentHash(ForwardRelativePathBuf),
}

#[derive(Debug, derive_more::Display, Dupe, Clone, Allocative, Pagable)]
pub enum BaseDeferredKey {
    TargetLabel(ConfiguredTargetLabel),
    AnonTarget(Arc<dyn BaseDeferredKeyDyn>),
    BxlLabel(BaseDeferredKeyBxl),
}

size_assert::words_of_type!(BaseDeferredKey, 3);

impl PartialEq for BaseDeferredKey {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (BaseDeferredKey::TargetLabel(a), BaseDeferredKey::TargetLabel(b)) => a == b,
            (BaseDeferredKey::TargetLabel(_), _) => false,
            (BaseDeferredKey::AnonTarget(a), BaseDeferredKey::AnonTarget(b)) => {
                a.eq_token() == b.eq_token()
            }
            (BaseDeferredKey::AnonTarget(_), _) => false,
            (BaseDeferredKey::BxlLabel(a), BaseDeferredKey::BxlLabel(b)) => a == b,
            (BaseDeferredKey::BxlLabel(_), _) => false,
        }
    }
}

impl Eq for BaseDeferredKey {}

impl PartialOrd for BaseDeferredKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

// `Ord` is only needed so page-out can serialize `HashMap`s keyed by
// `BaseDeferredKey` in a deterministic (process-stable) order.
impl Ord for BaseDeferredKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        fn rank(k: &BaseDeferredKey) -> u8 {
            match k {
                BaseDeferredKey::TargetLabel(_) => 0,
                BaseDeferredKey::AnonTarget(_) => 1,
                BaseDeferredKey::BxlLabel(_) => 2,
            }
        }
        match (self, other) {
            (BaseDeferredKey::TargetLabel(a), BaseDeferredKey::TargetLabel(b)) => a.cmp(b),
            // `dyn BaseDeferredKeyDyn` is not `Ord`; order by its content `strong_hash`
            // (a blake3 hash over all identity fields, stable across processes).
            (BaseDeferredKey::AnonTarget(a), BaseDeferredKey::AnonTarget(b)) => {
                a.strong_hash().cmp(&b.strong_hash())
            }
            (BaseDeferredKey::BxlLabel(a), BaseDeferredKey::BxlLabel(b)) => {
                a.0.strong_hash().cmp(&b.0.strong_hash())
            }
            _ => rank(self).cmp(&rank(other)),
        }
    }
}

impl Hash for BaseDeferredKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            BaseDeferredKey::TargetLabel(a) => a.hash(state),
            BaseDeferredKey::AnonTarget(d) | BaseDeferredKey::BxlLabel(BaseDeferredKeyBxl(d)) => {
                d.hash().hash(state)
            }
        }
    }
}

impl StrongHash for BaseDeferredKey {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        match self {
            BaseDeferredKey::TargetLabel(a) => a.strong_hash(state),
            BaseDeferredKey::AnonTarget(d) | BaseDeferredKey::BxlLabel(BaseDeferredKeyBxl(d)) => {
                d.strong_hash().strong_hash(state)
            }
        }
    }
}

impl BaseDeferredKey {
    pub fn unpack_target_label(&self) -> Option<&ConfiguredTargetLabel> {
        match self {
            BaseDeferredKey::TargetLabel(a) => Some(a),
            _ => None,
        }
    }

    pub fn configured_label(&self) -> Option<ConfiguredTargetLabel> {
        match self {
            BaseDeferredKey::TargetLabel(label) => Some(label.dupe()),
            BaseDeferredKey::AnonTarget(t) | BaseDeferredKey::BxlLabel(BaseDeferredKeyBxl(t)) => {
                t.configured_label()
            }
        }
    }

    pub(crate) fn make_hashed_path(
        &self,
        base: &ProjectRelativePath,
        prefix: &ForwardRelativePath,
        action_key: Option<&str>,
        path: &ForwardRelativePath,
        fully_hash_path: bool,
        path_resolution_method: BuckOutPathKind,
        content_hash: Option<&ContentBasedPathHash>,
    ) -> buck2_error::Result<ProjectRelativePathBuf> {
        match self {
            BaseDeferredKey::TargetLabel(target) => {
                let cell_relative_path = target.pkg().cell_relative_path().as_str();
                let escaped_target_name = Self::escape_target_name(target.name().as_str());

                // Keep the identifier as borrowed fragments so the complete output path
                // can be assembled with one final allocation. Repeated path joins
                // allocate and copy each growing prefix.
                let path_identifier = match path_resolution_method {
                    BuckOutPathKind::Configuration => [
                        target.cfg().output_hash().as_str(),
                        if target.exec_cfg().is_some() { "-" } else { "" },
                        target
                            .exec_cfg()
                            .as_ref()
                            .map_or("", |x| x.output_hash().as_str()),
                        "/",
                        cell_relative_path,
                        if cell_relative_path.is_empty() {
                            ""
                        } else {
                            "/"
                        },
                        "__",
                        escaped_target_name.as_ref(),
                        "__",
                        "/",
                        if action_key.is_none() {
                            ""
                        } else {
                            "__action__"
                        },
                        action_key.unwrap_or_default(),
                        if action_key.is_none() { "" } else { "__/" },
                    ],
                    BuckOutPathKind::ContentHash => {
                        let content_hash = content_hash.as_ref().map(|x| x.as_str());
                        if let Some(content_hash) = content_hash {
                            [
                                cell_relative_path,
                                if cell_relative_path.is_empty() {
                                    ""
                                } else {
                                    "/"
                                },
                                "__",
                                escaped_target_name.as_ref(),
                                "__",
                                "/",
                                if action_key.is_none() {
                                    ""
                                } else {
                                    "__action__"
                                },
                                action_key.unwrap_or_default(),
                                if action_key.is_none() { "" } else { "__/" },
                                content_hash,
                                "/",
                                "",
                                "",
                            ]
                        } else {
                            return Err(PathResolutionError::ContentBasedPathWithNoContentHash(
                                path.to_buf(),
                            ))?;
                        }
                    }
                };
                let path_hash;
                let hash_parts;
                let path_identifier = if fully_hash_path {
                    let mut hasher = DefaultHasher::new();
                    path_identifier.hash(&mut hasher);
                    path_hash = format!("{:016x}/", hasher.finish());
                    hash_parts = [path_hash.as_str()];
                    hash_parts.as_slice()
                } else {
                    path_identifier.as_slice()
                };

                let cell_name = target.pkg().cell_name();
                // Copy the identifier directly into the complete path, without
                // allocating an intermediate String for non-hashed paths.
                let mut hashed_path = String::with_capacity(
                    base.as_str().len()
                        + prefix.as_str().len()
                        + cell_name.as_str().len()
                        + 3
                        + path_identifier.iter().map(|part| part.len()).sum::<usize>()
                        + path.as_str().len(),
                );
                hashed_path.push_str(base.as_str());
                hashed_path.push('/');
                hashed_path.push_str(prefix.as_str());
                hashed_path.push('/');
                hashed_path.push_str(cell_name.as_str());
                hashed_path.push('/');
                for part in path_identifier {
                    hashed_path.push_str(part);
                }
                hashed_path.push_str(path.as_str());

                Ok(ProjectRelativePathBuf::unchecked_new(hashed_path))
            }
            BaseDeferredKey::AnonTarget(d) | BaseDeferredKey::BxlLabel(BaseDeferredKeyBxl(d)) => d
                .make_hashed_path(
                    base,
                    prefix,
                    action_key,
                    path,
                    path_resolution_method,
                    content_hash,
                ),
        }
    }

    pub fn make_unhashed_path(&self) -> Option<ForwardRelativePathBuf> {
        match self {
            BaseDeferredKey::TargetLabel(target) => Some(
                ForwardRelativePath::new(target.pkg().cell_name().as_str())
                    .unwrap()
                    .join(target.pkg().cell_relative_path()),
            ),
            _ => None,
        }
    }

    fn escape_target_name(target_name: &str) -> Cow<'_, str> {
        // Equals sign is difficult to escape especially for cmd.exe on Windows
        // which doesn't follow common escaping rules.
        if target_name.contains('=') {
            Cow::Owned(target_name.replace('=', EQ_SIGN_SUBST))
        } else {
            Cow::Borrowed(target_name)
        }
    }

    pub fn to_proto(&self) -> BaseDeferredKeyProto {
        match self {
            BaseDeferredKey::TargetLabel(t) => BaseDeferredKeyProto::TargetLabel(t.as_proto()),
            BaseDeferredKey::AnonTarget(d) | BaseDeferredKey::BxlLabel(BaseDeferredKeyBxl(d)) => {
                d.to_proto()
            }
        }
    }
}
