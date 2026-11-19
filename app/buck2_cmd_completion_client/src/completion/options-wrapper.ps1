# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

using namespace System.Management.Automation
using namespace System.Management.Automation.Language

# %INSERT_GENERATED_LINE%

# clap's static completer, whose `Register-ArgumentCompleter` call is rewritten at
# generation time into an assignment to `$BuckClapStaticCompleter` (see the Rust
# generator). We register it for both `buck` and `buck2`.

# clap_complete generated content BEGINS
# %INSERT_OPTION_COMPLETION%
# clap_complete generated content ENDS

if (-not $BuckClapStaticCompleter) {
    throw "buck2 completion: clap static completer was not defined - the Rust-side rewrite may have failed"
}
Register-ArgumentCompleter -Native -CommandName 'buck', 'buck2' -ScriptBlock $BuckClapStaticCompleter
