/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Module for parsing bazel labels

use std::fmt;

#[derive(PartialEq, Eq, Debug, Clone)]
pub struct Label {
    // The repository can be omitted, in which case the label is relative to the current repository
    pub repo: Option<LabelRepo>,
    // The package can be omitted, in which case the label is relative to the current package
    pub package: Option<String>,
    pub name: String,
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub struct LabelRepo {
    pub name: String,
    pub is_canonical: bool,
}

#[derive(thiserror::Error, Debug)]
#[error("Unable to parse the label `{}`", .label)]
pub struct LabelParseError {
    label: String,
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(repo) = &self.repo {
            fmt::Display::fmt(&repo, f)?;
        }

        if let Some(package) = &self.package {
            f.write_str("//")?;
            f.write_str(package)?;
        }

        f.write_str(":")?;
        f.write_str(&self.name)?;

        Ok(())
    }
}

impl fmt::Display for LabelRepo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.is_canonical { "@@" } else { "@" })?;
        f.write_str(&self.name)?;

        Ok(())
    }
}

impl Label {
    pub fn parse(label: &str) -> Result<Self, LabelParseError> {
        match label.split_once("//") {
            Some((repo_part, rest)) => {
                let repo = if repo_part.is_empty() {
                    None
                } else {
                    Some(Self::parse_repo(repo_part).ok_or_else(|| LabelParseError {
                        label: label.to_owned(),
                    })?)
                };

                let (package, name) = rest.split_once(':').unwrap_or_else(|| {
                    // Here the name is implicit, and comes from the last component of the package name
                    if let Some(index) = rest.rfind('/') {
                        (rest, &rest[index + 1..])
                    } else {
                        (rest, rest)
                    }
                });

                Ok(Label {
                    name: name.to_owned(),
                    package: Some(package.to_owned()),
                    repo,
                })
            }
            // Either we have a repo only (@foo or @@foo), or just a name (foo or :foo)
            None => {
                if let Some(repo) = Self::parse_repo(label) {
                    Ok(Label {
                        name: repo.name.to_owned(),
                        repo: Some(repo),
                        package: Some("".to_owned()),
                    })
                } else {
                    let name = label.strip_prefix(':').unwrap_or(label);

                    Ok(Label {
                        repo: None,
                        name: name.to_owned(),
                        package: None,
                    })
                }
            }
        }
    }

    fn parse_repo(repo: &str) -> Option<LabelRepo> {
        if let Some(repo_name) = repo.strip_prefix("@@") {
            Some(LabelRepo {
                name: repo_name.to_owned(),
                is_canonical: true,
            })
        } else if let Some(repo_name) = repo.strip_prefix('@') {
            Some(LabelRepo {
                name: repo_name.to_owned(),
                is_canonical: false,
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Label;
    use crate::bazel::label::LabelRepo;

    #[test]
    fn test_parsing_repo_only_labels() {
        assert_eq!(
            Label::parse("@foo").unwrap(),
            Label {
                repo: Some(LabelRepo {
                    is_canonical: false,
                    name: "foo".to_owned(),
                }),
                package: Some("".to_owned()),
                name: "foo".to_owned(),
            }
        );

        assert_eq!(
            Label::parse("@@foo").unwrap(),
            Label {
                repo: Some(LabelRepo {
                    is_canonical: true,
                    name: "foo".to_owned(),
                }),
                package: Some("".to_owned()),
                name: "foo".to_owned(),
            }
        );
    }

    #[test]
    fn test_parsing_name_only_labels() {
        assert_eq!(
            Label::parse("foo").unwrap(),
            Label {
                repo: None,
                package: None,
                name: "foo".to_owned(),
            }
        );

        assert_eq!(
            Label::parse(":foo").unwrap(),
            Label {
                repo: None,
                package: None,
                name: "foo".to_owned(),
            }
        );
    }

    #[test]
    fn test_full_labels() {
        assert_eq!(
            Label::parse("//foo/bar:baz").unwrap(),
            Label {
                repo: None,
                package: Some("foo/bar".to_owned()),
                name: "baz".to_owned(),
            }
        );

        assert_eq!(
            Label::parse("@foo//foo/bar:baz").unwrap(),
            Label {
                repo: Some(LabelRepo {
                    name: "foo".to_owned(),
                    is_canonical: false
                }),
                package: Some("foo/bar".to_owned()),
                name: "baz".to_owned(),
            }
        );

        assert_eq!(
            Label::parse("@@foo//foo/bar:baz").unwrap(),
            Label {
                repo: Some(LabelRepo {
                    name: "foo".to_owned(),
                    is_canonical: true
                }),
                package: Some("foo/bar".to_owned()),
                name: "baz".to_owned(),
            }
        );
    }

    #[test]
    fn test_labels_with_implicit_name() {
        assert_eq!(
            Label::parse("@foo//bar/baz").unwrap(),
            Label {
                repo: Some(LabelRepo {
                    name: "foo".to_owned(),
                    is_canonical: false
                }),
                package: Some("bar/baz".to_owned()),
                name: "baz".to_owned(),
            }
        );

        assert_eq!(
            Label::parse("@foo//bar").unwrap(),
            Label {
                repo: Some(LabelRepo {
                    name: "foo".to_owned(),
                    is_canonical: false
                }),
                package: Some("bar".to_owned()),
                name: "bar".to_owned(),
            }
        );

        assert_eq!(
            Label::parse("@@foo//bar").unwrap(),
            Label {
                repo: Some(LabelRepo {
                    name: "foo".to_owned(),
                    is_canonical: true
                }),
                package: Some("bar".to_owned()),
                name: "bar".to_owned(),
            }
        );
    }

    #[test]
    fn test_invalid_labels() {
        assert!(Label::parse("foo//bar/baz").is_err());
    }

    #[test]
    fn test_displaying_labels() {
        assert_eq!(format!("{}", Label::parse(":foo.bzl").unwrap()), ":foo.bzl");
        assert_eq!(
            format!("{}", Label::parse("@foo//bar/baz:qux").unwrap()),
            "@foo//bar/baz:qux"
        );
        assert_eq!(
            format!("{}", Label::parse("//foo/bar").unwrap()),
            "//foo/bar:bar"
        );
    }

    /// The implicit name is the last component of the package.
    #[test]
    fn test_implicit_name_is_the_last_package_component() {
        let label = Label::parse("//a/b/c").unwrap();
        assert_eq!(label.package.as_deref(), Some("a/b/c"));
        assert_eq!(label.name, "c");
        assert_eq!(
            Label::parse("@repo//foo/bar/baz").unwrap().to_string(),
            "@repo//foo/bar/baz:baz"
        );
    }
}
