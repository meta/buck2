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
	"encoding/json"
	"flag"
	"fmt"
	"io/fs"
	"os"
	"path"
	"path/filepath"
	"sort"
	"strings"
)

// EmbedCfg is the structure expected by the Go compiler's -embedcfg flag
type EmbedCfg struct {
	Patterns map[string][]string `json:"Patterns"`
	Files    map[string]string   `json:"Files"`
}

func main() {
	pkgdir := flag.String("pkgdir", "", "Package directory containing files to embed (required)")
	output := flag.String("o", "", "Output file for embedcfg JSON (required)")
	flag.Parse()

	if *pkgdir == "" {
		fatal("-pkgdir is required")
	}
	if *output == "" {
		fatal("-o is required")
	}

	patterns := flag.Args()

	files, pmap, err := resolveEmbed(*pkgdir, patterns)
	if err != nil {
		fatal("%v", err)
	}

	cfg := EmbedCfg{
		Patterns: pmap,
		Files:    make(map[string]string),
	}

	for _, file := range files {
		cfg.Files[file] = filepath.Join(*pkgdir, file)
	}

	data, err := json.MarshalIndent(&cfg, "", "\t")
	if err != nil {
		fatal("failed to marshal embedcfg: %v", err)
	}

	if err := os.WriteFile(*output, data, 0644); err != nil {
		fatal("failed to write output file: %v", err)
	}
}

func fatal(format string, args ...interface{}) {
	fmt.Fprintf(os.Stderr, format+"\n", args...)
	os.Exit(1)
}

// resolveEmbed resolves embed patterns and returns:
// - files: deduplicated list of matched files (relative to pkgdir)
// - pmap: mapping from original pattern to matched files
func resolveEmbed(pkgdir string, patterns []string) (files []string, pmap map[string][]string, err error) {
	pmap = make(map[string][]string)
	have := make(map[string]bool)
	var list []string

	for _, pattern := range patterns {
		origPattern := pattern

		// Handle all: prefix which includes hidden files
		all := strings.HasPrefix(pattern, "all:")
		if all {
			pattern = pattern[len("all:"):]
		}

		// Validate the pattern
		if !validEmbedPattern(pattern) {
			return nil, nil, fmt.Errorf("invalid embed pattern %q", origPattern)
		}

		// Match files for this pattern
		match, err := matchPattern(pkgdir, pattern, all)
		if err != nil {
			return nil, nil, fmt.Errorf("pattern %q: %v", origPattern, err)
		}

		if len(match) == 0 {
			return nil, nil, fmt.Errorf("pattern %s: no matching files found", origPattern)
		}

		// Sort matches for deterministic output
		sort.Strings(match)
		pmap[origPattern] = match

		// Add to deduplicated list
		for _, m := range match {
			if !have[m] {
				have[m] = true
				list = append(list, m)
			}
		}
	}

	sort.Strings(list)
	return list, pmap, nil
}

// validEmbedPattern checks if the pattern is valid for embedding
func validEmbedPattern(pattern string) bool {
	if !fs.ValidPath(pattern) {
		return false
	}
	// "." is not allowed as a pattern
	if pattern == "." {
		return false
	}
	return true
}

// isBadEmbedName checks if the name is invalid for embedding (VCS dirs, empty)
func isBadEmbedName(name string) bool {
	if name == "" {
		return true
	}
	switch name {
	case ".git", ".svn", ".hg", ".bzr":
		return true
	}
	return false
}

// isHidden returns true if the name starts with . or _
func isHidden(name string) bool {
	return strings.HasPrefix(name, ".") || strings.HasPrefix(name, "_")
}

// matchPattern matches files against a pattern
func matchPattern(pkgdir, pattern string, all bool) ([]string, error) {
	// Quote special glob characters in pkgdir
	quotedPkgdir := quoteGlobMeta(pkgdir)

	// Build the full glob pattern
	// Convert pattern separators from / to OS-specific
	osPattern := filepath.FromSlash(pattern)
	fullPattern := filepath.Join(quotedPkgdir, osPattern)

	// Get initial matches using filepath.Glob
	matches, err := filepath.Glob(fullPattern)
	if err != nil {
		return nil, err
	}

	var result []string

	for _, match := range matches {
		// Get the relative path from pkgdir
		rel, err := filepath.Rel(pkgdir, match)
		if err != nil {
			continue
		}

		info, err := os.Lstat(match)
		if err != nil {
			continue
		}

		// A directly named VCS directory or file is an error, as in `go build`.
		for dir := rel; dir != "."; dir = filepath.Dir(dir) {
			if elem := filepath.Base(dir); isBadEmbedName(elem) {
				what := "file"
				if info.IsDir() {
					what = "directory"
				}
				return nil, fmt.Errorf("cannot embed %s %s: invalid name %s", what, filepath.ToSlash(rel), elem)
			}
		}

		// Convert to forward slashes for consistency
		rel = filepath.ToSlash(rel)

		if info.IsDir() {
			// For directories, walk and collect all files
			dirFiles, err := walkDir(pkgdir, rel, all)
			if err != nil {
				return nil, err
			}
			result = append(result, dirFiles...)
		} else {
			result = append(result, rel)
		}
	}

	return result, nil
}

// walkDir recursively walks a directory and collects files for embedding
func walkDir(pkgdir, dir string, all bool) ([]string, error) {
	var result []string
	fullDir := filepath.Join(pkgdir, filepath.FromSlash(dir))

	// A directory with its own go.mod is another module, which `go build` never embeds.
	if _, err := os.Stat(filepath.Join(fullDir, "go.mod")); err == nil {
		return nil, nil
	}

	entries, err := os.ReadDir(fullDir)
	if err != nil {
		return nil, err
	}

	for _, entry := range entries {
		name := entry.Name()

		// Skip bad embed names (VCS directories)
		if isBadEmbedName(name) {
			continue
		}

		// Skip hidden files/dirs unless all: prefix was used
		if !all && isHidden(name) {
			continue
		}

		relPath := path.Join(dir, name)

		if entry.IsDir() {
			// Recursively walk subdirectory
			subFiles, err := walkDir(pkgdir, relPath, all)
			if err != nil {
				return nil, err
			}
			result = append(result, subFiles...)
		} else {
			result = append(result, relPath)
		}
	}

	return result, nil
}

// quoteGlobMeta quotes glob metacharacters in a path
func quoteGlobMeta(s string) string {
	special := `*?[\`
	var b strings.Builder
	for _, c := range s {
		if strings.ContainsRune(special, c) {
			b.WriteRune('\\')
		}
		b.WriteRune(c)
	}
	return b.String()
}
