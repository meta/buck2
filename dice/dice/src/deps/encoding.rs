/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! We pack the SeriesParallelDeps graph descriptors (`SPItem` and `SPSeriesHeader`) into
//! a `Vec<u32>` to minimize the cost of storing them.
//!
//! This module contains the utilities for encoding and decoding those.
//!
//! The basic structure is
//! ```ignore
//!   <4 bit tag> <u28 value>
//! ```

use std::fmt::Debug;
use std::fmt::Display;

use dupe::Dupe;
use itertools::Itertools;
use mini_vec::MiniVec;
use thiserror::Error;
use variant_name::VariantName;

use crate::deps::graph::SPItem;
use crate::deps::graph::SPSeriesHeader;

pub(crate) trait SPEncoder {
    fn write_series_header(&mut self, header: SPSeriesHeader);
    fn write_item(&mut self, item: SPItem);
}

#[derive(VariantName, Eq, PartialEq, Clone, Copy, Dupe)]
pub(crate) enum SPTag {
    HeaderSimple,
    HeaderComplexKeys,
    HeaderComplexSpecs,
    ItemKeys,
    ItemParallelKeys,
    ItemParallelSpecs,
}

impl SPTag {
    fn encode(self) -> u32 {
        match self {
            SPTag::HeaderSimple => 0,
            SPTag::HeaderComplexKeys => 1,
            SPTag::HeaderComplexSpecs => 2,
            SPTag::ItemKeys => 3,
            SPTag::ItemParallelKeys => 4,
            SPTag::ItemParallelSpecs => 5,
        }
    }

    fn decode(v: u32) -> Result<Self, SPDecoderError> {
        let r = match v {
            0 => SPTag::HeaderSimple,
            1 => SPTag::HeaderComplexKeys,
            2 => SPTag::HeaderComplexSpecs,
            3 => SPTag::ItemKeys,
            4 => SPTag::ItemParallelKeys,
            5 => SPTag::ItemParallelSpecs,
            v => {
                return Err(SPDecoderError::InvalidTag(v));
            }
        };
        assert!(r.encode() == v);
        Ok(r)
    }
}

impl Display for SPTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.variant_name())
    }
}

impl Debug for SPTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self, f)
    }
}

fn push(vec: &mut MiniVec<u32>, tag: SPTag, value: u32) {
    assert!(value < (1 << 28));
    vec.push((tag.encode() << 28) | value);
}

impl SPEncoder for MiniVec<u32> {
    fn write_series_header(&mut self, header: SPSeriesHeader) {
        match header {
            SPSeriesHeader::Simple { key_count } => {
                push(self, SPTag::HeaderSimple, key_count);
            }
            SPSeriesHeader::Complex {
                key_count,
                spec_count,
            } => {
                push(self, SPTag::HeaderComplexKeys, key_count);
                push(self, SPTag::HeaderComplexSpecs, spec_count);
            }
        }
    }

    fn write_item(&mut self, item: SPItem) {
        match item {
            SPItem::Keys { key_count } => {
                push(self, SPTag::ItemKeys, key_count);
            }
            SPItem::Parallel {
                key_count,
                spec_count,
            } => {
                push(self, SPTag::ItemParallelKeys, key_count);
                push(self, SPTag::ItemParallelSpecs, spec_count);
            }
        }
    }
}

fn debug_string<'a>(iter: impl Iterator<Item = &'a u32>) -> String {
    iter.map(|v| SPDecoder::split(*v))
        .map(|(x, y)| format!("({x:#04b}, {y})"))
        .join(",")
}

impl SPSeriesHeader {
    pub(crate) fn encoded_len(&self) -> u32 {
        match self {
            SPSeriesHeader::Simple { .. } => 1,
            SPSeriesHeader::Complex { spec_count, .. } => 2 + spec_count,
        }
    }
}

impl SPItem {
    pub(crate) fn encoded_len(&self) -> u32 {
        match self {
            SPItem::Keys { .. } => 1,
            SPItem::Parallel { .. } => 2,
        }
    }
}

pub(crate) struct SPDecoder<'a>(pub(crate) std::slice::Iter<'a, u32>);

impl SPDecoder<'_> {
    pub(crate) fn read_series_header(&mut self) -> Result<Option<SPSeriesHeader>, SPDecoderError> {
        self.read()?
            .map(|v| match v {
                (SPTag::HeaderSimple, key_count) => Ok(SPSeriesHeader::Simple { key_count }),
                (SPTag::HeaderComplexKeys, key_count) => match self.read()? {
                    Some((SPTag::HeaderComplexSpecs, spec_count)) => Ok(SPSeriesHeader::Complex {
                        key_count,
                        spec_count,
                    }),
                    v => Err(SPDecoderError::InvalidItem {
                        expected: vec![SPTag::HeaderComplexSpecs],
                        actual: v,
                        remaining: debug_string(&mut self.0),
                    }),
                },
                v => Err(SPDecoderError::InvalidItem {
                    expected: vec![SPTag::HeaderSimple, SPTag::HeaderComplexKeys],
                    actual: Some(v),
                    remaining: debug_string(&mut self.0),
                }),
            })
            .transpose()
    }

    pub(crate) fn read_item(&mut self) -> Result<Option<SPItem>, SPDecoderError> {
        self.read()?
            .map(|v| match v {
                (SPTag::ItemKeys, key_count) => Ok(SPItem::Keys { key_count }),
                (SPTag::ItemParallelKeys, key_count) => match self.read()? {
                    Some((SPTag::ItemParallelSpecs, spec_count)) => Ok(SPItem::Parallel {
                        key_count,
                        spec_count,
                    }),
                    v => Err(SPDecoderError::InvalidItem {
                        expected: vec![SPTag::ItemParallelSpecs],
                        actual: v,
                        remaining: debug_string(&mut self.0),
                    }),
                },
                v => Err(SPDecoderError::InvalidItem {
                    expected: vec![SPTag::ItemKeys, SPTag::ItemParallelKeys],
                    actual: Some(v),
                    remaining: debug_string(&mut self.0),
                }),
            })
            .transpose()
    }

    fn read(&mut self) -> Result<Option<(SPTag, u32)>, SPDecoderError> {
        self.0
            .next()
            .map(|v| {
                let (tag, val) = Self::split(*v);
                Ok((SPTag::decode(tag)?, val))
            })
            .transpose()
    }

    fn split(v: u32) -> (u32, u32) {
        (v >> 28, v & 0x0FFFFFFF)
    }

    pub(crate) fn split_at(&mut self, specs: u32) -> Self {
        let remaining_specs = self.0.as_slice();
        let (split_specs, remaining_specs) = remaining_specs.split_at(specs as usize);
        self.0 = remaining_specs.iter();
        SPDecoder(split_specs.iter())
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub(crate) enum SPDecoderError {
    #[error(
        "error decoding series-parallel graph, expected one of <{}> tags, got {:?}, remaining <{}>",
        .expected.iter().join(","),
        .actual,
        .remaining
    )]
    InvalidItem {
        expected: Vec<SPTag>,
        actual: Option<(SPTag, u32)>,
        remaining: String,
    },
    #[error("error decoding series-parallel graph, got invalid tag `{0}`")]
    InvalidTag(u32),
}

#[cfg(test)]
mod test {
    use mini_vec::MiniVec;

    use crate::deps::encoding::SPDecoder;
    use crate::deps::encoding::SPEncoder;
    use crate::deps::graph::SPItem;
    use crate::deps::graph::SPSeriesHeader;

    #[test]
    fn encoding_and_decoding_works() -> anyhow::Result<()> {
        let mut encoded = MiniVec::new();

        let item1 = SPItem::Keys { key_count: 3 };
        let item2 = SPItem::Keys { key_count: 2 };
        let item3 = SPItem::Parallel {
            key_count: 100,
            spec_count: 101,
        };

        let header1 = SPSeriesHeader::Simple { key_count: 2 };
        let header2 = SPSeriesHeader::Complex {
            key_count: 200,
            spec_count: 201,
        };
        let header3 = SPSeriesHeader::Simple { key_count: 4 };

        encoded.write_item(item1);
        encoded.write_item(item2);

        encoded.write_series_header(header1);
        encoded.write_series_header(header2);

        encoded.write_item(item3);
        encoded.write_series_header(header3);

        let mut decoder = SPDecoder(encoded.iter());

        assert_eq!(decoder.read_item(), Ok(Some(item1)));
        assert_eq!(decoder.read_item(), Ok(Some(item2)));
        assert_eq!(decoder.read_series_header(), Ok(Some(header1)));
        assert_eq!(decoder.read_series_header(), Ok(Some(header2)));
        assert_eq!(decoder.read_item(), Ok(Some(item3)));
        assert_eq!(decoder.read_series_header(), Ok(Some(header3)));
        assert_eq!(decoder.read_item(), Ok(None));

        Ok(())
    }

    #[test]
    fn decoder_returns_errors_on_wrong_type() -> anyhow::Result<()> {
        {
            let mut encoded = MiniVec::new();
            encoded.write_item(SPItem::Keys { key_count: 1 });
            assert!(SPDecoder(encoded.iter()).read_series_header().is_err());
        }
        {
            let mut encoded = MiniVec::new();
            encoded.write_item(SPItem::Parallel {
                key_count: 100,
                spec_count: 101,
            });
            assert!(SPDecoder(encoded.iter()).read_series_header().is_err());
        }
        {
            let mut encoded = MiniVec::new();
            encoded.write_series_header(SPSeriesHeader::Simple { key_count: 1 });
            assert!(SPDecoder(encoded.iter()).read_item().is_err());
        }
        {
            let mut encoded = MiniVec::new();
            encoded.write_series_header(SPSeriesHeader::Complex {
                key_count: 100,
                spec_count: 101,
            });
            assert!(SPDecoder(encoded.iter()).read_item().is_err());
        }

        Ok(())
    }
}
