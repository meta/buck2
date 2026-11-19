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
	"bufio"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

func loadArgs(args []string) []string {
	newArgs := make([]string, 0, 0)
	for _, arg := range args {
		if !strings.HasPrefix(arg, "@") {
			newArgs = append(newArgs, arg)
		} else {
			file, _ := os.Open(arg[1:])
			defer file.Close()
			scanner := bufio.NewScanner(file)
			for scanner.Scan() {
				newArgs = append(newArgs, scanner.Text())
			}
		}
	}
	return newArgs
}

func jsonStreamToArray(r io.Reader, w io.Writer) error {
	var objs []any
	for dec := json.NewDecoder(r); dec.More(); {
		var obj any
		if err := dec.Decode(&obj); err != nil {
			return fmt.Errorf("failed to decode json: %w", err)
		}
		objs = append(objs, obj)
	}
	if err := json.NewEncoder(w).Encode(objs); err != nil {
		return fmt.Errorf("failed to encode json: %w", err)
	}
	return nil
}

func main() {
	os.Args = loadArgs(os.Args)
	var wrappedBinary = flag.String("go", "", "wrapped go binary")
	var goRoot = flag.String("goroot", "", "go root")
	var defaultGoOS = flag.String("default-goos", "", "default GOOS (if not set by env)")
	var defaultGoArch = flag.String("default-goarch", "", "default GOARCH (if not set by env)")
	var outputFile = flag.String("output", "", "file to redirect stdout to")
	var convertJsonStream = flag.Bool("convert-json-stream", false, "convert json stream to array")
	var gnuBuildID = flag.Bool("gnu-build-id", false, "rewrite .note.gnu.build-id with sha256 of the linked output (internal Go links only)")
	flag.Parse()
	unknownArgs := flag.Args()

	if *wrappedBinary == "" {
		log.Fatal("No wrapped binary specified")
	}

	absWrappedBinary, err := filepath.Abs(*wrappedBinary)
	if err != nil {
		log.Fatal("Failed to resolve wrapped binary: %s", err)
	}

	envs := make(map[string]string)
	for _, e := range os.Environ() {
		pair := strings.SplitN(e, "=", 2)
		envs[pair[0]] = pair[1]
	}

	goroot := *goRoot
	if goroot == "" {
		goroot = envs["GOROOT"]
	}

	if os.Getenv("GOOS") == "" && *defaultGoOS != "" {
		envs["GOOS"] = *defaultGoOS
	}
	if os.Getenv("GOARCH") == "" && *defaultGoArch != "" {
		envs["GOARCH"] = *defaultGoArch
	}

	if goroot != "" {
		absGoroot, err := filepath.Abs(goroot)
		if err != nil {
			log.Fatal("Failed to resolve GOROOT: %s", err)
		}
		envs["GOROOT"] = absGoroot
	}

	if buckScratchPath, ok := envs["BUCK_SCRATCH_PATH"]; ok {
		absBuckScratchPath, err := filepath.Abs(buckScratchPath)
		if err != nil {
			log.Fatal("Failed to resolve BUCK_SCRATCH_PATH: %s", err)
		}
		envs["GOCACHE"] = absBuckScratchPath
		envs["TMPDIR"] = absBuckScratchPath
	}

	cwd, err := os.Getwd()
	if err != nil {
		log.Fatal("Failed to get current working directory: %s", err)
	}

	for i, arg := range unknownArgs {
		unknownArgs[i] = strings.ReplaceAll(arg, "%cwd%", cwd)
	}

	var output *os.File
	if *outputFile == "" {
		output = os.Stdout
	} else {
		output, err = os.Create(*outputFile)
		if err != nil {
			log.Fatalf("Error creating output file: %s", err)
			os.Exit(1)
		}
		defer output.Close()
	}

	cmd := exec.Command(absWrappedBinary, unknownArgs...)

	cmd.Env = make([]string, 0, len(envs)/2)
	for k, v := range envs {
		cmd.Env = append(cmd.Env, k+"="+v)
	}

	stdout, err := cmd.StdoutPipe()
	if err != nil {
		log.Fatalf("Error creating stdout pipe: %s", err)
	}
	defer stdout.Close()

	cmd.Stderr = os.Stderr

	if err := cmd.Start(); err != nil {
		log.Fatalf("Error starting command: %s", err)
	}

	if *convertJsonStream {
		if err := jsonStreamToArray(stdout, output); err != nil {
			log.Fatalf("Error converting json stream: %s", err)
		}
	} else {
		if _, err := io.Copy(output, stdout); err != nil {
			log.Fatalf("Error copying stdout: %s", err)
		}
	}

	err = cmd.Wait()
	if err != nil {
		exitCode := 1
		if exitErr, ok := err.(*exec.ExitError); ok {
			exitCode = exitErr.ExitCode()
		}
		fmt.Fprintln(os.Stderr, "Error running command:", err)
		os.Exit(exitCode)
	}

	if *gnuBuildID {
		out := outputPath(unknownArgs)
		if out == "" {
			log.Fatalf("--gnu-build-id set but no -o output found in linker args: %v", unknownArgs)
		}
		if err := setGNUBuildID(out); err != nil {
			log.Fatalf("Error setting GNU build id: %s", err)
		}
	}
}
