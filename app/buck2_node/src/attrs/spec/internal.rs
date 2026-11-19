/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Builtin note attributes.

use std::sync::LazyLock;

use buck2_hash::BuckFixedInsertionMap;
use buck2_hash::BuckFixedSortedSet;

use crate::attrs::attr::Attribute;
use crate::attrs::attr_type::AttrType;
use crate::attrs::attr_type::configuration_dep::ConfigurationDepKind;
use crate::attrs::configurable::AttrIsConfigurable;
use crate::attrs::spec::AttributeId;
use crate::metadata::map::MetadataMap;
use crate::modifiers::CfgModifiers;
use crate::visibility::VisibilitySpecification;
use crate::visibility::WithinViewSpecification;

pub struct InternalAttribute {
    pub id: AttributeId,
    pub name: &'static str,
    attr: fn() -> Attribute,
    is_configurable: AttrIsConfigurable,
}

pub const NAME_ATTRIBUTE: InternalAttribute = InternalAttribute {
    id: AttributeId(0),
    name: "name",
    attr: || Attribute::new_const(None, "name of the target", AttrType::string()),
    is_configurable: AttrIsConfigurable::No,
};

pub(crate) const DEFAULT_TARGET_PLATFORM_ATTRIBUTE: InternalAttribute = InternalAttribute {
    id: AttributeId(1),
    name: "default_target_platform",
    attr: || {
        Attribute::new_const(
            Some(|b| b.encode_none()),
            "specifies the default target platform, used when no platforms are specified on the command line",
            AttrType::option(AttrType::configuration_dep(
                ConfigurationDepKind::DefaultTargetPlatform,
            )),
        )
    },
    is_configurable: AttrIsConfigurable::No,
};

/// buck1 used "compatible_with" for this. in buck2, we have two "compatible with" concepts, both
/// target and exec compatibility and so we are switching to "target_compatible_with". For now we'll accept
/// either form for target compatibility (but not both).
pub const TARGET_COMPATIBLE_WITH_ATTRIBUTE: InternalAttribute = InternalAttribute {
    id: AttributeId(2),
    name: "target_compatible_with",
    attr: || {
        Attribute::new_const(
            Some(|b| b.encode_list(&[])),
            "a list of constraints that are required to be satisfied for this target to be compatible with a configuration",
            AttrType::list(AttrType::configuration_dep(
                ConfigurationDepKind::CompatibilityAttribute,
            )),
        )
    },
    is_configurable: AttrIsConfigurable::Yes,
};

pub const LEGACY_TARGET_COMPATIBLE_WITH_ATTRIBUTE: InternalAttribute = InternalAttribute {
    id: AttributeId(3),
    name: "compatible_with",
    attr: || {
        Attribute::new_const(
            Some(|b| b.encode_list(&[])),
            "a list of constraints that are required to be satisfied for this target to be compatible with a configuration",
            AttrType::list(AttrType::configuration_dep(
                ConfigurationDepKind::CompatibilityAttribute,
            )),
        )
    },
    is_configurable: AttrIsConfigurable::Yes,
};

pub const EXEC_COMPATIBLE_WITH_ATTRIBUTE: InternalAttribute = InternalAttribute {
    id: AttributeId(4),
    name: "exec_compatible_with",
    attr: || {
        Attribute::new_const(
            Some(|b| b.encode_list(&[])),
            "a list of constraints that are required to be satisfied for this target to be compatible with an execution platform",
            AttrType::list(AttrType::configuration_dep(
                ConfigurationDepKind::CompatibilityAttribute,
            )),
        )
    },
    is_configurable: AttrIsConfigurable::Yes,
};

pub const VISIBILITY_ATTRIBUTE: InternalAttribute = InternalAttribute {
    id: AttributeId(5),
    name: "visibility",
    attr: || {
        Attribute::new_const(
            Some(|b| b.encode_visibility(&VisibilitySpecification::DEFAULT)),
            "a list of visibility patterns restricting what targets can depend on this one",
            AttrType::visibility(),
        )
    },
    // visibility attributes aren't configurable so that we can cache them on targetnodes.
    is_configurable: AttrIsConfigurable::No,
};

pub const WITHIN_VIEW_ATTRIBUTE: InternalAttribute = InternalAttribute {
    id: AttributeId(6),
    name: "within_view",
    attr: || {
        Attribute::new_const(
            Some(|b| b.encode_within_view(&WithinViewSpecification::PUBLIC)),
            "a list of visibility patterns restricting what this target can depend on",
            AttrType::within_view(),
        )
    },
    is_configurable: AttrIsConfigurable::No,
};

pub(crate) const METADATA_ATTRIBUTE: InternalAttribute = InternalAttribute {
    id: AttributeId(7),
    name: "metadata",
    attr: || {
        Attribute::new_const(
            Some(|b| b.encode_metadata(&MetadataMap::default())),
            "a key-value map of metadata associated with this target",
            AttrType::metadata(),
        )
    },
    is_configurable: AttrIsConfigurable::No,
};

pub(crate) const TESTS_ATTRIBUTE: InternalAttribute = InternalAttribute {
    id: AttributeId(8),
    name: "tests",
    attr: || {
        Attribute::new_const(
            Some(|b| b.encode_list(&[])),
            "a list of targets that provide tests for this one",
            AttrType::list(AttrType::label()),
        )
    },
    is_configurable: AttrIsConfigurable::Yes,
};

pub(crate) const TARGET_MODIFIERS_ATTRIBUTE: InternalAttribute = InternalAttribute {
    id: AttributeId(9),
    name: "modifiers",
    attr: || {
        Attribute::new_const(
            Some(|b| b.encode_target_modifiers(&CfgModifiers::empty())),
            "an array of modifiers associated with this target",
            AttrType::target_modifiers(),
        )
    },
    is_configurable: AttrIsConfigurable::No,
};

const INTERNAL_ATTRS: [InternalAttribute; 10] = [
    NAME_ATTRIBUTE,
    DEFAULT_TARGET_PLATFORM_ATTRIBUTE,
    TARGET_COMPATIBLE_WITH_ATTRIBUTE,
    LEGACY_TARGET_COMPATIBLE_WITH_ATTRIBUTE,
    EXEC_COMPATIBLE_WITH_ATTRIBUTE,
    VISIBILITY_ATTRIBUTE,
    WITHIN_VIEW_ATTRIBUTE,
    METADATA_ATTRIBUTE,
    TESTS_ATTRIBUTE,
    TARGET_MODIFIERS_ATTRIBUTE,
];

pub struct OptionalInternalAttribute {
    pub name: &'static str,
    pub(super) attr: fn() -> Attribute,
    is_configurable: AttrIsConfigurable,
}

pub const INCOMING_TRANSITION_ATTRIBUTE: OptionalInternalAttribute = OptionalInternalAttribute {
    name: "incoming_transition",
    attr: || {
        Attribute::new_const(
            Some(|b| b.encode_none()),
            "specifies the incoming transition to apply to this target",
            AttrType::option(AttrType::configuration_dep(
                ConfigurationDepKind::Transition,
            )),
        )
    },
    is_configurable: AttrIsConfigurable::No,
};

const fn to_optional(attr: InternalAttribute) -> OptionalInternalAttribute {
    OptionalInternalAttribute {
        name: attr.name,
        attr: attr.attr,
        is_configurable: attr.is_configurable,
    }
}

/// Includes optional internal attrs
const ALL_INTERNAL_ATTRS: [OptionalInternalAttribute; 11] = [
    to_optional(NAME_ATTRIBUTE),
    to_optional(DEFAULT_TARGET_PLATFORM_ATTRIBUTE),
    to_optional(TARGET_COMPATIBLE_WITH_ATTRIBUTE),
    to_optional(LEGACY_TARGET_COMPATIBLE_WITH_ATTRIBUTE),
    to_optional(EXEC_COMPATIBLE_WITH_ATTRIBUTE),
    to_optional(VISIBILITY_ATTRIBUTE),
    to_optional(WITHIN_VIEW_ATTRIBUTE),
    to_optional(METADATA_ATTRIBUTE),
    to_optional(TESTS_ATTRIBUTE),
    to_optional(TARGET_MODIFIERS_ATTRIBUTE),
    INCOMING_TRANSITION_ATTRIBUTE,
];

pub fn is_internal_attr(name: &str) -> bool {
    static ATTRS: LazyLock<BuckFixedSortedSet<&'static str>> =
        LazyLock::new(|| ALL_INTERNAL_ATTRS.iter().map(|attr| attr.name).collect());
    ATTRS.contains(name)
}

pub(super) fn common_internal_attrs() -> &'static BuckFixedInsertionMap<&'static str, Attribute> {
    static ATTRS: LazyLock<BuckFixedInsertionMap<&'static str, Attribute>> = LazyLock::new(|| {
        INTERNAL_ATTRS
            .iter()
            .map(|attr| (attr.name, (attr.attr)()))
            .collect()
    });
    &ATTRS
}

pub fn attr_is_configurable(name: &str) -> AttrIsConfigurable {
    for a in &ALL_INTERNAL_ATTRS {
        if a.name == name {
            return a.is_configurable;
        }
    }
    AttrIsConfigurable::Yes
}

#[cfg(test)]
mod tests {
    use crate::attrs::spec::internal::INTERNAL_ATTRS;
    use crate::attrs::spec::internal::common_internal_attrs;

    #[test]
    fn verify_attr_ids() {
        for (i, a) in INTERNAL_ATTRS.iter().enumerate() {
            assert_eq!(a.id.0 as usize, i);
            assert_eq!(common_internal_attrs().get_index_of(a.name), Some(i));
        }
    }
}
