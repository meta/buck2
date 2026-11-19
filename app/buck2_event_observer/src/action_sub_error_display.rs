/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_data::ActionSubError;

pub trait ActionSubErrorDisplay {
    fn display(&self) -> Option<String>;
    fn has_location_info(&self) -> bool;
}

impl ActionSubErrorDisplay for ActionSubError {
    fn display(&self) -> Option<String> {
        fn display_location(error: &ActionSubError) -> Option<String> {
            match &error.file {
                Some(file) => {
                    let mut res = String::new();
                    res.push_str(file);
                    if let Some(lnum) = error.lnum {
                        res.push_str(&format!(":{}", lnum));
                        if let Some(end_lnum) = error.end_lnum {
                            res.push_str(&format!("-{}", end_lnum));
                        }
                        if let Some(col) = error.col {
                            res.push_str(&format!(":{}", col));
                            if let Some(end_col) = error.end_col {
                                res.push_str(&format!("-{}", end_col));
                            }
                        }
                        Some(res)
                    } else {
                        None
                    }
                }
                None => None,
            }
        }

        let mut res = String::new();
        res.push_str(&format!("[{}]", self.category));
        if let Some(subcategory) = &self.subcategory {
            res.push_str(&format!("[{}]", subcategory));
        }
        if let Some(location_str) = display_location(self) {
            res.push_str(&format!(" {}", location_str));
        }

        match (&self.error_type, self.error_number) {
            (Some(error_type), Some(error_number)) => {
                res.push_str(&format!(" {}[{}]:", error_type, error_number))
            }
            (Some(error_type), None) => res.push_str(&format!(" {}:", error_type)),
            (None, Some(error_number)) => res.push_str(&format!(" [{}]:", error_number)),
            (None, None) => {}
        };

        if let Some(message) = &self.message {
            res.push_str(&format!(" {}", message));
        }

        if let Some(remediation) = &self.remediation {
            res.push_str(&format!("\n  Hint: {}", remediation));
        }

        Some(res)
    }

    fn has_location_info(&self) -> bool {
        self.file.is_some()
            || self.lnum.is_some()
            || self.col.is_some()
            || self.end_lnum.is_some()
            || self.end_col.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_sub_error_display_basic() {
        let error = ActionSubError {
            category: "rust_error".to_owned(),
            message: Some("undefined variable".to_owned()),
            file: None,
            lnum: None,
            end_lnum: None,
            col: None,
            end_col: None,
            error_type: None,
            error_number: None,
            show_in_stderr: false,
            subcategory: None,
            remediation: None,
        };

        let display = error.display();
        assert_eq!(display, Some("[rust_error] undefined variable".to_owned()));
    }

    #[test]
    fn test_action_sub_error_display_with_file_and_line() {
        let error = ActionSubError {
            category: "compile_error".to_owned(),
            message: Some("syntax error".to_owned()),
            file: Some("src/main.rs".to_owned()),
            lnum: Some(42),
            end_lnum: None,
            col: None,
            end_col: None,
            error_type: None,
            error_number: None,
            show_in_stderr: false,
            subcategory: None,
            remediation: None,
        };

        let display = error.display();
        assert_eq!(
            display,
            Some("[compile_error] src/main.rs:42 syntax error".to_owned())
        );
    }

    #[test]
    fn test_action_sub_error_display_with_line_range() {
        let error = ActionSubError {
            category: "lint_warning".to_owned(),
            message: Some("unused import".to_owned()),
            file: Some("lib.rs".to_owned()),
            lnum: Some(10),
            end_lnum: Some(15),
            col: None,
            end_col: None,
            error_type: None,
            error_number: None,
            show_in_stderr: false,
            subcategory: None,
            remediation: None,
        };

        let display = error.display();
        assert_eq!(
            display,
            Some("[lint_warning] lib.rs:10-15 unused import".to_owned())
        );
    }

    #[test]
    fn test_action_sub_error_display_with_column() {
        let error = ActionSubError {
            category: "type_error".to_owned(),
            message: Some("type mismatch".to_owned()),
            file: Some("test.rs".to_owned()),
            lnum: Some(5),
            end_lnum: None,
            col: Some(12),
            end_col: None,
            error_type: None,
            error_number: None,
            show_in_stderr: false,
            subcategory: None,
            remediation: None,
        };

        let display = error.display();
        assert_eq!(
            display,
            Some("[type_error] test.rs:5:12 type mismatch".to_owned())
        );
    }

    #[test]
    fn test_action_sub_error_display_with_column_range() {
        let error = ActionSubError {
            category: "parse_error".to_owned(),
            message: Some("unexpected token".to_owned()),
            file: Some("parser.rs".to_owned()),
            lnum: Some(20),
            end_lnum: None,
            col: Some(5),
            end_col: Some(10),
            error_type: None,
            error_number: None,
            show_in_stderr: false,
            subcategory: None,
            remediation: None,
        };

        let display = error.display();
        assert_eq!(
            display,
            Some("[parse_error] parser.rs:20:5-10 unexpected token".to_owned())
        );
    }

    #[test]
    fn test_action_sub_error_display_with_full_range() {
        let error = ActionSubError {
            category: "semantic_error".to_owned(),
            message: Some("invalid operation".to_owned()),
            file: Some("analyzer.rs".to_owned()),
            lnum: Some(100),
            end_lnum: Some(105),
            col: Some(15),
            end_col: Some(25),
            error_type: None,
            error_number: None,
            show_in_stderr: false,
            subcategory: None,
            remediation: None,
        };

        let display = error.display();
        assert_eq!(
            display,
            Some("[semantic_error] analyzer.rs:100-105:15-25 invalid operation".to_owned())
        );
    }

    #[test]
    fn test_action_sub_error_display_with_error_type() {
        let error = ActionSubError {
            category: "gcc_error".to_owned(),
            message: Some("undefined reference".to_owned()),
            file: Some("main.c".to_owned()),
            lnum: Some(50),
            end_lnum: None,
            col: None,
            end_col: None,
            error_type: Some("error".to_owned()),
            error_number: None,
            show_in_stderr: false,
            subcategory: None,
            remediation: None,
        };

        let display = error.display();
        assert_eq!(
            display,
            Some("[gcc_error] main.c:50 error: undefined reference".to_owned())
        );
    }

    #[test]
    fn test_action_sub_error_display_with_error_number() {
        let error = ActionSubError {
            category: "http_error".to_owned(),
            message: Some("Not Found".to_owned()),
            file: None,
            lnum: None,
            end_lnum: None,
            col: None,
            end_col: None,
            error_type: None,
            error_number: Some(404),
            show_in_stderr: false,
            subcategory: None,
            remediation: None,
        };

        let display = error.display();
        assert_eq!(display, Some("[http_error] [404]: Not Found".to_owned()));
    }

    #[test]
    fn test_action_sub_error_display_with_error_type_and_number() {
        let error = ActionSubError {
            category: "rustc".to_owned(),
            message: Some("cannot borrow as mutable".to_owned()),
            file: Some("borrow.rs".to_owned()),
            lnum: Some(15),
            end_lnum: None,
            col: Some(9),
            end_col: None,
            error_type: Some("error".to_owned()),
            error_number: Some(502),
            show_in_stderr: false,
            subcategory: None,
            remediation: None,
        };

        let display = error.display();
        assert_eq!(
            display,
            Some("[rustc] borrow.rs:15:9 error[502]: cannot borrow as mutable".to_owned())
        );
    }

    #[test]
    fn test_action_sub_error_display_minimal() {
        let error = ActionSubError {
            category: "unknown".to_owned(),
            message: None,
            file: None,
            lnum: None,
            end_lnum: None,
            col: None,
            end_col: None,
            error_type: None,
            error_number: None,
            show_in_stderr: false,
            subcategory: None,
            remediation: None,
        };

        let display = error.display();
        assert_eq!(display, Some("[unknown]".to_owned()));
    }

    #[test]
    fn test_action_sub_error_display_file_without_line() {
        // File without line number should not display location
        let error = ActionSubError {
            category: "file_error".to_owned(),
            message: Some("file not found".to_owned()),
            file: Some("missing.txt".to_owned()),
            lnum: None,
            end_lnum: None,
            col: None,
            end_col: None,
            error_type: None,
            error_number: None,
            show_in_stderr: false,
            subcategory: None,
            remediation: None,
        };

        let display = error.display();
        assert_eq!(display, Some("[file_error] file not found".to_owned()));
    }

    #[test]
    fn test_action_sub_error_display_all_fields() {
        let error = ActionSubError {
            category: "complex_error".to_owned(),
            message: Some("comprehensive error message".to_owned()),
            file: Some("complex.rs".to_owned()),
            lnum: Some(25),
            end_lnum: Some(30),
            col: Some(10),
            end_col: Some(20),
            error_type: Some("warning".to_owned()),
            error_number: Some(1001),
            show_in_stderr: false,
            subcategory: None,
            remediation: None,
        };

        let display = error.display();
        assert_eq!(
            display,
            Some(
                "[complex_error] complex.rs:25-30:10-20 warning[1001]: comprehensive error message"
                    .to_owned()
            )
        );
    }

    #[test]
    fn test_action_sub_error_display_with_remediation() {
        let error = ActionSubError {
            category: "rustc".to_owned(),
            message: Some("cannot borrow as mutable".to_owned()),
            file: Some("borrow.rs".to_owned()),
            lnum: Some(15),
            end_lnum: None,
            col: Some(9),
            end_col: None,
            error_type: Some("error".to_owned()),
            error_number: Some(502),
            show_in_stderr: false,
            subcategory: None,
            remediation: Some("Consider using `RefCell` for interior mutability".to_owned()),
        };

        let display = error.display();
        assert_eq!(
            display,
            Some(
                "[rustc] borrow.rs:15:9 error[502]: cannot borrow as mutable\n  Hint: Consider using `RefCell` for interior mutability"
                    .to_owned()
            )
        );
    }

    #[test]
    fn test_action_sub_error_display_with_subcategory() {
        let error = ActionSubError {
            category: "clang".to_owned(),
            message: Some("implicit conversion loses precision".to_owned()),
            file: Some("main.cpp".to_owned()),
            lnum: Some(42),
            end_lnum: None,
            col: Some(10),
            end_col: None,
            error_type: Some("warning".to_owned()),
            error_number: None,
            show_in_stderr: false,
            subcategory: Some("conversion".to_owned()),
            remediation: None,
        };

        let display = error.display();
        assert_eq!(
            display,
            Some(
                "[clang][conversion] main.cpp:42:10 warning: implicit conversion loses precision"
                    .to_owned()
            )
        );
    }
}
