/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;
use std::fmt::Display;
use std::fmt::Formatter;
use std::io::Write;

use async_trait::async_trait;
use buck2_build_api::actions::query::ActionQueryNode;
use buck2_build_api::actions::query::PRINT_ACTION_NODE;
use buck2_build_api::interpreter::rule_defs::provider::collection::FrozenProviderCollectionValue;
use buck2_core::cells::CellResolver;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_query::query::environment::AttrFmtOptions;
use buck2_query::query::environment::QueryTarget;
use buck2_query::query::environment::QueryTargets;
use buck2_query::query::syntax::simple::eval::file_set::FileSet;
use buck2_query::query::syntax::simple::eval::multi_query::MultiQueryResult;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use buck2_query::query::syntax::simple::eval::values::QueryEvaluationValue;
use buck2_util::indent::indent;
use dupe::Clone_;
use dupe::Copy_;
use dupe::Dupe_;
use indent_write::fmt::IndentWriter;
use indent_write::io::IndentWriter as IoIndentWriter;
use regex::RegexSet;
use serde::Serialize;
use serde::Serializer;
use serde::ser::SerializeMap;
use serde::ser::SerializeSeq;

use crate::dot::Dot;
use crate::dot::DotCompact;
use crate::dot::targets::DotTargetGraph;
use crate::html::Html;
use crate::query::QueryCommandError;
use crate::query::query_target_ext::QueryCommandTarget;
use crate::query_output_format::QueryOutputFormatInfo;

#[derive(Copy_, Dupe_, Clone_)]
pub(crate) enum ShouldPrintProviders<'a, T> {
    No,
    Yes(&'a dyn ProviderLookUp<T>),
}

#[async_trait]
pub(crate) trait ProviderLookUp<T: QueryTarget>: Send + Sync {
    async fn lookup(
        &self,
        t: &T,
    ) -> buck2_error::Result<MaybeCompatible<FrozenProviderCollectionValue>>;
}

#[derive(Debug)]
pub(crate) struct QueryResultPrinter<'a> {
    resolver: &'a CellResolver,
    attributes: Option<RegexSet>,
    output_format: QueryOutputFormatInfo,
}

struct TargetSetJsonPrinter<'a, T: QueryTarget> {
    value: Vec<PrintableQueryTarget<'a, T>>,
    is_complex: bool,
}

impl<'a, T: QueryCommandTarget> TargetSetJsonPrinter<'a, T> {
    async fn new(
        target_call_stacks: bool,
        print_providers: ShouldPrintProviders<'a, T>,
        attributes: &'a Option<RegexSet>,
        targets: &'a TargetSet<T>,
    ) -> buck2_error::Result<TargetSetJsonPrinter<'a, T>> {
        Ok(TargetSetJsonPrinter {
            value: printable_targets(targets, print_providers, attributes, target_call_stacks)
                .await?,
            is_complex: attributes.is_some()
                || target_call_stacks
                || matches!(print_providers, ShouldPrintProviders::Yes(_)),
        })
    }
}

struct PrintableQueryTarget<'a, T: QueryTarget> {
    value: &'a T,
    attributes: &'a Option<RegexSet>,
    providers: Option<FrozenProviderCollectionValue>,
    target_call_stacks: bool,
    /// Present if call stacks were asked for and the target has one.
    call_stack: Option<String>,
}

impl<T: QueryTarget> PrintableQueryTarget<'_, T> {
    fn label(&self) -> String {
        self.value.node_key().to_string()
    }
}

impl<T: QueryCommandTarget> Display for PrintableQueryTarget<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value.node_key())?;

        if self.target_call_stacks || self.providers.is_some() {
            writeln!(f)?;
        }

        if self.target_call_stacks {
            match &self.call_stack {
                Some(call_stack) => {
                    write!(f, "{}", indent("  ", call_stack))?;
                }
                None => {
                    // This is `aquery`.
                }
            }
        }

        if let Some(providers) = &self.providers {
            use std::fmt::Write;
            write!(
                IndentWriter::new("  ", f),
                "{:#}",
                providers.provider_collection()
            )?;
        }

        Ok(())
    }
}

/// Serde's error for the attribute walk, which reports an unreadable attribute
/// through the same channel.
struct AttrWalkError<E>(E);

impl<E: serde::ser::Error> From<buck2_error::Error> for AttrWalkError<E> {
    fn from(error: buck2_error::Error) -> Self {
        AttrWalkError(E::custom(error))
    }
}

impl<T: QueryCommandTarget> Serialize for PrintableQueryTarget<'_, T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(None)?;

        QueryTargets::for_all_attrs(self.value, |attr_name, attr_value| {
            if let Some(attr_regex) = self.attributes {
                if attr_regex.is_match(attr_name) {
                    struct AttrValueSerialize<'a, 'b, T: QueryCommandTarget> {
                        target: &'a T,
                        attr: &'a T::Attr<'b>,
                    }

                    impl<T: QueryCommandTarget> Serialize for AttrValueSerialize<'_, '_, T> {
                        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
                        where
                            S: Serializer,
                        {
                            self.target.attr_serialize(self.attr, serializer)
                        }
                    }

                    map.serialize_entry(
                        attr_name,
                        &AttrValueSerialize {
                            target: self.value,
                            attr: attr_value,
                        },
                    )
                    .map_err(AttrWalkError)?;
                }
            }
            Ok(())
        })
        .map_err(|AttrWalkError(error)| error)?;

        if self.target_call_stacks {
            map.serialize_entry("buck.target_call_stack", &self.call_stack)?;
        }

        if let Some(providers) = &self.providers {
            map.serialize_entry("buck.providers", providers)?;
        }

        map.end()
    }
}

impl<T: QueryCommandTarget> Serialize for TargetSetJsonPrinter<'_, T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.is_complex {
            serializer.collect_map(self.value.iter().map(|target| (target.label(), target)))
        } else {
            serializer.collect_seq(self.value.iter().map(|target| target.label()))
        }
    }
}

struct FileSetJsonPrinter<'a> {
    value: &'a FileSet,
    resolver: &'a CellResolver,
}

impl Serialize for FileSetJsonPrinter<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut seq = serializer.serialize_seq(None)?;
        for file in self.value.iter() {
            seq.serialize_element(
                &self
                    .resolver
                    .resolve_path(*file)
                    .map_err(serde::ser::Error::custom)?
                    .to_string(),
            )?;
        }
        SerializeSeq::end(seq)
    }
}

impl<'a> QueryResultPrinter<'a> {
    /// Utility for creating from the options in their protobuf form.
    pub(crate) fn from_request_options(
        resolver: &'a CellResolver,
        attributes: &[String],
        output_format: i32,
        trace_id: String,
    ) -> buck2_error::Result<Self> {
        Self::from_options(
            resolver,
            attributes,
            QueryOutputFormatInfo::from_protobuf_int(output_format, trace_id)
                .expect("cli should send a valid output_format enum"),
        )
    }

    pub(crate) fn from_options(
        resolver: &'a CellResolver,
        attributes: &[String],
        output_format: QueryOutputFormatInfo,
    ) -> buck2_error::Result<Self> {
        let output_format = match (output_format, attributes.is_empty()) {
            // following buck1's behavior, if any attributes are requested we use json output instead of list output
            (QueryOutputFormatInfo::Default, false) => QueryOutputFormatInfo::Json,
            (v, _) => v,
        };

        let attributes = if attributes.is_empty() {
            None
        } else {
            Some(RegexSet::new(attributes)?)
        };

        Ok(Self {
            resolver,
            attributes,
            output_format,
        })
    }

    pub(crate) async fn print_multi_output<'b, T: QueryCommandTarget, W: std::io::Write>(
        &self,
        mut output: W,
        multi_result: MultiQueryResult<T>,
        target_call_stacks: bool,
        print_providers: ShouldPrintProviders<'b, T>,
    ) -> buck2_error::Result<()> {
        match (&self.output_format, &self.attributes) {
            // A multi-query only has interesting output with --json output. For non-json output it gets merged together.
            // TODO(cjhopman): buck1 does this really odd thing that a multi-query that requests any attributes
            // gets the entire result merged together rather than printed as a multi-query. We match that behavior, but
            // it really doesn't make sense and we should migrate off of that.
            (QueryOutputFormatInfo::Json, None) => {
                let multi_result = multi_result.0;
                let mut captured_error = Ok(());

                {
                    let mut ser = serde_json::Serializer::pretty(&mut output);
                    let mut seq = ser.serialize_map(Some(multi_result.len()))?;
                    for (arg, result) in multi_result {
                        match result {
                            Ok(v) => match v {
                                QueryEvaluationValue::TargetSet(targets) => seq.serialize_entry(
                                    &arg,
                                    &TargetSetJsonPrinter::new(
                                        target_call_stacks,
                                        print_providers,
                                        &self.attributes,
                                        &targets,
                                    )
                                    .await?,
                                )?,
                                QueryEvaluationValue::FileSet(files) => seq.serialize_entry(
                                    &arg,
                                    &FileSetJsonPrinter {
                                        resolver: self.resolver,
                                        value: &files,
                                    },
                                )?,
                            },
                            Err(e) => {
                                seq.serialize_entry(
                                    &arg,
                                    &serde_json::json!({ "$error": format!("{:#}", e) }),
                                )?;
                                captured_error = Err(e);
                            }
                        }
                    }
                    SerializeMap::end(seq)?;
                }
                // need to add a newline to flush the output.
                writeln!(&mut output)?;
                Ok(captured_error?)
            }
            _ => {
                self.print_single_output(
                    output,
                    multi_result.merged()?,
                    target_call_stacks,
                    print_providers,
                )
                .await
            }
        }
    }

    pub(crate) async fn print_single_output<'b, T: QueryCommandTarget, W: std::io::Write>(
        &self,
        mut output: W,
        result: QueryEvaluationValue<T>,
        call_stack: bool,
        print_providers: ShouldPrintProviders<'b, T>,
    ) -> buck2_error::Result<()> {
        match result {
            QueryEvaluationValue::TargetSet(targets) => match &self.output_format {
                QueryOutputFormatInfo::Default => {
                    for target in
                        printable_targets(&targets, print_providers, &self.attributes, call_stack)
                            .await?
                    {
                        writeln!(&mut output, "{target}")?;
                    }
                }
                QueryOutputFormatInfo::Starlark => {
                    for (i, target) in targets.iter().enumerate() {
                        if i > 0 {
                            writeln!(&mut output)?;
                            writeln!(&mut output)?;
                        }
                        if call_stack {
                            match target.call_stack()? {
                                Some(call_stack) => {
                                    write!(&mut output, "{}", indent("#  ", &call_stack))?;
                                }
                                None => {
                                    // This is `aquery`.
                                }
                            }
                        }
                        writeln!(&mut output, "{}(", target.rule_type())?;
                        let mut inner_out = IoIndentWriter::new("  ", &mut output);
                        let mut attrs = BTreeMap::new();

                        target.defined_attrs_for_each(|k, v| {
                            attrs.insert(
                                k.to_owned(),
                                format!("{:#}", target.attr_display(v, AttrFmtOptions::default())),
                            );
                            buck2_error::Ok(())
                        })?;
                        if let Some(name) = attrs.remove("name") {
                            writeln!(&mut inner_out, "name = {name},")?;
                        }
                        for (k, v) in attrs {
                            writeln!(&mut inner_out, "{k} = {v},")?;
                        }
                        writeln!(&mut output, ")")?;
                    }
                }
                QueryOutputFormatInfo::Json => {
                    {
                        let mut ser = serde_json::Serializer::pretty(&mut output);
                        TargetSetJsonPrinter::new(
                            call_stack,
                            print_providers,
                            &self.attributes,
                            &targets,
                        )
                        .await?
                        .serialize(&mut ser)?;
                    }
                    // need to add a newline to flush the output.
                    writeln!(&mut output)?
                }
                QueryOutputFormatInfo::Dot => {
                    Dot::render(
                        &DotTargetGraph {
                            targets,
                            attributes: self.attributes.clone(),
                        },
                        &mut output,
                    )?;
                }
                QueryOutputFormatInfo::Html(trace_id) => {
                    Html::render(targets, &mut output, trace_id.clone()).await?
                }
                QueryOutputFormatInfo::DotCompact => {
                    DotCompact::render(
                        &DotTargetGraph {
                            targets,
                            attributes: self.attributes.clone(),
                        },
                        &mut output,
                    )?;
                }
            },
            QueryEvaluationValue::FileSet(files) => {
                if self.attributes.is_some() {
                    return Err(QueryCommandError::FileSetHasNoAttributes.into());
                }
                match self.output_format {
                    QueryOutputFormatInfo::Default | QueryOutputFormatInfo::Starlark => {
                        for file in files.iter() {
                            writeln!(&mut output, "{}", self.resolver.resolve_path(*file)?)?;
                        }
                    }
                    QueryOutputFormatInfo::Json => {
                        {
                            let mut ser = serde_json::Serializer::pretty(&mut output);
                            FileSetJsonPrinter {
                                resolver: self.resolver,
                                value: &files,
                            }
                            .serialize(&mut ser)?;
                        }
                        // need to add a newline to flush the output.
                        writeln!(&mut output)?;
                    }
                    QueryOutputFormatInfo::Dot => {
                        return Err(buck2_error::buck2_error!(
                            buck2_error::ErrorTag::Unimplemented,
                            "dot output for files not implemented yet"
                        ));
                    }
                    QueryOutputFormatInfo::DotCompact => {
                        return Err(buck2_error::buck2_error!(
                            buck2_error::ErrorTag::Unimplemented,
                            "dot_compact output for files not implemented yet"
                        ));
                    }
                    QueryOutputFormatInfo::Html(..) => {
                        return Err(buck2_error::buck2_error!(
                            buck2_error::ErrorTag::Unimplemented,
                            "html output for files not implemented yet"
                        ));
                    }
                }
            }
        }

        Ok(())
    }
}

async fn printable_targets<'a, T: QueryCommandTarget>(
    targets: &'a TargetSet<T>,
    print_providers: ShouldPrintProviders<'a, T>,
    attributes: &'a Option<RegexSet>,
    target_call_stacks: bool,
) -> buck2_error::Result<Vec<PrintableQueryTarget<'a, T>>> {
    buck2_util::future::join_all(targets.iter().map(|t| async move {
        Ok(PrintableQueryTarget {
            value: t,
            attributes,
            target_call_stacks,
            call_stack: if target_call_stacks {
                t.call_stack()?
            } else {
                None
            },
            providers: match print_providers {
                ShouldPrintProviders::No => None,
                ShouldPrintProviders::Yes(lookup) => {
                    Some(lookup.lookup(t).await?.require_compatible()?)
                }
            },
        })
    }))
    .await
    .into_iter()
    .collect::<buck2_error::Result<_>>()
}

async fn print_action_node(
    stdout: &mut (dyn Write + Send),
    action: ActionQueryNode,
    json: bool,
    output_attributes: &[String],
    cell_resolver: &CellResolver,
) -> buck2_error::Result<()> {
    // Dot/DotCompact output format don't make sense here.
    let output_format = if json {
        QueryOutputFormatInfo::Json
    } else {
        QueryOutputFormatInfo::Default
    };

    let query_result_printer =
        QueryResultPrinter::from_options(cell_resolver, output_attributes, output_format)?;

    let mut result = TargetSet::new();
    result.insert(action);

    query_result_printer
        .print_single_output(
            stdout,
            QueryEvaluationValue::TargetSet(result),
            false,
            ShouldPrintProviders::No,
        )
        .await
}

pub(crate) fn init_print_action_node() {
    PRINT_ACTION_NODE.init(|stdout, action, json, output_attributes, cell_resolver| {
        Box::pin(print_action_node(
            stdout,
            action,
            json,
            output_attributes,
            cell_resolver,
        ))
    });
}
