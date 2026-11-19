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
	_ "embed"
	"encoding/json"
	"fmt"
)

//go:embed config.json
var configData []byte

type Config struct {
	Message string `json:"message"`
}

func main() {
	var cfg Config
	if err := json.Unmarshal(configData, &cfg); err != nil {
		panic(err)
	}
	fmt.Println(cfg.Message)
}
