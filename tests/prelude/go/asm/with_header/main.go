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

import "fmt"

type goAsmCheck struct {
	x int64
	y int64
}

func sizeOfFoo() int64

func main() {
	fmt.Println(sizeOfFoo())
}
