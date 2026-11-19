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
	"bytes"
	"context"
	"fmt"
	"go/parser"
	"go/token"
	"log/slog"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strconv"
	"strings"

	"golang.org/x/tools/go/packages"
)

// queryStd resolves packages the "normal" way
// This is mostly used to resolve builtins and stdlib which buck doesn't know about
// - stdlibDeps is a special argument, we never add it to roots, as it's always a dependency of buck's package
func queryStd(ctx context.Context, req *packages.DriverRequest, goBinPath string, queryArgs []string) ([]*packages.Package, error) {
	// If we exist within a buck environment, then use the buck go tool instead
	// of the one in the system PATH
	origPath := os.Getenv("PATH")
	goBinDir := filepath.Dir(goBinPath)
	os.Setenv("PATH", fmt.Sprintf("%s:%s", goBinDir, origPath))
	slog.Info("overriding PATH to use Go from Buck", "dir", goBinDir)
	defer os.Setenv("PATH", origPath)

	mode := req.Mode

	// HACK: patch request to fix Syntax and Types for stdlib
	needTypes := packages.NeedTypes | packages.NeedTypesInfo
	if req.Mode&(packages.NeedSyntax|needTypes) != 0 {
		mode |= packages.NeedFiles | packages.NeedCompiledGoFiles
	}
	if req.Mode&(needTypes) != 0 {
		mode |= packages.NeedExportFile | packages.NeedDeps
	}

	// HACK: if go distribution has been built with CGO_ENABLED=0, this changes default behaviour on "always disabled"
	// Unfortunately we're relying on the original behaviour "lookup GCC" until we enable buck2 to build stdlib atonomously
	// Let's emulate this behaviour it for now
	cgoEnabled := "CGO_ENABLED=1"
	if _, err := exec.LookPath("gcc"); err != nil {
		cgoEnabled = "CGO_ENABLED=0"
	}

	loadcfg := &packages.Config{
		Context:    ctx,
		BuildFlags: req.BuildFlags,
		Overlay:    req.Overlay,
		Tests:      req.Tests,
		Mode:       mode,
		Logf: func(format string, args ...any) {
			slog.Info(fmt.Sprintf(format, args...))
		},
		Env: append(os.Environ(), cgoEnabled),
	}

	slog.Info("running stdlib load", "mode", loadcfg.Mode, "tests", loadcfg.Tests, "buildFlags", loadcfg.BuildFlags)
	loaded, err := packages.Load(loadcfg, queryArgs...)
	if err != nil {
		return nil, err
	}

	return loaded, nil
}

func parseImports(path string, content []byte) []string {
	ast, err := parser.ParseFile(token.NewFileSet(), path, bytes.NewReader(content), parser.ImportsOnly)
	if err != nil {
		return []string{}
	}

	imports := make([]string, 0, len(ast.Imports))
	for _, imp := range ast.Imports {
		impPath, err := strconv.Unquote(imp.Path.Value)
		if err != nil {
			continue
		}
		imports = append(imports, impPath)
	}

	return imports
}

// flattenDeps traverses response of packages.Load
// to transform it into a format of gopackages driver response
// Borrowed from: https://github.com/golang/tools/blob/15c16a55ca665ba6c72a3159c7dca3b0455e37d2/go/packages/gopackages/main.go#L129
func flattenDeps(pkgs []*packages.Package) []*packages.Package {
	var all []*packages.Package // postorder
	seen := make(map[*packages.Package]bool)
	var visit func(*packages.Package)
	visit = func(lpkg *packages.Package) {
		if !seen[lpkg] {
			seen[lpkg] = true

			// visit imports
			var importPaths []string
			for path := range lpkg.Imports {
				importPaths = append(importPaths, path)
			}
			sort.Strings(importPaths) // for determinism
			for _, path := range importPaths {
				visit(lpkg.Imports[path])
			}

			all = append(all, lpkg)
		}
	}
	for _, lpkg := range pkgs {
		visit(lpkg)
	}
	return all
}

// getImportedStdPackages analyzes the response and request to find imported standard library packages
func getImportedStdPackages(ctx context.Context, resp *packages.DriverResponse, req *packages.DriverRequest, goBinary string) ([]*packages.Package, error) {
	if len(resp.Roots) == 0 {
		return nil, nil
	}

	stdPackages, err := queryStd(ctx, req, goBinary, []string{metaStdLib})
	if err != nil {
		return nil, err
	}

	stdPackageByImportName := make(map[string]*packages.Package)
	for _, pkg := range stdPackages {
		// skip test packages, we don't import them
		if strings.HasSuffix(pkg.ID, ".test") || strings.HasSuffix(pkg.ID, ".test]") {
			continue
		}

		stdPackageByImportName[pkg.PkgPath] = pkg
	}

	importedPkgs := make(map[string]bool)
	for _, pkg := range resp.Packages {
		for _, imp := range pkg.Imports {
			importedPkgs[imp.ID] = true
		}

		addOverlayImports(pkg, req.Overlay, stdPackageByImportName, importedPkgs)
	}

	directDeps := []*packages.Package{}
	for imported := range importedPkgs {
		if pkg, ok := stdPackageByImportName[imported]; ok {
			directDeps = append(directDeps, pkg)
		}
	}

	return flattenDeps(directDeps), nil
}

// addOverlayImports records the std packages that unsaved edits of pkg's files import, so a
// package the user is editing resolves imports that are not in its saved sources yet.
func addOverlayImports(pkg *packages.Package, overlay map[string][]byte, stdPackageByImportName map[string]*packages.Package, importedPkgs map[string]bool) {
	for _, file := range pkg.GoFiles {
		content, ok := overlay[file]
		if !ok {
			continue
		}
		// The BXL response leaves Imports nil for a package without imports.
		if pkg.Imports == nil {
			pkg.Imports = map[string]*packages.Package{}
		}
		for _, impPath := range parseImports(file, content) {
			if stdPkg, ok := stdPackageByImportName[impPath]; ok {
				pkg.Imports[impPath] = stdPkg
			}
			importedPkgs[impPath] = true
		}
	}
}

// buildStdQuery constructs query arguments for standard library targets
func buildStdQuery(stdFiles, stdPatterns []string) []string {
	query := make([]string, 0, len(stdFiles)+len(stdPatterns))
	for _, pattern := range stdPatterns {
		query = append(query, "pattern="+pattern)
	}
	for _, file := range stdFiles {
		query = append(query, "file="+file)
	}
	return query
}

// extractRoots extracts package IDs as roots from a list of packages
func extractRoots(packages []*packages.Package) []string {
	roots := make([]string, 0, len(packages))
	for _, pkg := range packages {
		roots = append(roots, pkg.ID)
	}
	return roots
}
