/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use clap::ValueEnum;
use tracing::info;

use crate::path::safe_canonicalize;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub(crate) enum ProjectKind {
    Binary,
    Library,
}

pub(crate) struct New {
    pub(crate) name: String,
    pub(crate) kind: ProjectKind,
    pub(crate) path: Option<PathBuf>,
}

impl New {
    pub(crate) fn run(self) -> Result<(), anyhow::Error> {
        let name = self.name;

        let (target, kind) = match self.kind {
            ProjectKind::Library => {
                let target = Target::Library { name: name.clone() };
                let lib = EntryFile::Lib(LibFile);
                (target, lib)
            }
            ProjectKind::Binary => {
                let target = Target::Binary { name: name.clone() };
                let main = EntryFile::Main(MainFile);

                (target, main)
            }
        };

        let path = match self.path {
            Some(path) => path,
            None => std::env::current_dir()?,
        };
        let path = safe_canonicalize(&path);

        let project_dir = Path::new(&name);
        let path = path.join(project_dir);

        fs::create_dir(&path).context("Unable to create project directory")?;

        info!(?path);

        // create the `BUCK` file
        let buck_path = path.join("BUCK");
        let mut buck_file = fs::File::create(buck_path).context("Unable to create `BUCK` file")?;
        buck_file
            .write_all(target.render().as_bytes())
            .context("Unable to write generated template to `BUCK` file")?;

        // create src dir
        let src_dur = path.join(Path::new("src"));
        fs::create_dir(&src_dur).context("Unable to create `src/` directory")?;

        match kind {
            EntryFile::Main(main) => {
                let path = src_dur.join(Path::new("main.rs"));
                let mut main_file = fs::File::create(path).context("Unable to create main file")?;

                main_file
                    .write_all(main.render().as_bytes())
                    .context("Unable to write contents of main.rs")?;
            }
            EntryFile::Lib(lib) => {
                let path = src_dur.join(Path::new("lib.rs"));
                let mut lib_file = fs::File::create(path).context("Unable to create main file")?;

                lib_file
                    .write_all(lib.render().as_bytes())
                    .context("Unable to write contents of lib.rs")?;
            }
        }

        Ok(())
    }
}

enum EntryFile {
    Main(MainFile),
    Lib(LibFile),
}

struct MainFile;

impl MainFile {
    fn render(&self) -> String {
        "fn main() {
    println!(\"Hello from Rust at Meta!\");
}
"
        .into()
    }
}

struct LibFile;

impl LibFile {
    fn render(&self) -> String {
        "pub fn add_numbers(left: usize, right: usize) -> usize {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add_numbers(2, 2);
        assert_eq!(result, 4);
    }
}
"
        .into()
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Target {
    Library { name: String },
    Binary { name: String },
}

impl Target {
    fn render(&self) -> String {
        match self {
            Target::Library { name } => {
                let template = include_str!("../../templates/BUCK_LIB");
                template.replace("__NAME_PLACEHOLDER__", name)
            }
            Target::Binary { name } => {
                let template = include_str!("../../templates/BUCK_BIN");
                template.replace("__NAME_PLACEHOLDER__", name)
            }
        }
    }
}

#[test]
fn test_render() {
    let target = Target::Library {
        name: String::from("a-rust-library"),
    };

    let expected = r#"load("@fbsource//tools/build_defs:rust_library.bzl", "rust_library")

rust_library(
    name = "a-rust-library",
    srcs = glob(["src/**/*.rs"]),
    deps = [],
)
"#;

    assert_eq!(expected, target.render().replace("\r\n", "\n"));

    let target = Target::Binary {
        name: String::from("a-rust-binary"),
    };

    let expected = r#"load("@fbsource//tools/build_defs:rust_binary.bzl", "rust_binary")

rust_binary(
    name = "a-rust-binary",
    srcs = glob(["src/**/*.rs"]),
    deps = [],
)
"#;

    assert_eq!(expected, target.render().replace("\r\n", "\n"));
}

#[test]
fn test_run_creates_buck_file() {
    let parent = std::env::temp_dir().join(format!("rust-project-new-{}", std::process::id()));
    fs::create_dir_all(&parent).unwrap();

    New {
        name: String::from("a-rust-library"),
        kind: ProjectKind::Library,
        path: Some(parent.clone()),
    }
    .run()
    .unwrap();

    let project = parent.join("a-rust-library");
    let has_buck = project.join("BUCK").is_file();
    let has_targets = project.join("TARGETS").exists();
    fs::remove_dir_all(&parent).unwrap();

    assert!(has_buck, "`rust-project new` should create a `BUCK` file");
    assert!(
        !has_targets,
        "`rust-project new` should not create a `TARGETS` file"
    );
}
