/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package main

import (
	fspkg "io/fs"
	"testing"
)

func TestEmbedDirectory(t *testing.T) {
	all, err := fspkg.Glob(tmplAll, "templates/*.tmpl")
	if err != nil {
		t.Fatal(err)
	}
	if len(all) != 2 {
		t.Errorf("expected 2 files in templates/*.tmpl for tmplAll, got %d", len(all))
	}

	allSub, err := fspkg.Glob(tmplAll, "templates/**/*.tmpl")
	if err != nil {
		t.Fatal(err)
	}
	if len(allSub) != 1 {
		t.Errorf("expected 1 file in templates/**/*.tmpl for tmplAll, got %d", len(allSub))
	}
}

func TestEmbedNoSubDir(t *testing.T) {
	noSub, err := fspkg.Glob(tmplNoSubDir, "templates/*.tmpl")
	if err != nil {
		t.Fatal(err)
	}
	if len(noSub) != 2 {
		t.Errorf("expected 2 files in templates/*.tmpl for tmplNoSubDir, got %d", len(noSub))
	}
}

func TestEmbedOnlySubDir(t *testing.T) {
	onlySub, err := fspkg.Glob(tmplOnlySubDir, "templates/**/*.tmpl")
	if err != nil {
		t.Fatal(err)
	}
	if len(onlySub) != 1 {
		t.Errorf("expected 1 file in templates/**/*.tmpl for tmplOnlySubDir, got %d", len(onlySub))
	}
}

func TestEmbedFileContent(t *testing.T) {
	content, err := tmplAll.ReadFile("templates/main.go.tmpl")
	if err != nil {
		t.Fatalf("failed to read templates/main.go.tmpl: %v", err)
	}
	if len(content) == 0 {
		t.Error("templates/main.go.tmpl is empty")
	}

	subContent, err := tmplAll.ReadFile("templates/subdir/subtemplate.tmpl")
	if err != nil {
		t.Fatalf("failed to read templates/subdir/subtemplate.tmpl: %v", err)
	}
	if len(subContent) == 0 {
		t.Error("templates/subdir/subtemplate.tmpl is empty")
	}
}
