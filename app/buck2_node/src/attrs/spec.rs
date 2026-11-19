/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_hash::BuckFixedInsertionMap;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::index_map::Entry;
use buck2_util::exact_size_iter::IterExactSize;
use dupe::Dupe;
use dupe::OptionDupedExt;
use pagable::Pagable;
use serde::Deserialize;
use serde::Serialize;

use super::anon_target_attr_validation::AnonRuleAttrValidation;
use crate::attrs::arena::CoercedAttrArenaBuilder;
use crate::attrs::arena::CoercedAttrRef;
use crate::attrs::arena::OwnedCoercedAttr;
use crate::attrs::attr::Attribute;
use crate::attrs::coerced_attr_full::CoercedAttrFull;
use crate::attrs::inspect_options::AttrInspectOptions;
use crate::attrs::spec::internal::INCOMING_TRANSITION_ATTRIBUTE;
use crate::attrs::spec::internal::common_internal_attrs;
use crate::attrs::spec::internal::is_internal_attr;
use crate::attrs::values::AttrValues;
use crate::attrs::values::UnsealedAttrValues;
use crate::rule::RuleIncomingTransition;

pub mod internal;

/// Identifies an attribute within a rule.
#[derive(
    Debug,
    Clone,
    dupe::Dupe,
    Copy,
    Eq,
    Hash,
    PartialEq,
    Ord,
    PartialOrd,
    Allocative,
    Serialize,
    Deserialize,
    Pagable,
    strong_hash::StrongHash
)]
pub struct AttributeId(u16); // Index in the attribute spec

impl AttributeId {
    const MAX_INDEX: u16 = u16::MAX;
}

/// AttributeSpec holds the specification for a rules attributes as defined in the rule() call.
/// Attributes are kept in declaration order, internal attributes first; an attribute's
/// `AttributeId` is its position.
///
/// For its attribute values, a TargetNode will hold sorted (AttributeId, arena ref) rows that have values
/// only for the values that are explicitly set. Default values need to be looked up through the AttributeSpec.
#[derive(Debug, Eq, PartialEq, Hash, Pagable, Allocative)]
pub struct AttributeSpec {
    attributes: BuckFixedInsertionMap<Box<str>, Attribute>,
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
pub(crate) enum AttributeSpecError {
    #[error("User provided attribute `{0}` overrides internal attribute")]
    InternalAttributeRedefined(String),
    #[error("Duplicate attribute `{0}`")]
    DuplicateAttribute(String),
    #[error("Rule definition has no attribute `{0}`")]
    UnknownAttribute(String),
    #[error("Too many attributes: {0} > {}", AttributeId::MAX_INDEX)]
    TooManyAttributes(usize),
}

impl AttributeSpec {
    fn new(
        attributes: BuckMutInsertionMap<Box<str>, Attribute>,
    ) -> buck2_error::Result<AttributeSpec> {
        if attributes.len() > AttributeId::MAX_INDEX as usize {
            return Err(AttributeSpecError::TooManyAttributes(attributes.len()).into());
        }
        Ok(AttributeSpec {
            attributes: attributes.freeze(),
        })
    }

    /// The rule default for `idx` as a view into the attribute's own sealed
    /// storage, if the attribute has one.
    fn default_value(&self, idx: AttributeId) -> Option<CoercedAttrRef<'_>> {
        self.attribute_by_id(idx).default_as_ref()
    }

    pub fn from(
        attributes: Vec<(String, Attribute)>,
        is_anon: bool,
        cfg: &RuleIncomingTransition,
    ) -> buck2_error::Result<Self> {
        let internal_attrs = common_internal_attrs();

        let num_internal_attrs =
            internal_attrs.len() + usize::from(cfg == &RuleIncomingTransition::FromAttribute);
        let mut instances: BuckMutInsertionMap<Box<str>, Attribute> =
            BuckMutInsertionMap::with_capacity(attributes.len() + num_internal_attrs);
        for (name, instance) in internal_attrs {
            instances.insert((*name).into(), instance.clone());
        }

        if cfg == &RuleIncomingTransition::FromAttribute {
            instances.insert(
                INCOMING_TRANSITION_ATTRIBUTE.name.into(),
                (INCOMING_TRANSITION_ATTRIBUTE.attr)(),
            );
        }

        for (name, instance) in attributes.into_iter() {
            if is_anon {
                instance.coercer().validate_for_anon_rule()?;
            }

            if is_internal_attr(&name) {
                return Err(AttributeSpecError::InternalAttributeRedefined(name).into());
            }
            match instances.entry(name.into_boxed_str()) {
                Entry::Vacant(e) => {
                    e.insert(instance);
                }
                Entry::Occupied(e) => {
                    return Err(AttributeSpecError::DuplicateAttribute(e.key().to_string()).into());
                }
            }
        }

        AttributeSpec::new(instances)
    }

    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.attributes.len()
    }

    /// Number of attributes excluding internal attributes, i.e. the number of
    /// attributes declared in the `rule()` call. O(1).
    pub fn num_non_internal_attrs(&self) -> usize {
        // All common internal attrs are unconditionally present, and user
        // attrs may not shadow internal ones, so the subtraction is exact.
        let num_internal = common_internal_attrs().len()
            + usize::from(
                self.attributes
                    .contains_key(INCOMING_TRANSITION_ATTRIBUTE.name),
            );
        self.attributes.len() - num_internal
    }

    pub fn attr_specs(&self) -> impl ExactSizeIterator<Item = (&str, AttributeId, &Attribute)> {
        self.attributes
            .iter()
            .enumerate()
            .map(|(index_in_attribute_spec, (name, attribute))| {
                (
                    &**name,
                    AttributeId(index_in_attribute_spec as u16),
                    attribute,
                )
            })
    }

    fn attribute_by_id(&self, id: AttributeId) -> &Attribute {
        self.attributes.get_index(id.0 as usize).unwrap().1
    }

    pub fn attribute_name_by_id(&self, id: AttributeId) -> &str {
        self.attributes.get_index(id.0 as usize).unwrap().0
    }

    pub fn attribute_id_by_name(&self, name: &str) -> Option<AttributeId> {
        self.attributes
            .get_index_of(name)
            .map(|index_in_attribute_spec| AttributeId(index_in_attribute_spec as u16))
    }

    pub fn attribute(&self, name: &str) -> Option<&Attribute> {
        self.attributes.get(name)
    }

    /// Returns an iterator over all of the attribute (name, value) pairs.
    pub fn attrs<'v>(
        &'v self,
        attr_values: &'v AttrValues,
        opts: AttrInspectOptions,
    ) -> impl ExactSizeIterator<Item = CoercedAttrFull<'v>> {
        // The `filter_map` below yields every spec entry when defaults are
        // included, and exactly the explicitly-set entries otherwise, so the
        // length is known up front even though `FilterMap::size_hint` is not.
        let len = if opts.include_default() {
            self.len()
        } else {
            attr_values.len()
        };
        let mut pos = 0;
        let mut entry: Option<(AttributeId, CoercedAttrRef<'v>)> = attr_values.get_by_index(0);

        self.attr_specs()
            .filter_map(move |(name, idx, attr)| match entry {
                Some((entry_idx, entry_attr)) if entry_idx == idx => {
                    pos += 1;
                    entry = attr_values.get_by_index(pos);
                    Some(CoercedAttrFull {
                        name,
                        attr,
                        value: entry_attr,
                    })
                }
                _ => {
                    let value = self
                        .default_value(idx)
                        .unwrap_or_else(|| panic!("no default for attribute `{name}`"));
                    if opts.include_default() {
                        Some(CoercedAttrFull { name, attr, value })
                    } else {
                        None
                    }
                }
            })
            .with_exact_size(len)
    }

    /// Like [`Self::attrs`], but yielding owned handles that share the
    /// target's storage (or the attribute's own, for defaults), for holders
    /// that outlive the borrow of the node.
    pub fn owned_attrs<'v>(
        &'v self,
        attr_values: &'v AttrValues,
        opts: AttrInspectOptions,
    ) -> impl Iterator<Item = (&'v str, OwnedCoercedAttr)> + 'v {
        let mut pos = 0;
        let mut entry: Option<(AttributeId, OwnedCoercedAttr)> = attr_values.get_owned_by_index(0);

        self.attr_specs()
            .filter_map(move |(name, idx, _attr)| match entry.take() {
                Some((entry_idx, value)) if entry_idx == idx => {
                    pos += 1;
                    entry = attr_values.get_owned_by_index(pos);
                    Some((name, value))
                }
                cur => {
                    entry = cur;
                    if opts.include_default() {
                        let value = self
                            .attribute_by_id(idx)
                            .default_as_owned()
                            .unwrap_or_else(|| panic!("no default for attribute `{name}`"));
                        Some((name, value.dupe()))
                    } else {
                        None
                    }
                }
            })
    }

    /// Like [`Self::attrs`], but over a target's rows while its package is
    /// still evaluating, viewed through the package's arena builder: the
    /// `within_view` check reads them before the arena seals. Defaults come
    /// from each attribute's own arena.
    pub fn unsealed_attrs<'v>(
        &'v self,
        builder: &'v CoercedAttrArenaBuilder,
        attr_values: &'v UnsealedAttrValues,
        opts: AttrInspectOptions,
    ) -> impl Iterator<Item = CoercedAttrFull<'v>> {
        let mut rows = attr_values.iter();
        let mut entry = rows.next();

        self.attr_specs()
            .filter_map(move |(name, idx, attr)| match entry {
                Some((entry_idx, word)) if entry_idx == idx => {
                    entry = rows.next();
                    Some(CoercedAttrFull {
                        name,
                        attr,
                        value: builder.view(word),
                    })
                }
                _ => {
                    let value = self
                        .default_value(idx)
                        .unwrap_or_else(|| panic!("no default for attribute `{name}`"));
                    if opts.include_default() {
                        Some(CoercedAttrFull { name, attr, value })
                    } else {
                        None
                    }
                }
            })
    }

    pub fn known_attr_or_none<'v>(
        &'v self,
        idx: AttributeId,
        attr_values: &'v AttrValues,
        opts: AttrInspectOptions,
    ) -> Option<CoercedAttrFull<'v>> {
        let value = if let Some(attr) = attr_values.get(idx) {
            attr
        } else if opts.include_default() {
            self.default_value(idx)?
        } else {
            return None;
        };

        Some(CoercedAttrFull {
            name: self.attribute_name_by_id(idx),
            attr: self.attribute_by_id(idx),
            value,
        })
    }

    /// Like [`Self::attr_or_none`], but as an owned handle sharing the
    /// target's storage (or the attribute's own, for a default).
    pub fn owned_attr_or_none(
        &self,
        attr_values: &AttrValues,
        name: &str,
        opts: AttrInspectOptions,
    ) -> Option<OwnedCoercedAttr> {
        let idx = self.attribute_id_by_name(name)?;
        if let Some(owned) = attr_values.get_owned(idx) {
            return Some(owned);
        }
        if opts.include_default() {
            return self.attribute_by_id(idx).default_as_owned().duped();
        }
        None
    }

    pub fn attr_or_none<'v>(
        &'v self,
        attr_values: &'v AttrValues,
        name: &str,
        opts: AttrInspectOptions,
    ) -> Option<CoercedAttrFull<'v>> {
        if let Some(idx) = self.attribute_id_by_name(name) {
            self.known_attr_or_none(idx, attr_values, opts)
        } else {
            None
        }
    }

    pub fn attr<'v>(
        &'v self,
        attr_values: &'v AttrValues,
        key: &str,
        opts: AttrInspectOptions,
    ) -> buck2_error::Result<Option<CoercedAttrFull<'v>>> {
        if let Some(idx) = self.attribute_id_by_name(key) {
            Ok(self.known_attr_or_none(idx, attr_values, opts))
        } else {
            Err(AttributeSpecError::UnknownAttribute(key.to_owned()).into())
        }
    }
}

pub(crate) mod testing {
    use crate::attrs::attr::Attribute;
    use crate::attrs::spec::AttributeSpec;
    use crate::rule::RuleIncomingTransition;

    impl AttributeSpec {
        pub fn testing_new(attributes: Vec<(String, Attribute)>) -> AttributeSpec {
            AttributeSpec::from(attributes, false, &RuleIncomingTransition::None).unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::attrs::attr::Attribute;
    use crate::attrs::attr_type::AttrType;
    use crate::attrs::spec::AttributeSpec;
    use crate::rule::RuleIncomingTransition;

    #[test]
    fn test_num_non_internal_attrs() {
        let user_attrs = vec![
            (
                "foo".to_owned(),
                Attribute::new_const(None, "", AttrType::string()),
            ),
            (
                "bar".to_owned(),
                Attribute::new_const(None, "", AttrType::string()),
            ),
        ];

        let spec =
            AttributeSpec::from(user_attrs.clone(), false, &RuleIncomingTransition::None).unwrap();
        assert_eq!(2, spec.num_non_internal_attrs());

        let spec_with_transition =
            AttributeSpec::from(user_attrs, false, &RuleIncomingTransition::FromAttribute).unwrap();
        assert_eq!(2, spec_with_transition.num_non_internal_attrs());
        assert_eq!(spec.len() + 1, spec_with_transition.len());
    }
}
