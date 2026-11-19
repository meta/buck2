/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package gobuckifylib

import "slices"

type StringSet map[string]struct{}

func NewSet() *StringSet {
	newSet := make(StringSet)
	return &newSet
}

func NewFromList(vs []string) *StringSet {
	newSet := NewSet()
	newSet.AddList(vs)
	return newSet
}

func (s *StringSet) Add(v string) {
	(*s)[v] = struct{}{}
}

func (s *StringSet) AddList(vs []string) {
	for _, v := range vs {
		(*s)[v] = struct{}{}
	}
}

func (s *StringSet) Remove(v string) {
	delete((*s), v)
}

func (s *StringSet) Len() int {
	return len((*s))
}

func (s *StringSet) SortedList() []string {
	result := make([]string, 0, len((*s)))
	for k := range *s {
		result = append(result, k)
	}
	slices.Sort(result)
	return result
}
