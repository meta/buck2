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
	"strings"
	"testing"
)

func TestGeneratedDirectoryEmbed(t *testing.T) {
	all, err := fspkg.Glob(dir0All, "dir0/dir1/*.txt")
	if err != nil {
		t.Fatal(err)
	}
	if len(all) != 1 {
		t.Errorf("expected 1 file in dir0/dir1/*.txt for dir0All, got %d", len(all))
	}

	allSub, err := fspkg.Glob(dir0All, "dir0/dir1/**/*.txt")
	if err != nil {
		t.Fatal(err)
	}
	if len(allSub) != 1 {
		t.Errorf("expected 1 file in dir0/dir1/**/*.txt for dir0All, got %d", len(allSub))
	}
}

func TestGeneratedDirectoryNoSubDir(t *testing.T) {
	noSub, err := fspkg.Glob(dir0NoSubDir, "dir0/dir1/*.txt")
	if err != nil {
		t.Fatal(err)
	}
	if len(noSub) != 1 {
		t.Errorf("expected 1 file in dir0/dir1/*.txt for dir0NoSubDir, got %d", len(noSub))
	}
}

func TestGeneratedDirectoryOnlySubDir(t *testing.T) {
	onlySub, err := fspkg.Glob(dir0OnlySubDir, "dir0/dir1/**/*.txt")
	if err != nil {
		t.Fatal(err)
	}
	if len(onlySub) != 1 {
		t.Errorf("expected 1 file in dir0/dir1/**/*.txt for dir0OnlySubDir, got %d", len(onlySub))
	}
}

func TestGeneratedDirectoryFileContent(t *testing.T) {
	content1, err := dir0All.ReadFile("dir0/dir1/file1.txt")
	if err != nil {
		t.Fatalf("failed to read dir0/dir1/file1.txt: %v", err)
	}
	if strings.TrimSpace(string(content1)) != "content 1" {
		t.Errorf("expected file1.txt content to be 'content 1', got %q", strings.TrimSpace(string(content1)))
	}

	content2, err := dir0All.ReadFile("dir0/dir1/dir2/file2.txt")
	if err != nil {
		t.Fatalf("failed to read dir0/dir1/dir2/file2.txt: %v", err)
	}
	if strings.TrimSpace(string(content2)) != "content 2" {
		t.Errorf("expected file2.txt content to be 'content 2', got %q", strings.TrimSpace(string(content2)))
	}
}
