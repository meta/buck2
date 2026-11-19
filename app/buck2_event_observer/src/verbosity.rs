/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_hash::BuckMutSet;
use dupe::Dupe;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum VerbosityError {
    #[error("Can't have more than 1 level set at a time")]
    MoreThan1Level,
    #[error("Item name not recognized: {0}")]
    UnknownItem(String),
}

const VERBOSITY_ITEM_VARIANTS: usize = 7;

/// The logging verbosity to use in our various consoles.
///
/// Accessor methods should be used to determine
/// specific requested functionality.
#[derive(Debug, Copy, Clone)]
pub struct Verbosity {
    // Use array (vs Vec or HashSet) to keep it Copy
    items: [Option<VerbosityItem>; VERBOSITY_ITEM_VARIANTS],
}

/// Each level is a superset of the previous ones.
#[derive(Debug, Copy, Clone, Dupe, PartialEq, Eq)]
enum VerbosityLevel {
    /// Print as few messages as possible.
    Quiet,
    /// The default verbosity. Error messages are printed, but extra details may not be.
    Default,
    /// Print more things, but still a manageable and potentially useful amount of output.
    Verbose,
    /// Print even more things, this is likely not very useful.
    AllCommands,
    /// Print too much output, this is likely not useful.
    AllStderr,
}

#[derive(Debug, Copy, Clone, Dupe, PartialEq, Eq, Hash)]
pub enum VerbosityItem {
    /// Print stderr for successful commands
    Stderr,
    /// Print full command for failed actions
    FullFailedCommand,
    /// Print all commands that are being executed
    Commands,
    /// Print all actions (in consoles where that is relevant)
    Actions,
    /// Print periodic status messages
    Status,
    /// Print I/O stats and so on when outputting status of what we're waiting
    /// on. Note that we'll still print them if we hit a condition where we think they are
    /// relevant, such as zero open spans
    Stats,
    /// Some commands print a success message to stderr when they succeed
    Success,
    // ** update VERBOSITY_ITEM_VARIANTS const if more items are added **
}

impl VerbosityLevel {
    fn items(self) -> BuckMutSet<VerbosityItem> {
        let items = match self {
            Self::Quiet => vec![],
            Self::Default => vec![VerbosityItem::Status, VerbosityItem::Success],
            Self::Verbose => vec![
                VerbosityItem::FullFailedCommand,
                VerbosityItem::Actions,
                VerbosityItem::Stats,
            ],
            Self::AllCommands => vec![VerbosityItem::Commands],
            Self::AllStderr => vec![VerbosityItem::Stderr],
        };
        self.add_to_previous(items)
    }

    fn add_to_previous(self, items: Vec<VerbosityItem>) -> BuckMutSet<VerbosityItem> {
        let mut items: BuckMutSet<_> = items.into_iter().collect();
        if let Some(level) = self.previous() {
            items.extend(level.items());
        }
        items
    }

    fn previous(self) -> Option<Self> {
        match self {
            Self::Quiet => None,
            Self::Default => Some(Self::Quiet),
            Self::Verbose => Some(Self::Default),
            Self::AllCommands => Some(Self::Verbose),
            Self::AllStderr => Some(Self::AllCommands),
        }
    }

    fn from(value: i64) -> Self {
        match value {
            i if i <= 0 => Self::Quiet,
            1 => Self::Default,
            2 => Self::Verbose,
            3 => Self::AllCommands,
            _ => Self::AllStderr,
        }
    }
}

impl VerbosityItem {
    fn from(value: &str) -> buck2_error::Result<Self> {
        let item = match value {
            "stderr" => Self::Stderr,
            "full_failed_command" => Self::FullFailedCommand,
            "commands" => Self::Commands,
            "actions" => Self::Actions,
            "status" => Self::Status,
            "stats" => Self::Stats,
            "success" => Self::Success,
            _ => return Err(VerbosityError::UnknownItem(value.to_owned()).into()),
        };
        Ok(item)
    }
}

impl Verbosity {
    pub fn try_from_cli(value: &str) -> buck2_error::Result<Verbosity> {
        let split: Vec<&str> = value.split(',').collect();
        let mut levels: Vec<VerbosityLevel> = Vec::new();
        let mut items: BuckMutSet<VerbosityItem> = BuckMutSet::default();

        for &value in &split {
            if let Ok(value) = value.parse::<i64>() {
                levels.push(VerbosityLevel::from(value));
            } else {
                items.insert(VerbosityItem::from(value)?);
            }
        }

        if levels.len() > 1 {
            return Err(VerbosityError::MoreThan1Level.into());
        }

        if let Some(level) = levels.first() {
            items.extend(level.items());
        }

        Ok(Self::from_items(items))
    }

    fn from_items(items: BuckMutSet<VerbosityItem>) -> Self {
        let mut array = [None; VERBOSITY_ITEM_VARIANTS];
        let vec: Vec<_> = items.into_iter().map(Some).collect();
        for (i, opt_item) in vec.into_iter().enumerate() {
            array[i] = opt_item;
        }

        Self { items: array }
    }

    /// If a verbosity setting is triggered at a particular level, does this verbosity trigger it
    fn has(self, required: VerbosityItem) -> bool {
        self.items.contains(&Some(required))
    }

    /// Whether stderr should be printed to users for successful commands by default.
    pub fn print_success_stderr(self) -> bool {
        self.has(VerbosityItem::Stderr)
    }

    /// Whether the full command for failed actions should be printed. Otherwise, a truncated command is printed.
    pub fn print_failure_full_command(self) -> bool {
        self.has(VerbosityItem::FullFailedCommand)
    }

    /// Whether to print all commands that are being executed
    pub fn print_all_commands(self) -> bool {
        self.has(VerbosityItem::Commands)
    }

    /// Whether we should show all actions (in consoles where that is relevant).
    pub fn print_all_actions(self) -> bool {
        self.has(VerbosityItem::Actions)
    }

    /// Whether we should print periodic status messages
    pub fn print_status(self) -> bool {
        self.has(VerbosityItem::Status)
    }

    /// Whether we should print I/O stats and so on when outputting status of what we're waiting
    /// on. Note that we'll still print them if we hit a condition where we think they are
    /// relevant, such as zero open spans.
    pub fn always_print_stats_in_status(self) -> bool {
        self.has(VerbosityItem::Stats)
    }

    /// For commands that might do this, whether a message should be printed when the command succeeds.
    pub fn print_success_message(self) -> bool {
        self.has(VerbosityItem::Success)
    }
}

impl Default for Verbosity {
    fn default() -> Self {
        Self::from_items(VerbosityLevel::Default.items())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quiet_with_items() {
        let verbosity = Verbosity::try_from_cli("stats,stderr").unwrap();
        assert!(!verbosity.print_status());
        assert!(!verbosity.print_success_message());
        assert!(!verbosity.print_failure_full_command());
        assert!(verbosity.always_print_stats_in_status());
        assert!(!verbosity.print_all_actions());
        assert!(!verbosity.print_all_commands());
        assert!(verbosity.print_success_stderr());
    }

    #[test]
    fn test_default_with_stderr() {
        let verbosity = Verbosity::try_from_cli("1,stderr").unwrap();
        assert!(verbosity.print_status());
        assert!(verbosity.print_success_message());
        assert!(!verbosity.print_failure_full_command());
        assert!(!verbosity.always_print_stats_in_status());
        assert!(!verbosity.print_all_actions());
        assert!(!verbosity.print_all_commands());
        assert!(verbosity.print_success_stderr());
    }

    #[test]
    fn test_verbose() {
        let verbosity = Verbosity::try_from_cli("2").unwrap();
        assert!(verbosity.print_status());
        assert!(verbosity.print_success_message());
        assert!(verbosity.print_failure_full_command());
        assert!(verbosity.always_print_stats_in_status());
        assert!(verbosity.print_all_actions());
        assert!(!verbosity.print_all_commands());
        assert!(!verbosity.print_success_stderr());
    }

    #[test]
    fn test_more_than_one_level_throws_error() {
        let result = Verbosity::try_from_cli("0,1");
        assert!(result.is_err());
    }

    #[test]
    fn test_unknown_item_throws_error() {
        let result = Verbosity::try_from_cli("unknown");
        assert!(result.is_err());
    }

    #[test]
    fn test_default() {
        let verbosity = Verbosity::default();
        assert!(verbosity.print_status());
        assert!(verbosity.print_success_message());
        assert!(!verbosity.print_failure_full_command());
        assert!(!verbosity.always_print_stats_in_status());
        assert!(!verbosity.print_all_actions());
        assert!(!verbosity.print_all_commands());
        assert!(!verbosity.print_success_stderr());
    }
}
