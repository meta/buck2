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
use std::fmt::Write as _;
use std::hash::Hash;
use std::hash::Hasher;

use allocative::Allocative;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::provider::label::ProvidersLabelMaybeConfigured;
use buck2_error::internal_error;
use buck2_hash::BuckMutMap;
use buck2_query_parser::BinaryOp;
use buck2_query_parser::Expr;
use dupe::Dupe;
use pagable::Pagable;
use serde::Serializer;
use serde_json::to_value;
use smallvec::SmallVec;
use smallvec::smallvec;
use strong_hash::StrongHash;

use crate::attrs::arena::AttrToken;
use crate::attrs::arena::AttrWord;
use crate::attrs::arena::CoercedAttrArena;
use crate::attrs::arena::CoercedAttrArenaBuilder;
use crate::attrs::arena::CoercedAttrHash;
use crate::attrs::arena::fields::ChildOf;
use crate::attrs::arena::fields::Cursor;
use crate::attrs::arena::fields::Extent;
use crate::attrs::arena::fields::Field;
use crate::attrs::arena::fields::IdHasher;
use crate::attrs::arena::fields::RecordWriter;
use crate::attrs::arena::schema::Kind;
use crate::attrs::arena::schema::View;
use crate::attrs::arena::schema::encode_kind;
use crate::attrs::arena::storage::RefRun;
use crate::attrs::attr_type::AttrTypeInner;
use crate::attrs::attr_type::arg::QueryExpansion;
use crate::attrs::attr_type::dep::DepAttrType;
use crate::attrs::coerced::configure::Configure;
use crate::attrs::coerced::kinds;
use crate::attrs::coerced::kinds::CoercedAttrKind;
use crate::attrs::coerced::render::Fmt;
use crate::attrs::coerced::render::MatchesAny;
use crate::attrs::coerced::render::SerializeKind;
use crate::attrs::coerced::render::ToJson;
use crate::attrs::coerced::semantics::Semantics;
use crate::attrs::coerced::semantics::mismatch;
use crate::attrs::coerced::traverse::Traverse;
use crate::attrs::configuration_context::AttrConfigurationContext;
use crate::attrs::configured_attr::ConfiguredAttr;
use crate::attrs::configured_traversal::ConfiguredAttrTraversal;
use crate::attrs::strong_hash_without_config::StrongHashWithoutConfig;
use crate::attrs::traversal::CoercedAttrTraversal;
use crate::provider_id_set::ProviderIdSet;

/// Attribute type created with `attrs.query(...)`.
#[derive(Debug, Pagable, Eq, PartialEq, Hash, Allocative, StrongHash)]
pub struct QueryAttrType {
    pub inner: DepAttrType,
}

impl QueryAttrType {
    pub fn new(inner: DepAttrType) -> Self {
        Self { inner }
    }
}

/// Attribute value of type `attrs.query(...)`.
#[derive(Debug, Eq, PartialEq, Hash, StrongHash, Clone, Allocative, Pagable)]
pub struct QueryAttr<P: ProvidersLabelMaybeConfigured> {
    pub providers: ProviderIdSet,
    pub query: QueryAttrBase<P>,
}

impl QueryAttr<ConfiguredProvidersLabel> {
    pub(crate) fn traverse(
        &self,
        traversal: &mut dyn ConfiguredAttrTraversal,
    ) -> buck2_error::Result<()> {
        self.query.traverse(traversal)
    }
}

impl StrongHashWithoutConfig for QueryAttr<ConfiguredProvidersLabel> {
    fn strong_hash_without_config<H: Hasher>(&self, state: &mut H) {
        let Self { providers, query } = self;
        providers.strong_hash(state);
        query.strong_hash_without_config(state);
    }
}

/// Query in target node attribute, like `$(query_outputs ...)`.
#[derive(Debug, Eq, PartialEq, Hash, Clone, Allocative, Pagable, StrongHash)]
pub struct QueryMacroBase<P: ProvidersLabelMaybeConfigured> {
    pub expansion_type: QueryExpansion,
    pub query: QueryAttrBase<P>,
}

impl<P: ProvidersLabelMaybeConfigured> Display for QueryMacroBase<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.expansion_type, self.query.query)?;
        Ok(())
    }
}

impl QueryMacroBase<ConfiguredProvidersLabel> {
    pub(crate) fn traverse(
        &self,
        traversal: &mut dyn ConfiguredAttrTraversal,
    ) -> buck2_error::Result<()> {
        self.query.traverse(traversal)
    }
}

impl StrongHashWithoutConfig for QueryMacroBase<ConfiguredProvidersLabel> {
    fn strong_hash_without_config<H: Hasher>(&self, state: &mut H) {
        let Self {
            expansion_type,
            query,
        } = self;
        expansion_type.strong_hash(state);
        query.strong_hash_without_config(state);
    }
}

impl QueryMacroBase<ProvidersLabel> {
    pub(crate) fn traverse<'a>(
        &'a self,
        traversal: &mut dyn CoercedAttrTraversal<'a>,
    ) -> buck2_error::Result<()> {
        self.query.traverse(traversal)
    }

    pub(crate) fn configure(
        &self,
        ctx: &dyn AttrConfigurationContext,
    ) -> buck2_error::Result<QueryMacroBase<ConfiguredProvidersLabel>> {
        Ok(QueryMacroBase {
            expansion_type: self.expansion_type.clone(),
            query: self.query.configure(ctx)?,
        })
    }
}

/// Query in target node.
///
/// Used in either:
/// * Attribute created with `attrs.query(...)`
/// * Query inside macros like `$(query_targets ...)`
#[derive(Debug, Eq, PartialEq, Hash, Clone, Allocative, Pagable, StrongHash)]
pub struct QueryAttrBase<P: ProvidersLabelMaybeConfigured> {
    pub query: String,
    pub resolved_literals: ResolvedQueryLiterals<P>,
}

type OffsetAndLength = (usize, usize);

#[derive(
    Debug,
    Eq,
    PartialEq,
    Hash,
    Clone,
    Allocative,
    strong_hash::StrongHash,
    Pagable
)]
pub struct ResolvedQueryLiterals<P: ProvidersLabelMaybeConfigured>(
    pub BTreeMap<OffsetAndLength, P>,
);

impl StrongHashWithoutConfig for ResolvedQueryLiterals<ConfiguredProvidersLabel> {
    fn strong_hash_without_config<H: Hasher>(&self, state: &mut H) {
        let Self(resolved_literals) = self;
        (resolved_literals.len() as u64).strong_hash(state);
        resolved_literals
            .iter()
            .for_each(|(&(offset, length), label)| {
                (offset as u64).strong_hash(state);
                (length as u64).strong_hash(state);
                label.strong_hash_without_config(state);
            });
    }
}

impl StrongHashWithoutConfig for QueryAttrBase<ConfiguredProvidersLabel> {
    fn strong_hash_without_config<H: Hasher>(&self, state: &mut H) {
        let Self {
            query,
            resolved_literals,
        } = self;
        query.strong_hash(state);
        resolved_literals.strong_hash_without_config(state);
    }
}

impl QueryAttrBase<ConfiguredProvidersLabel> {
    pub(crate) fn traverse(
        &self,
        traversal: &mut dyn ConfiguredAttrTraversal,
    ) -> buck2_error::Result<()> {
        // queries have no inputs.
        for dep in self.resolved_literals.0.values() {
            traversal.dep(dep)?;
        }
        traversal.query(&self.query, &self.resolved_literals)?;
        Ok(())
    }
}

impl QueryAttrBase<ProvidersLabel> {
    fn configure(
        &self,
        ctx: &dyn AttrConfigurationContext,
    ) -> buck2_error::Result<QueryAttrBase<ConfiguredProvidersLabel>> {
        Ok(QueryAttrBase {
            query: self.query.clone(),
            resolved_literals: ResolvedQueryLiterals(
                self.resolved_literals
                    .0
                    .iter()
                    .map(|(key, value)| Ok((*key, ctx.configure_target(value))))
                    .collect::<buck2_error::Result<_>>()?,
            ),
        })
    }

    fn traverse<'a>(
        &'a self,
        traversal: &mut dyn CoercedAttrTraversal<'a>,
    ) -> buck2_error::Result<()> {
        // queries don't have any configuration_deps or inputs currently.
        for dep in self.resolved_literals.0.values() {
            traversal.dep(dep)?;
        }
        Ok(())
    }
}

impl Kind for kinds::Query {
    type Fields = ChildOf<kinds::QueryExpr>;
}

impl CoercedAttrArenaBuilder {
    pub fn encode_parsed_query(&mut self, expr: AttrToken) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::Query>(self, expr)
    }
}

impl Semantics for kinds::Query {
    fn traverse<'a>(
        expr: View<'a, Self>,
        Traverse { v, t, tr, .. }: Traverse<'_, 'a>,
    ) -> buck2_error::Result<()> {
        // Queries have no inputs or configuration deps: literal labels only,
        // in expression order.
        if !matches!(&t.non_optional().0.inner, AttrTypeInner::Query(_)) {
            return Err(mismatch(v, t));
        }
        let mut labels = Vec::new();
        expr.labels(&mut labels);
        for label in labels {
            tr.dep(label)?;
        }
        Ok(())
    }

    fn configure(
        expr: View<'_, Self>,
        Configure { v, t, ctx, .. }: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        match &t.non_optional().0.inner {
            AttrTypeInner::Query(_) => ResultMaybeCompatible::Compatible(ConfiguredAttr::Query(
                Box::new(expr.configure(ctx)),
            )),
            _ => mismatch(v, t).into(),
        }
    }

    fn to_json(expr: View<'_, Self>, _: ToJson<'_>) -> buck2_error::Result<serde_json::Value> {
        Ok(to_value(expr.query().0)?)
    }

    fn fmt(expr: View<'_, Self>, Fmt { f, .. }: Fmt<'_, '_>) -> std::fmt::Result {
        write!(f, "\"{}\"", expr.query().0)
    }

    fn any_matches(
        expr: View<'_, Self>,
        MatchesAny { filter }: MatchesAny<'_>,
    ) -> buck2_error::Result<bool> {
        filter(&expr.query().0)
    }

    fn serialize<S: Serializer>(
        expr: View<'_, Self>,
        SerializeKind { ser, .. }: SerializeKind<'_, S>,
    ) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&expr.query().0)
    }
}

// Query-expression node subtags (first head field of a `QueryExpr` record).
const Q_EXPR_NONE: u32 = 0;
const Q_EXPR_WORD: u32 = 1;
const Q_EXPR_STRING: u32 = 2;
const Q_EXPR_INT: u32 = 3;
const Q_EXPR_LABEL: u32 = 4;
const Q_EXPR_FUNCTION: u32 = 5;
const Q_EXPR_BINOP: u32 = 6;
const Q_EXPR_SET: u32 = 7;
const Q_EXPR_FILESET: u32 = 8;

fn binary_op_bits(op: BinaryOp) -> u32 {
    match op {
        BinaryOp::Intersect => 0,
        BinaryOp::Except => 1,
        BinaryOp::Union => 2,
    }
}

fn binary_op_from_bits(bits: u32) -> BinaryOp {
    match bits {
        0 => BinaryOp::Intersect,
        1 => BinaryOp::Except,
        2 => BinaryOp::Union,
        _ => unreachable!("should be valid: records are only written by the encoder"),
    }
}

/// One query-expression node: head `[subtag, nv, values..., n]` then `n`
/// refs; the view is a handle that decodes the node on demand.
pub struct Node;

impl Field for Node {
    const DEPS: bool = true;
    const INLINE: bool = false;
    const HAS_TEXT: bool = false;
    type View<'a> = QueryExprRef<'a>;
    type Spec<'s> = &'s QueryNode<'s>;

    fn shape(peek: &dyn Fn(u32) -> Option<u32>, at: u32) -> Option<Extent> {
        let head = 3u32.checked_add(peek(at + 1)?)?;
        Some(Extent {
            head,
            refs: Some(peek(at + head - 1)?),
            text: None,
        })
    }

    fn decode<'a>(c: &mut Cursor<'a>) -> QueryExprRef<'a> {
        QueryExprRef {
            arena: c.arena,
            r: c.r,
        }
    }

    fn encode<'s>(
        node: &'s QueryNode<'s>,
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'s>,
    ) -> buck2_error::Result<()> {
        write_node(b, w, node)
    }

    /// Node for node and hash for hash the same as [`Self::encode`] from
    /// the parsed expression.
    fn reemit<'a>(
        e: QueryExprRef<'a>,
        b: &mut CoercedAttrArenaBuilder,
        w: &mut RecordWriter<'a>,
    ) -> buck2_error::Result<()> {
        let node = node_of_view(b, e)?;
        write_node(b, w, &node)
    }

    fn identity(e: QueryExprRef<'_>, h: &mut IdHasher<'_>) {
        e.strong_hash(&mut h.h);
    }

    fn same(a: QueryExprRef<'_>, b: QueryExprRef<'_>) -> bool {
        a.same_expr(&b)
    }
}

impl Kind for kinds::QueryExpr {
    type Fields = Node;
}

/// Handle to one stored query-expression node ([`CoercedAttrKind::QueryExpr`]);
/// `kind` decodes it shallowly.
#[derive(Copy, Clone)]
pub struct QueryExprRef<'a> {
    pub(crate) arena: &'a CoercedAttrArena,
    pub(crate) r: AttrWord,
}

/// Shallow decode of a [`QueryExprRef`]. Children decode lazily via further
/// `kind` calls.
enum QueryExprKind<'a> {
    None,
    /// Bare word (set members, unresolved tokens).
    Word(&'a str),
    /// Quoted string.
    String(&'a str),
    Int(u64),
    /// A target literal, resolved at coercion.
    Label(&'a ProvidersLabel),
    Function {
        name: &'a str,
        args: QueryExprSeq<'a>,
    },
    BinOp {
        first: QueryExprRef<'a>,
        rest: QueryExprOpSeq<'a>,
    },
    Set(QueryExprSeq<'a>),
    FileSet(QueryExprSeq<'a>),
}

/// View of a run of query-expression child refs.
#[derive(Copy, Clone)]
struct QueryExprSeq<'a> {
    run: RefRun<'a>,
}

impl<'a> QueryExprSeq<'a> {
    fn len(&self) -> u32 {
        self.run.len()
    }

    fn iter(&self) -> impl Iterator<Item = QueryExprRef<'a>> + 'a {
        let arena = self.run.arena;
        self.run.iter().map(move |r| QueryExprRef { arena, r })
    }
}

/// View of a binop node's tail: `n` operators in the head, paired with `n`
/// operand refs.
#[derive(Copy, Clone)]
struct QueryExprOpSeq<'a> {
    ops: RefRun<'a>,
    run: RefRun<'a>,
}

impl<'a> QueryExprOpSeq<'a> {
    fn len(&self) -> u32 {
        self.run.len()
    }

    fn iter(&self) -> impl Iterator<Item = (BinaryOp, QueryExprRef<'a>)> + 'a {
        let arena = self.run.arena;
        self.ops
            .iter_raw()
            .zip(self.run.iter())
            .map(move |(op, r)| (binary_op_from_bits(op), QueryExprRef { arena, r }))
    }
}

impl<'a> QueryExprRef<'a> {
    /// The canonical reprint of the stored expression plus its literal map,
    /// with offsets into the reprint. One walk produces both, so the map's
    /// spans always agree with the returned text.
    pub fn query(&self) -> (String, ResolvedQueryLiterals<ProvidersLabel>) {
        let mut out = String::new();
        let mut lits = BTreeMap::new();
        self.reprint(&mut out, &mut |span, l| {
            lits.insert(span, l.dupe());
        });
        (out, ResolvedQueryLiterals(lits))
    }

    /// The `attrs.query()` value this expression is, configured.
    pub(crate) fn configure(
        &self,
        ctx: &dyn AttrConfigurationContext,
    ) -> QueryAttr<ConfiguredProvidersLabel> {
        let mut query = String::new();
        let mut lits = BTreeMap::new();
        self.reprint(&mut query, &mut |span, l| {
            lits.insert(span, ctx.configure_target(l));
        });
        QueryAttr {
            providers: ProviderIdSet::EMPTY,
            query: QueryAttrBase {
                query,
                resolved_literals: ResolvedQueryLiterals(lits),
            },
        }
    }

    fn kind(&self) -> QueryExprKind<'a> {
        let arena = self.arena;
        let mut c = Cursor::new(arena, self.r);
        let subtag = c.raw();
        let nv = c.raw();
        let vals = c.raws(nv);
        let n = c.raw();
        let refs = c.refs(n);
        match subtag {
            Q_EXPR_NONE => QueryExprKind::None,
            Q_EXPR_WORD | Q_EXPR_STRING => {
                let text = arena.str_at(refs.get(0));
                if subtag == Q_EXPR_WORD {
                    QueryExprKind::Word(text)
                } else {
                    QueryExprKind::String(text)
                }
            }
            Q_EXPR_INT => QueryExprKind::Int(vals.raw(0) as u64 | ((vals.raw(1) as u64) << 32)),
            Q_EXPR_LABEL => QueryExprKind::Label(&arena.providers_labels[vals.raw(0) as usize]),
            Q_EXPR_FUNCTION => QueryExprKind::Function {
                name: arena.str_at(refs.get(0)),
                args: QueryExprSeq { run: refs.skip(1) },
            },
            Q_EXPR_BINOP => QueryExprKind::BinOp {
                first: QueryExprRef {
                    arena,
                    r: refs.get(0),
                },
                rest: QueryExprOpSeq {
                    ops: vals,
                    run: refs.skip(1),
                },
            },
            Q_EXPR_SET | Q_EXPR_FILESET => {
                let seq = QueryExprSeq { run: refs };
                if subtag == Q_EXPR_SET {
                    QueryExprKind::Set(seq)
                } else {
                    QueryExprKind::FileSet(seq)
                }
            }
            _ => unreachable!("should be valid: query nodes are only written by the encoder"),
        }
    }

    /// Reprints the node in the parser `Display` form
    /// (`buck2_query_parser::Expr`), handing each literal label and its span
    /// in the reprint to `lit`. Reparsing the reprint yields the same
    /// expression.
    pub(crate) fn reprint(
        &self,
        out: &mut String,
        lit: &mut dyn FnMut((usize, usize), &'a ProvidersLabel),
    ) {
        match self.kind() {
            QueryExprKind::None => out.push_str("None"),
            QueryExprKind::Word(t) if buck2_query_parser::is_unquoted_word(t) => out.push_str(t),
            QueryExprKind::Word(t) | QueryExprKind::String(t) => {
                let quote = Self::quote_for(t);
                out.push(quote);
                out.push_str(t);
                out.push(quote);
            }
            QueryExprKind::Int(v) => write!(out, "{v}").expect("should write: String"),
            QueryExprKind::Label(label) => {
                let mut start = out.len();
                write!(out, "{label}").expect("should write: String");
                // Target names and subtarget brackets reach outside the
                // unquoted-word grammar.
                if !buck2_query_parser::is_unquoted_word(&out[start..]) {
                    let quote = Self::quote_for(&out[start..]);
                    out.insert(start, quote);
                    out.push(quote);
                    start += 1;
                    lit((start, out.len() - 1 - start), label);
                } else {
                    lit((start, out.len() - start), label);
                }
            }
            QueryExprKind::Function { name, args } => {
                out.push_str(name);
                out.push('(');
                for (i, a) in args.iter().enumerate() {
                    if i != 0 {
                        out.push_str(", ");
                    }
                    a.reprint(out, lit);
                }
                out.push(')');
            }
            QueryExprKind::BinOp { first, rest } => {
                for _ in 0..rest.len() {
                    out.push('(');
                }
                out.push(' ');
                first.reprint(out, lit);
                for (op, ex) in rest.iter() {
                    out.push(' ');
                    out.push_str(match op {
                        BinaryOp::Intersect => "^",
                        BinaryOp::Except => "-",
                        BinaryOp::Union => "+",
                    });
                    out.push(' ');
                    ex.reprint(out, lit);
                    out.push(')');
                }
            }
            QueryExprKind::Set(items) => Self::reprint_set("set(", items, out, lit),
            QueryExprKind::FileSet(items) => Self::reprint_set("fileset(", items, out, lit),
        }
    }

    /// The query grammar has no escapes, so quoted text holds at most one
    /// quote kind.
    fn quote_for(t: &str) -> char {
        if t.contains('\'') { '"' } else { '\'' }
    }

    fn reprint_set(
        prefix: &str,
        items: QueryExprSeq<'a>,
        out: &mut String,
        lit: &mut dyn FnMut((usize, usize), &'a ProvidersLabel),
    ) {
        out.push_str(prefix);
        for (i, v) in items.iter().enumerate() {
            if i != 0 {
                out.push(' ');
            }
            v.reprint(out, lit);
        }
        out.push(')');
    }

    /// Collects the node's literal labels, in expression order.
    pub(crate) fn labels(&self, out: &mut Vec<&'a ProvidersLabel>) {
        match self.kind() {
            QueryExprKind::Label(l) => out.push(l),
            QueryExprKind::Function { args, .. } => {
                for a in args.iter() {
                    a.labels(out);
                }
            }
            QueryExprKind::BinOp { first, rest } => {
                first.labels(out);
                for (_, ex) in rest.iter() {
                    ex.labels(out);
                }
            }
            QueryExprKind::Set(items) | QueryExprKind::FileSet(items) => {
                for v in items.iter() {
                    v.labels(out);
                }
            }
            QueryExprKind::None
            | QueryExprKind::Word(_)
            | QueryExprKind::String(_)
            | QueryExprKind::Int(_) => {}
        }
    }
}

/// Subtag plus content, children in order.
impl QueryExprRef<'_> {
    /// Equal expressions reprint identically with identical literal maps.
    fn same_expr(&self, other: &QueryExprRef<'_>) -> bool {
        fn reprinted(e: &QueryExprRef<'_>) -> (String, BTreeMap<(usize, usize), ProvidersLabel>) {
            let mut out = String::new();
            let mut lits = BTreeMap::new();
            e.reprint(&mut out, &mut |span, l| {
                lits.insert(span, l.dupe());
            });
            (out, lits)
        }
        reprinted(self) == reprinted(other)
    }
}

impl PartialEq for QueryExprRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.same_expr(other)
    }
}

impl StrongHash for QueryExprRef<'_> {
    fn strong_hash<H: Hasher>(&self, h: &mut H) {
        match self.kind() {
            QueryExprKind::None => h.write_u32(Q_EXPR_NONE),
            QueryExprKind::Word(t) => {
                h.write_u32(Q_EXPR_WORD);
                t.strong_hash(h);
            }
            QueryExprKind::String(t) => {
                h.write_u32(Q_EXPR_STRING);
                t.strong_hash(h);
            }
            QueryExprKind::Int(v) => {
                h.write_u32(Q_EXPR_INT);
                h.write_u64(v);
            }
            QueryExprKind::Label(l) => {
                h.write_u32(Q_EXPR_LABEL);
                l.strong_hash(h);
            }
            QueryExprKind::Function { name, args } => {
                h.write_u32(Q_EXPR_FUNCTION);
                name.strong_hash(h);
                h.write_u32(args.len());
                for a in args.iter() {
                    a.strong_hash(h);
                }
            }
            QueryExprKind::BinOp { first, rest } => {
                h.write_u32(Q_EXPR_BINOP);
                first.strong_hash(h);
                h.write_u32(rest.len());
                for (op, ex) in rest.iter() {
                    h.write_u32(binary_op_bits(op));
                    ex.strong_hash(h);
                }
            }
            QueryExprKind::Set(items) => {
                h.write_u32(Q_EXPR_SET);
                h.write_u32(items.len());
                for v in items.iter() {
                    v.strong_hash(h);
                }
            }
            QueryExprKind::FileSet(items) => {
                h.write_u32(Q_EXPR_FILESET);
                h.write_u32(items.len());
                for v in items.iter() {
                    v.strong_hash(h);
                }
            }
        }
    }
}

/// One node of a query expression as its record stores it, children
/// already encoded. Both the parser's expression and a decoded node convert
/// to this, so the record layout and its content hash are defined once.
pub enum QueryNode<'a> {
    None,
    Word(&'a str),
    String(&'a str),
    Int(u64),
    Label(&'a ProvidersLabel),
    Function {
        name: &'a str,
        args: SmallVec<[AttrToken; 8]>,
    },
    BinOp {
        first: AttrToken,
        rest: SmallVec<[(BinaryOp, AttrToken); 8]>,
    },
    Set(SmallVec<[AttrToken; 16]>),
    FileSet(SmallVec<[AttrToken; 16]>),
}

/// Writes one node record: `[subtag, nv, values..., n]` then refs; the
/// content hash folds the subtag and the node's content.
fn write_node(
    b: &mut CoercedAttrArenaBuilder,
    w: &mut RecordWriter<'_>,
    node: &QueryNode<'_>,
) -> buck2_error::Result<()> {
    let mut values: SmallVec<[u32; 8]> = smallvec![];
    let mut refs: SmallVec<[AttrWord; 16]> = smallvec![];
    let subtag = match node {
        QueryNode::None => {
            w.hash(&Q_EXPR_NONE);
            Q_EXPR_NONE
        }
        QueryNode::Word(t) | QueryNode::String(t) => {
            let subtag = match node {
                QueryNode::Word(_) => Q_EXPR_WORD,
                _ => Q_EXPR_STRING,
            };
            let AttrToken(sr, sh) = b.encode_str(CoercedAttrKind::String, t)?;
            refs.push(sr);
            w.hash(&(subtag, sh));
            subtag
        }
        QueryNode::Int(v) => {
            values.extend([*v as u32, (*v >> 32) as u32]);
            w.hash(&(Q_EXPR_INT, v));
            Q_EXPR_INT
        }
        QueryNode::Label(label) => {
            let (idx, lh) = b.providers_label_idx_and_hash(label);
            values.push(idx);
            w.hash(&(Q_EXPR_LABEL, lh));
            Q_EXPR_LABEL
        }
        QueryNode::Function { name, args } => {
            let AttrToken(nr, nh) = b.encode_str(CoercedAttrKind::String, name)?;
            refs.push(nr);
            refs.extend(args.iter().map(|a| a.0));
            let hashes: SmallVec<[CoercedAttrHash; 8]> = args.iter().map(|a| a.1).collect();
            w.hash(&(Q_EXPR_FUNCTION, nh, &hashes));
            Q_EXPR_FUNCTION
        }
        QueryNode::BinOp { first, rest } => {
            values.extend(rest.iter().map(|(op, _)| binary_op_bits(*op)));
            refs.push(first.0);
            refs.extend(rest.iter().map(|(_, x)| x.0));
            let hashes: SmallVec<[CoercedAttrHash; 8]> = rest
                .iter()
                .zip(&values)
                .map(|((_, x), &opw)| CoercedAttrHash(x.1.0 ^ ((opw as u64) << 60)))
                .collect();
            w.hash(&(Q_EXPR_BINOP, first.1, &hashes));
            Q_EXPR_BINOP
        }
        QueryNode::Set(items) | QueryNode::FileSet(items) => {
            let subtag = match node {
                QueryNode::Set(_) => Q_EXPR_SET,
                _ => Q_EXPR_FILESET,
            };
            refs.extend(items.iter().map(|i| i.0));
            let hashes: SmallVec<[CoercedAttrHash; 16]> = items.iter().map(|i| i.1).collect();
            w.hash(&(subtag, &hashes));
            subtag
        }
    };
    w.head(subtag);
    w.head(values.len() as u32);
    for v in values {
        w.head(v);
    }
    w.head(refs.len() as u32);
    for r in refs {
        w.r(r);
    }
    Ok(())
}

impl CoercedAttrArenaBuilder {
    /// Encode an `attrs.query()` value as its parsed expression.
    pub fn encode_query(
        &mut self,
        q: &QueryAttrBase<ProvidersLabel>,
    ) -> buck2_error::Result<AttrToken> {
        let parsed = buck2_query_parser::parse_expr(&q.query)?;
        let expr = self.encode_query_expr(&parsed, &q.query, &q.resolved_literals)?;
        self.encode_parsed_query(expr)
    }

    /// Encode a parsed query expression. Literal text (from
    /// `resolved_literals`) becomes providers-label refs; every other text
    /// becomes a shared text record.
    ///
    /// Coercion records one literal per distinct text, keyed by the span of
    /// its first occurrence, and evaluation resolves literals by text; so a
    /// token is a literal whenever its text matches a recorded one, repeated
    /// occurrences included.
    pub(crate) fn encode_query_expr(
        &mut self,
        e: &buck2_query_parser::SpannedExpr<'_>,
        src: &str,
        lits: &ResolvedQueryLiterals<ProvidersLabel>,
    ) -> buck2_error::Result<AttrToken> {
        let by_text: BuckMutMap<&str, &ProvidersLabel> = lits
            .0
            .iter()
            .map(|(&(off, len), label)| (&src[off..off + len], label))
            .collect();
        let mut matched: BuckMutMap<&str, ()> = BuckMutMap::default();
        let r = self.encode_parsed_node(e, &by_text, &mut matched)?;
        if matched.len() != by_text.len() {
            return Err(internal_error!(
                "query literals must all match expression nodes: matched {} of {}",
                matched.len(),
                by_text.len()
            ));
        }
        Ok(r)
    }

    fn encode_parsed_node<'s>(
        &mut self,
        e: &buck2_query_parser::SpannedExpr<'s>,
        by_text: &BuckMutMap<&'s str, &ProvidersLabel>,
        matched: &mut BuckMutMap<&'s str, ()>,
    ) -> buck2_error::Result<AttrToken> {
        // A word or string whose text names a resolved literal is that
        // literal.
        let mut literal_or =
            |text: &'s str, as_text: fn(&'s str) -> QueryNode<'s>| match by_text.get(text) {
                Some(label) => {
                    matched.insert(text, ());
                    QueryNode::Label(label)
                }
                None => as_text(text),
            };
        let node = match &e.value {
            Expr::None => QueryNode::None,
            Expr::String(w) => literal_or(w, QueryNode::String),
            Expr::Integer(v) => QueryNode::Int(*v),
            Expr::Function {
                function_name,
                args,
            } => QueryNode::Function {
                name: function_name.fragment(),
                args: args
                    .iter()
                    .map(|a| self.encode_parsed_node(a, by_text, matched))
                    .collect::<buck2_error::Result<_>>()?,
            },
            Expr::BinaryOpSequence(first, rest) => QueryNode::BinOp {
                first: self.encode_parsed_node(first, by_text, matched)?,
                rest: rest
                    .iter()
                    .map(|(op, ex)| Ok((*op, self.encode_parsed_node(ex, by_text, matched)?)))
                    .collect::<buck2_error::Result<_>>()?,
            },
            Expr::Set(vals) | Expr::FileSet(vals) => {
                let items = vals
                    .iter()
                    .map(|v| self.encode_query_node(literal_or(v.fragment(), QueryNode::Word)))
                    .collect::<buck2_error::Result<_>>()?;
                match &e.value {
                    Expr::Set(_) => QueryNode::Set(items),
                    _ => QueryNode::FileSet(items),
                }
            }
        };
        self.encode_query_node(node)
    }

    /// [`Self::encode_query_expr`] from a decoded expression: node for node
    /// and hash for hash.
    pub(crate) fn encode_query_expr_view(
        &mut self,
        e: QueryExprRef<'_>,
    ) -> buck2_error::Result<AttrToken> {
        let node = node_of_view(self, e)?;
        encode_kind::<kinds::QueryExpr>(self, &node)
    }
}

/// A decoded node as the spec of its copy, children already copied.
fn node_of_view<'a>(
    b: &mut CoercedAttrArenaBuilder,
    e: QueryExprRef<'a>,
) -> buck2_error::Result<QueryNode<'a>> {
    Ok(match e.kind() {
        QueryExprKind::None => QueryNode::None,
        QueryExprKind::Word(t) => QueryNode::Word(t),
        QueryExprKind::String(t) => QueryNode::String(t),
        QueryExprKind::Int(v) => QueryNode::Int(v),
        QueryExprKind::Label(l) => QueryNode::Label(l),
        QueryExprKind::Function { name, args } => QueryNode::Function {
            name,
            args: args
                .iter()
                .map(|a| b.encode_query_expr_view(a))
                .collect::<buck2_error::Result<_>>()?,
        },
        QueryExprKind::BinOp { first, rest } => QueryNode::BinOp {
            first: b.encode_query_expr_view(first)?,
            rest: rest
                .iter()
                .map(|(op, ex)| Ok((op, b.encode_query_expr_view(ex)?)))
                .collect::<buck2_error::Result<_>>()?,
        },
        QueryExprKind::Set(items) => QueryNode::Set(
            items
                .iter()
                .map(|v| b.encode_query_expr_view(v))
                .collect::<buck2_error::Result<_>>()?,
        ),
        QueryExprKind::FileSet(items) => QueryNode::FileSet(
            items
                .iter()
                .map(|v| b.encode_query_expr_view(v))
                .collect::<buck2_error::Result<_>>()?,
        ),
    })
}

impl CoercedAttrArenaBuilder {
    fn encode_query_node(&mut self, node: QueryNode<'_>) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::QueryExpr>(self, &node)
    }
}
