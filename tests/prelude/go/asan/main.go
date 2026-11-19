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
#include <stdlib.h>
#include <stdio.h>

int *p;

int* test() {
 p = (int *)malloc(10 * sizeof(int));
 free(p);
 return p;
}
*/
import "C"

func main() {
	a := C.test()
	*a = 3 // use-after-free // BOOM
	print(*a)
}
