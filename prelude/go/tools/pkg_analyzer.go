/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

// pkg_analyzer is a tool that provides package analysis generation
// for Go packages. It serves as a replacement for "go list" in Buck2 builds,
// providing the exact output format expected by go_list.bzl.
//
// Usage:
//
//	pkg_analyzer [flags] <directory>
package main

import (
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"go/build"
	"os"
	"path/filepath"
	"strings"
)

// AnalyzerOutput represents the unified JSON output containing both
// go list results and embedcfg for the Go compiler.
type AnalyzerOutput struct {
	// Package information (matches go list -json output expected by go_list.bzl)
	Name              string   `json:"Name"`
	Imports           []string `json:"Imports,omitempty"`
	TestImports       []string `json:"TestImports,omitempty"`
	XTestImports      []string `json:"XTestImports,omitempty"`
	GoFiles           []string `json:"GoFiles,omitempty"`
	HFiles            []string `json:"HFiles,omitempty"`
	CFiles            []string `json:"CFiles,omitempty"`
	CXXFiles          []string `json:"CXXFiles,omitempty"`
	CgoFiles          []string `json:"CgoFiles,omitempty"`
	SFiles            []string `json:"SFiles,omitempty"`
	SysoFiles         []string `json:"SysoFiles,omitempty"`
	TestGoFiles       []string `json:"TestGoFiles,omitempty"`
	XTestGoFiles      []string `json:"XTestGoFiles,omitempty"`
	IgnoredGoFiles    []string `json:"IgnoredGoFiles,omitempty"`
	IgnoredOtherFiles []string `json:"IgnoredOtherFiles,omitempty"`
	CgoCFLAGS         []string `json:"CgoCFLAGS,omitempty"`
	CgoCPPFLAGS       []string `json:"CgoCPPFLAGS,omitempty"`
	EmbedPatterns     []string `json:"EmbedPatterns,omitempty"`
	TestEmbedPatterns []string `json:"TestEmbedPatterns,omitempty"`

	// Error information (matches go list -e output)
	Error *PackageError `json:"Error,omitempty"`
}

// PackageError describes an error loading information about a package.
type PackageError struct {
	Err string `json:"Err"`
}

func main() {
	tags := flag.String("tags", "", "Build tags (comma-separated)")
	cgoEnabled := flag.Bool("cgo", false, "Enable CGO")
	withTests := flag.Bool("tests", false, "Include test files")
	race := flag.Bool("race", false, "Enable race detection")
	asan := flag.Bool("asan", false, "Enable address sanitizer")
	goos := flag.String("goos", "", "Target GOOS (default: current OS)")
	goarch := flag.String("goarch", "", "Target GOARCH (default: current arch)")
	output := flag.String("o", "", "Output file (default: stdout)")
	flag.Usage = func() {
		fmt.Fprintf(os.Stderr, `Usage: pkg_analyzer [flags] <directory>

Analyze a Go package and output JSON containing package information
(matching go list -json format). Optionally outputs embedcfg for the
Go compiler to a separate file via -embedcfg flag.

Flags:
`)
		flag.PrintDefaults()
	}

	flag.Parse()

	if flag.NArg() != 1 {
		flag.Usage()
		os.Exit(1)
	}

	dir := flag.Arg(0)
	absDir, err := filepath.Abs(dir)
	if err != nil {
		fmt.Fprintf(os.Stderr, "pkg_analyzer: failed to resolve directory: %v\n", err)
		os.Exit(1)
	}

	// Build the context with the specified settings
	ctx := build.Default
	if *goos != "" {
		ctx.GOOS = *goos
	}
	if *goarch != "" {
		ctx.GOARCH = *goarch
	}
	ctx.CgoEnabled = *cgoEnabled

	// Parse build tags
	var buildTags []string
	if *tags != "" {
		buildTags = strings.Split(*tags, ",")
	}
	if *race {
		buildTags = append(buildTags, "race")
	}
	if *asan {
		buildTags = append(buildTags, "asan")
	}
	ctx.BuildTags = buildTags

	// Import the package
	pkg, err := ctx.ImportDir(absDir, build.ImportComment|build.IgnoreVendor)
	var pkgErr *PackageError
	if err != nil {
		var noGoErr *build.NoGoError
		if errors.As(err, &noGoErr) {
			// TODO(michaelpo): Skip unil we are ready to handle no-go errors.
			// We likely be skippling this in the future as well and return an error directly in the build-rules.
		} else {
			pkgErr = &PackageError{
				Err: err.Error(),
			}
		}
	}

	// Build the output
	out := AnalyzerOutput{
		Name:              pkg.Name,
		Imports:           pkg.Imports,
		GoFiles:           pkg.GoFiles,
		CgoFiles:          pkg.CgoFiles,
		HFiles:            pkg.HFiles,
		CFiles:            pkg.CFiles,
		CXXFiles:          pkg.CXXFiles,
		SFiles:            pkg.SFiles,
		SysoFiles:         pkg.SysoFiles,
		IgnoredGoFiles:    pkg.IgnoredGoFiles,
		IgnoredOtherFiles: pkg.IgnoredOtherFiles,
		CgoCFLAGS:         pkg.CgoCFLAGS,
		CgoCPPFLAGS:       pkg.CgoCPPFLAGS,
		EmbedPatterns:     pkg.EmbedPatterns,
		Error:             pkgErr,
	}

	// Include test files if requested
	if *withTests {
		out.TestGoFiles = pkg.TestGoFiles
		out.XTestGoFiles = pkg.XTestGoFiles
		out.TestImports = pkg.TestImports
		out.TestEmbedPatterns = pkg.TestEmbedPatterns
	}

	jsonData, err := json.MarshalIndent(out, "", "\t")
	if err != nil {
		fmt.Fprintf(os.Stderr, "pkg_analyzer: failed to marshal JSON: %v\n", err)
		os.Exit(1)
	}

	if *output != "" {
		if err := os.WriteFile(*output, jsonData, 0644); err != nil {
			fmt.Fprintf(os.Stderr, "pkg_analyzer: failed to write output file: %v\n", err)
			os.Exit(1)
		}
	} else {
		fmt.Println(string(jsonData))
	}
}
