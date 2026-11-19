/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package driver

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"maps"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"

	"golang.org/x/tools/go/packages"
)

// LoadModeTypecheckCgo is a hack to support cgo in gopls
// We declare a copy of private value that for some reason is used in gopls
// Ideally we should not do this, but it's the only way to support cgo in gopls
const LoadModeTypecheckCgo = packages.LoadMode(4096)

const cgoGenFileNameExt = ".cgo1.go"

// queryBXL is a wrapper around query that will use BXL to resolve the targets
func queryBXL(
	ctx context.Context,
	req *packages.DriverRequest,
	bucker Bucker,
	patterns []string,
	files []string,
) (*packages.DriverResponse, error) {
	if len(patterns) == 0 && len(files) == 0 {
		return &packages.DriverResponse{}, nil
	}

	bxlArgs := buildBXLArgs(req, patterns, files)

	bxlOut, err := bucker.BXL(ctx, "prelude//go/tools/gopackagesdriver:driver.bxl:driver", bxlArgs)
	if err != nil {
		var ee *exec.ExitError
		if errors.As(err, &ee) {
			// Create ad-hoc package with information about the error
			// This is likely will be done inside BXL
			// once  https://fburl.com/workplace/q79a59rn implemented
			actionErrors := retriveActionErrors(ee.Stderr)
			if len(actionErrors) > 0 {
				slog.Warn("failed to query BXL, but we have action errors", "err", err)
				return &packages.DriverResponse{
					Roots: []string{"ad-hoc-error-recovery"},
					Packages: []*packages.Package{{
						ID:              "ad-hoc-error-recovery",
						GoFiles:         files,
						CompiledGoFiles: files,
						Errors: []packages.Error{{
							Msg:  actionErrors,
							Kind: packages.ListError,
						}},
					}},
				}, nil
			}
		}

		return nil, err
	}

	filePath := strings.TrimSuffix(string(bxlOut), "\n")

	file, err := os.ReadFile(filePath)
	if err != nil {
		return nil, err
	}
	slog.Debug("Response from query BXL", "file", file)

	var response packages.DriverResponse
	err = json.Unmarshal(file, &response)
	if err != nil {
		return nil, err
	}

	rewrites := make(map[string]string)
	for _, pkg := range response.Packages {
		pkgRewrites, err := cgoPathRewrites(pkg)
		if err != nil {
			return nil, err
		}
		for generatedFile, sourceFile := range pkgRewrites {
			if _, ok := rewrites[generatedFile]; ok {
				return nil, fmt.Errorf("duplicate CGo path rewrite for %q", generatedFile)
			}
			rewrites[generatedFile] = sourceFile
		}
	}
	for generatedFile, sourceFile := range rewrites {
		if err := fixRePath(generatedFile, sourceFile); err != nil {
			return nil, fmt.Errorf("rewrite CGo source path for %q: %w", generatedFile, err)
		}
	}

	return &response, nil
}

func cgoPathRewrites(pkg *packages.Package) (map[string]string, error) {
	sourcesByBase := make(map[string]map[string]struct{})
	for _, sourceFile := range pkg.GoFiles {
		sourceBase := filepath.Base(sourceFile)
		if sourcesByBase[sourceBase] == nil {
			sourcesByBase[sourceBase] = make(map[string]struct{})
		}
		sourcesByBase[sourceBase][sourceFile] = struct{}{}
	}

	rewrites := make(map[string]string)
	for _, generatedFile := range pkg.CompiledGoFiles {
		generatedBase := filepath.Base(generatedFile)
		if !strings.HasSuffix(generatedBase, cgoGenFileNameExt) {
			continue
		}

		sourceBase := strings.TrimSuffix(generatedBase, cgoGenFileNameExt) + ".go"
		matches := sourcesByBase[sourceBase]
		if len(matches) != 1 {
			return nil, fmt.Errorf("package %q: expected exactly one GoFiles source named %q for %q, found %d", pkg.ID, sourceBase, generatedFile, len(matches))
		}
		sourceFile := slices.Collect(maps.Keys(matches))[0]
		if !filepath.IsAbs(sourceFile) {
			return nil, fmt.Errorf("package %q: GoFiles source %q for %q is not absolute", pkg.ID, sourceFile, generatedFile)
		}
		rewrites[generatedFile] = sourceFile
	}
	return rewrites, nil
}

func buildBXLArgs(req *packages.DriverRequest, patterns []string, files []string) []string {
	bxlArgs := []string{}
	if len(patterns) > 0 {
		bxlArgs = append(bxlArgs, "--target_exprs")
		bxlArgs = append(bxlArgs, patterns...)
	}
	if len(files) > 0 {
		bxlArgs = append(bxlArgs, "--files")
		bxlArgs = append(bxlArgs, files...)
	}
	if req.Tests {
		bxlArgs = append(bxlArgs, "--need_tests", "true")
	}
	if req.Mode&packages.NeedName != 0 {
		bxlArgs = append(bxlArgs, "--need_name", "true")
	}
	// we have to implicitly add some flags if syntax/types requested
	// same as `go list` driver does
	// todo: add packages.NeedTypesSizes to the query as well, when we fix slow builds
	// as gopls requires it, but it can't wait very long
	needTypes := packages.NeedTypes | packages.NeedTypesInfo
	needCompiledGoFiles := packages.NeedCompiledGoFiles | packages.NeedSyntax | needTypes
	if req.Mode&(packages.NeedFiles|needCompiledGoFiles) != 0 {
		bxlArgs = append(bxlArgs, "--need_files", "true")
	}
	if req.Mode&needCompiledGoFiles != 0 {
		bxlArgs = append(bxlArgs, "--need_compiled_go_files", "true")
	}
	if req.Mode&(packages.NeedImports|needTypes) != 0 {
		bxlArgs = append(bxlArgs, "--need_imports", "true")
	}
	if req.Mode&(packages.NeedDeps|needTypes) != 0 {
		bxlArgs = append(bxlArgs, "--need_deps", "true")
	}
	if req.Mode&(packages.NeedExportFile|needTypes) != 0 {
		bxlArgs = append(bxlArgs, "--need_export_file", "true")
	}
	if req.Mode&LoadModeTypecheckCgo != 0 {
		bxlArgs = append(bxlArgs, "--typecheck_cgo", "true")
	}
	return bxlArgs
}
