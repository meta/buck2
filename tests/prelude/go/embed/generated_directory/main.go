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
	"embed"
	"fmt"
	fspkg "io/fs"
	"log"
	"strings"
)

//go:embed dir0
var dir0All embed.FS

//go:embed dir0/dir1/*.txt
var dir0NoSubDir embed.FS

//go:embed dir0/dir1/**/*
var dir0OnlySubDir embed.FS

func main() {
	all, err := fspkg.Glob(dir0All, "dir0/dir1/*.txt")
	if err != nil {
		log.Fatal(err)
	}
	allSub, err := fspkg.Glob(dir0All, "dir0/dir1/**/*.txt")
	if err != nil {
		log.Fatal(err)
	}

	noSub, err := fspkg.Glob(dir0NoSubDir, "dir0/dir1/*.txt")
	if err != nil {
		log.Fatal(err)
	}

	onlySub, err := fspkg.Glob(dir0OnlySubDir, "dir0/dir1/**/*.txt")
	if err != nil {
		log.Fatal(err)
	}

	content1, err := dir0All.ReadFile("dir0/dir1/file1.txt")
	if err != nil {
		log.Fatal(err)
	}
	content2, err := dir0All.ReadFile("dir0/dir1/dir2/file2.txt")
	if err != nil {
		log.Fatal(err)
	}

	fmt.Printf("all=%d,nosubdir=%d,onlysubdir=%d,file1=%s,file2=%s",
		len(all)+len(allSub), len(noSub), len(onlySub),
		strings.TrimSpace(string(content1)), strings.TrimSpace(string(content2)))
}
