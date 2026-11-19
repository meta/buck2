# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# For a step's script to source. What the script has to say about the run goes
# where it can be read without a GitHub login: on the run's summary page and in
# annotations. Reading a job's log takes a login.

# shellcheck shell=bash

# Puts the text $2 on the run's summary page under the heading $1.
summarize() {
  {
    echo "### $1"
    echo '```'
    echo "$2"
    echo '```'
  } >> "$GITHUB_STEP_SUMMARY"
}

# How many annotations of each level this step has made: GitHub keeps ten of
# a level for a step.
levels=(error warning notice)
made=(0 0 0)

# Puts the text $3 in annotations of the level $1, titled $2. GitHub keeps
# about 4000 characters of an annotation, so the text goes out by whole lines
# over as many annotations as it needs, and over the levels below $1 once that
# one is used up.
annotate() {
  local chunks=() chunk="" line
  while IFS= read -r line; do
    line=${line:0:1000}
    if [[ -n $chunk && $((${#chunk} + ${#line})) -ge 3500 ]]; then
      chunks+=("$chunk")
      chunk=""
    fi
    chunk+="$line"$'\n'
  done <<< "$3"
  chunks+=("$chunk")
  local level=0
  [[ $1 == error ]] || level=1
  local i title message
  for i in "${!chunks[@]}"; do
    while [[ $level -lt 3 && ${made[$level]} -ge 10 ]]; do
      level=$((level + 1))
    done
    [[ $level -lt 3 ]] || break
    made[level]=$((made[level] + 1))
    title="$2 ($((i + 1)) of ${#chunks[@]})"
    # An annotation is one line: its newlines are escaped, and so are the
    # colons and commas of its title.
    title=${title//'%'/'%25'}
    title=${title//':'/'%3A'}
    title=${title//','/'%2C'}
    message=${chunks[$i]%$'\n'}
    message=${message//'%'/'%25'}
    message=${message//$'\r'/'%0D'}
    message=${message//$'\n'/'%0A'}
    echo "::${levels[$level]} title=$title::$message"
  done
}

# Puts the text $3 on the run's summary page and in annotations of the level
# $1, both under the heading $2.
publish() {
  summarize "$2" "$3"
  annotate "$1" "$2" "$3"
}
