/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

// Lists Go test, benchmark, fuzz target and example names from `*_test.go`
// source files without compiling or running the test binary. Output is one
// name per line on stdout. Mirrors `go test -list <regex>` semantics:
// detection rules and ordering match `cmd/go/internal/load.isTest` and the
// `testing.MainStart` slice population done by testmaingen.go in this same
// directory.
//
// Used by TPX for static test listing. For the same source set and the same
// `-match` regex, the output is identical to what
// `<test_binary> -test.list=<regex>` would print.

package main

import (
	"bufio"
	"flag"
	"fmt"
	"go/ast"
	"go/doc"
	"go/parser"
	"go/token"
	"log"
	"os"
	"regexp"
	"strings"
	"unicode"
	"unicode/utf8"
)

var matchRegex = flag.String("match", "",
	"Regex to filter names. Empty matches all. Mirrors `go test -list`.")

// Resolve argsfiles in args (e.g. `@file.txt`). Matches testmaingen's helper.
func loadArgs(args []string) []string {
	newArgs := make([]string, 0, len(args))
	for _, arg := range args {
		if !strings.HasPrefix(arg, "@") {
			newArgs = append(newArgs, arg)
			continue
		}
		file, err := os.Open(arg[1:])
		if err != nil {
			log.Fatalln("Could not open argsfile:", err)
		}
		scanner := bufio.NewScanner(file)
		for scanner.Scan() {
			newArgs = append(newArgs, scanner.Text())
		}
		if err := scanner.Err(); err != nil {
			log.Fatalln("Could not read argsfile:", err)
		}
		file.Close()
	}
	return newArgs
}

// isTest reports whether name has the given test-kind prefix (Test, Benchmark,
// Fuzz) and isn't followed by a lowercase letter. Matches Go's
// `cmd/go/internal/load.isTest`: "Testing" is excluded, "Test" / "TestX" /
// "Test_X" / "Test123" are accepted.
func isTest(name, prefix string) bool {
	if !strings.HasPrefix(name, prefix) {
		return false
	}
	if len(name) == len(prefix) {
		return true
	}
	r, _ := utf8.DecodeRuneInString(name[len(prefix):])
	return !unicode.IsLower(r)
}

// isTestFunc reports whether fn has the signature `func F(_ *<pkg>.<arg>)`
// where <arg> is T, B or F. Matches testmaingen's `isTestFunc`.
func isTestFunc(fn *ast.FuncDecl, arg string) bool {
	if fn.Recv != nil {
		return false
	}
	if fn.Type.Results != nil && len(fn.Type.Results.List) > 0 {
		return false
	}
	if fn.Type.Params == nil || len(fn.Type.Params.List) != 1 {
		return false
	}
	if len(fn.Type.Params.List[0].Names) > 1 {
		return false
	}
	ptr, ok := fn.Type.Params.List[0].Type.(*ast.StarExpr)
	if !ok {
		return false
	}
	if name, ok := ptr.X.(*ast.Ident); ok && name.Name == arg {
		return true
	}
	if sel, ok := ptr.X.(*ast.SelectorExpr); ok && sel.Sel.Name == arg {
		return true
	}
	return false
}

// collectNames walks one parsed file and returns test/benchmark/fuzz/example
// names in the order `testing.MainStart` registers them: tests first (source
// order), then benchmarks, then fuzz targets, then examples.
func collectNames(f *ast.File) []string {
	var tests, benches, fuzzes, examples []string
	for _, d := range f.Decls {
		fn, ok := d.(*ast.FuncDecl)
		if !ok {
			continue
		}
		if fn.Recv != nil {
			continue
		}
		name := fn.Name.Name
		switch {
		case name == "TestMain":
			// TestMain(t *testing.T) is a regular test; TestMain(m *testing.M)
			// is the binary entrypoint and is not listed. Mirrors testmaingen.
			if isTestFunc(fn, "T") {
				tests = append(tests, name)
			}
		case isTest(name, "Test"):
			if isTestFunc(fn, "T") {
				tests = append(tests, name)
			}
		case isTest(name, "Benchmark"):
			if isTestFunc(fn, "B") {
				benches = append(benches, name)
			}
		case isTest(name, "Fuzz"):
			if isTestFunc(fn, "F") {
				fuzzes = append(fuzzes, name)
			}
		}
	}
	// Examples via go/doc — same filter testmaingen applies: skip examples
	// without expected output unless they were declared with empty output.
	for _, e := range doc.Examples(f) {
		if e.Output == "" && !e.EmptyOutput {
			continue
		}
		examples = append(examples, "Example"+e.Name)
	}
	out := make([]string, 0, len(tests)+len(benches)+len(fuzzes)+len(examples))
	out = append(out, tests...)
	out = append(out, benches...)
	out = append(out, fuzzes...)
	out = append(out, examples...)
	return out
}

func main() {
	os.Args = loadArgs(os.Args)
	flag.Parse()

	var matcher *regexp.Regexp
	if *matchRegex != "" {
		var err error
		matcher, err = regexp.Compile(*matchRegex)
		if err != nil {
			log.Fatalln("Invalid -match regex:", err)
		}
	}

	fset := token.NewFileSet()
	bw := bufio.NewWriter(os.Stdout)

	for _, filename := range flag.Args() {
		f, err := parser.ParseFile(fset, filename, nil, parser.ParseComments|parser.SkipObjectResolution)
		if err != nil {
			log.Fatalln("Could not parse:", err)
		}
		names := collectNames(f)
		for _, n := range names {
			if matcher != nil && !matcher.MatchString(n) {
				continue
			}
			if _, err := fmt.Fprintln(bw, n); err != nil {
				log.Fatalln("Could not write name:", err)
			}
		}
	}

	if err := bw.Flush(); err != nil {
		log.Fatalln("Could not flush stdout:", err)
	}
}
