# Buck2 Shims for Meta

These files implement shims for Meta internal buck2 cells, macros, and targets.

Via these shims, the buck2 experience when building Meta open source projects
should be nearly identical to the internal buck2 experience.

The shims need Buck2 and its prelude from 2026-09-01 or later, which support
`config_setting(root_values = ...)`. The `buck2` that Meta's open source
projects ship meets this.

## These shims are not recommended for non-Meta projects!!!

Prefer to use [rules from the buck2 prelude](https://buck2.build/docs/prelude/globals/)
and the [buck2 build apis](https://buck2.build/docs/api/build/globals/)

## License

This source code is licensed under both the MIT license found in the
LICENSE-MIT file in the root directory of this source tree and the Apache
License, Version 2.0 found in the LICENSE-APACHE file in the root directory
of this source tree.
