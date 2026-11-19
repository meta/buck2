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

/*
#include <stdio.h>

void print_value(int i) {
    printf("i = %d\n", i);
    fflush(stdout);
}
*/
import "C"

func main() {
	C.print_value(1)
}
