/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::core::graph::revision::Revision;
use crate::deps::encoding::SPDecoder;
use crate::deps::graph::DepEdge;
use crate::deps::graph::SPItem;
use crate::deps::graph::SPSeriesHeader;

pub(crate) enum SeriesParallelDepsIteratorItem<'a, R = Revision> {
    Key(DepEdge<R>),
    Parallel(ParallelNodeIterator<'a, R>),
}

struct IteratorData<'a, R> {
    deps: std::slice::Iter<'a, DepEdge<R>>,
    specs: SPDecoder<'a>,
}

pub(crate) struct SeriesNodeIterator<'a, R = Revision> {
    data: IteratorData<'a, R>,
    keys_to_next_spec: u32,
}

impl<R> SeriesNodeIterator<'_, R> {
    pub(crate) fn new<'a>(
        deps: std::slice::Iter<'a, DepEdge<R>>,
        specs: std::slice::Iter<'a, u32>,
    ) -> SeriesNodeIterator<'a, R> {
        SeriesNodeIterator {
            data: IteratorData {
                deps,
                specs: SPDecoder(specs),
            },
            keys_to_next_spec: 0,
        }
    }
}

impl<'a, R: Copy> IteratorData<'a, R> {
    fn split_at(&mut self, keys: u32, specs: u32) -> IteratorData<'a, R> {
        let remaining_keys = self.deps.as_slice();
        let (split_keys, remaining_keys) = remaining_keys.split_at(keys as usize);
        self.deps = remaining_keys.iter();

        let split_specs = self.specs.split_at(specs);
        IteratorData {
            deps: split_keys.iter(),
            specs: split_specs,
        }
    }

    fn next_edge(&mut self) -> Option<DepEdge<R>> {
        self.deps.next().copied()
    }
}

impl<'a, R: Copy> Iterator for SeriesNodeIterator<'a, R> {
    type Item = SeriesParallelDepsIteratorItem<'a, R>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.keys_to_next_spec == 0 {
            match self.data.specs.read_item().unwrap() {
                Some(SPItem::Keys { key_count }) => {
                    self.keys_to_next_spec = key_count;
                }
                Some(SPItem::Parallel {
                    key_count,
                    spec_count,
                }) => {
                    let parallel_data = self.data.split_at(key_count, spec_count);
                    return Some(SeriesParallelDepsIteratorItem::Parallel(
                        ParallelNodeIterator {
                            data: parallel_data,
                        },
                    ));
                }
                None => {
                    // drain the trailing keys
                    self.keys_to_next_spec = u32::MAX;
                }
            }
        }
        self.keys_to_next_spec -= 1;
        self.data
            .next_edge()
            .map(SeriesParallelDepsIteratorItem::Key)
    }
}

pub(crate) struct ParallelNodeIterator<'a, R = Revision> {
    data: IteratorData<'a, R>,
}

impl<'a, R: Copy> Iterator for ParallelNodeIterator<'a, R> {
    type Item = SeriesNodeIterator<'a, R>;

    fn next(&mut self) -> Option<Self::Item> {
        let (next_series_keys, next_series_specs) =
            match self.data.specs.read_series_header().unwrap() {
                None => {
                    return None;
                }
                Some(SPSeriesHeader::Complex {
                    key_count,
                    spec_count,
                }) => (key_count, spec_count),
                Some(SPSeriesHeader::Simple { key_count }) => (key_count, 0),
            };

        let next_series_data = self.data.split_at(next_series_keys, next_series_specs);
        Some(SeriesNodeIterator {
            data: next_series_data,
            keys_to_next_spec: 0,
        })
    }
}
