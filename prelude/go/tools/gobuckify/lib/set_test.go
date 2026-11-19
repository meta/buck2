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

import (
	"reflect"
	"testing"
)

func TestNewSet(t *testing.T) {
	s := NewSet()
	if s == nil {
		t.Fatal("NewSet() returned nil")
	}
	if len(*s) != 0 {
		t.Errorf("Expected empty set, got set with %d elements", len(*s))
	}
}

func TestAdd(t *testing.T) {
	s := NewSet()

	// Add a single element
	s.Add("test1")
	if len(*s) != 1 {
		t.Errorf("Expected set with 1 element, got %d", len(*s))
	}
	if _, exists := (*s)["test1"]; !exists {
		t.Errorf("Element 'test1' not found in set after Add")
	}

	// Add the same element again (should not increase size)
	s.Add("test1")
	if len(*s) != 1 {
		t.Errorf("Expected set with 1 element after adding duplicate, got %d", len(*s))
	}

	// Add a different element
	s.Add("test2")
	if len(*s) != 2 {
		t.Errorf("Expected set with 2 elements, got %d", len(*s))
	}
	if _, exists := (*s)["test2"]; !exists {
		t.Errorf("Element 'test2' not found in set after Add")
	}
}

func TestRemove(t *testing.T) {
	s := NewSet()

	// Add and then remove an element
	s.Add("test1")
	s.Remove("test1")
	if len(*s) != 0 {
		t.Errorf("Expected empty set after removing element, got %d elements", len(*s))
	}
	if _, exists := (*s)["test1"]; exists {
		t.Errorf("Element 'test1' still found in set after Remove")
	}

	// Remove an element that doesn't exist (should not error)
	s.Remove("nonexistent")
	if len(*s) != 0 {
		t.Errorf("Expected empty set after removing nonexistent element, got %d elements", len(*s))
	}

	// Add multiple elements and remove one
	s.Add("test1")
	s.Add("test2")
	s.Remove("test1")
	if len(*s) != 1 {
		t.Errorf("Expected set with 1 element after removing one of two elements, got %d", len(*s))
	}
	if _, exists := (*s)["test1"]; exists {
		t.Errorf("Element 'test1' still found in set after Remove")
	}
	if _, exists := (*s)["test2"]; !exists {
		t.Errorf("Element 'test2' not found in set after removing different element")
	}
}

func TestLen(t *testing.T) {
	s := NewSet()

	// Empty set
	if s.Len() != 0 {
		t.Errorf("Expected length 0 for empty set, got %d", s.Len())
	}

	// Set with one element
	s.Add("test1")
	if s.Len() != 1 {
		t.Errorf("Expected length 1 after adding element, got %d", s.Len())
	}

	// Set with multiple elements
	s.Add("test2")
	s.Add("test3")
	if s.Len() != 3 {
		t.Errorf("Expected length 3 after adding three elements, got %d", s.Len())
	}

	// After removing an element
	s.Remove("test2")
	if s.Len() != 2 {
		t.Errorf("Expected length 2 after removing element, got %d", s.Len())
	}
}

func TestSortedList(t *testing.T) {
	s := NewSet()

	// Empty set
	list := s.SortedList()
	if len(list) != 0 {
		t.Errorf("Expected empty list for empty set, got list with %d elements", len(list))
	}

	// Set with one element
	s.Add("test1")
	list = s.SortedList()
	expected := []string{"test1"}
	if !reflect.DeepEqual(list, expected) {
		t.Errorf("Expected %v, got %v", expected, list)
	}

	// Set with multiple elements (added in non-alphabetical order)
	s.Add("test3")
	s.Add("test2")
	list = s.SortedList()
	expected = []string{"test1", "test2", "test3"}
	if !reflect.DeepEqual(list, expected) {
		t.Errorf("Expected %v, got %v", expected, list)
	}

	// Verify the original set is not modified
	if s.Len() != 3 {
		t.Errorf("Expected set to still have 3 elements, got %d", s.Len())
	}
}

func TestSetOperations(t *testing.T) {
	// Test multiple operations together
	s := NewSet()

	// Add elements
	s.Add("apple")
	s.Add("banana")
	s.Add("cherry")

	// Verify length
	if s.Len() != 3 {
		t.Errorf("Expected length 3, got %d", s.Len())
	}

	// Verify sorted list
	expected := []string{"apple", "banana", "cherry"}
	list := s.SortedList()
	if !reflect.DeepEqual(list, expected) {
		t.Errorf("Expected %v, got %v", expected, list)
	}

	// Remove an element
	s.Remove("banana")

	// Verify length after removal
	if s.Len() != 2 {
		t.Errorf("Expected length 2 after removal, got %d", s.Len())
	}

	// Verify sorted list after removal
	expected = []string{"apple", "cherry"}
	list = s.SortedList()
	if !reflect.DeepEqual(list, expected) {
		t.Errorf("Expected %v, got %v", expected, list)
	}
}
