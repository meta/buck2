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
)

//go:embed templates
var tmplAll embed.FS

//go:embed templates/*.tmpl
var tmplNoSubDir embed.FS

//go:embed templates/**/*
var tmplOnlySubDir embed.FS

func main() {
	all, err := fspkg.Glob(tmplAll, "templates/*.tmpl")
	if err != nil {
		log.Fatal(err)
	}
	allSub, err := fspkg.Glob(tmplAll, "templates/**/*.tmpl")
	if err != nil {
		log.Fatal(err)
	}

	noSub, err := fspkg.Glob(tmplNoSubDir, "templates/*.tmpl")
	if err != nil {
		log.Fatal(err)
	}

	onlySub, err := fspkg.Glob(tmplOnlySubDir, "templates/**/*.tmpl")
	if err != nil {
		log.Fatal(err)
	}

	fmt.Printf("all=%d,nosubdir=%d,onlysubdir=%d", len(all)+len(allSub), len(noSub), len(onlySub))
}
