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
	"strings"
	"testing"

	"golang.org/x/tools/go/packages"
)

// A package without imports, as the BXL response describes it, whose only file
// gained `import "fmt"` in the editor.
func overlayImportFixture() (*packages.Package, map[string][]byte, map[string]*packages.Package, map[string]bool) {
	pkg := &packages.Package{ID: "root//a:a", PkgPath: "a", GoFiles: []string{"/repo/a/a.go"}}
	overlay := map[string][]byte{"/repo/a/a.go": []byte("package a\n\nimport \"fmt\"\n\nvar _ = fmt.Sprint\n")}
	std := map[string]*packages.Package{"fmt": {ID: "fmt", PkgPath: "fmt"}}
	return pkg, overlay, std, map[string]bool{}
}

// An import added in the editor to a package without imports is recorded.
func TestOverlayImportIntoPackageWithoutImportsIsRecorded(t *testing.T) {
	pkg, overlay, std, imported := overlayImportFixture()
	addOverlayImports(pkg, overlay, std, imported)
	if pkg.Imports["fmt"] != std["fmt"] {
		t.Fatalf("fmt missing from Imports: %v", pkg.Imports)
	}
	if !imported["fmt"] {
		t.Fatalf("fmt not recorded as imported: %v", imported)
	}
	if !strings.HasPrefix(pkg.Imports["fmt"].ID, "fmt") {
		t.Fatalf("unexpected import record %v", pkg.Imports["fmt"])
	}
}
