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
	"flag"
	"log"
	"os"
	"os/exec"
	"strings"
)

func main() {
	output := flag.String("o", "", "Output file")
	flag.Parse()

	if *output == "" {
		flag.Usage()
		os.Exit(1)
	}

	cmd := exec.Command("go", "env", "GOROOT")
	out, err := cmd.Output()
	if err != nil {
		log.Fatalf("failed to execute `go env GOROOT`: %v", err)
	}
	goroot := strings.TrimSpace(string(out))
	if goroot == "" {
		log.Fatal("GOROOT is empty")
	}

	if err := os.CopyFS(*output, os.DirFS(goroot)); err != nil {
		log.Fatalf("copy directory %q -> %q: %v", goroot, *output, err)
	}
}
