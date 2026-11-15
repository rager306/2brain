//! Effective meta: which `meta:` blocks a type-def surfaces, per meta type
//! ([[type-def meta::au-type-system]]).
//!
//! `meta:` is NOT inherited as data, it is computed at read time over the
//! owner-relative [`DefClosure`], each def read in the repo that owns it.
//! - a def DECLARES a meta type `T` when it carries a block of `T`, or `meta: []`.
//! - a declaration is shadowed by any declaration of `T` on a strict descendant.
//! - the survivors are the effective meta for `T`, a surviving `meta: []`
//!   contributing nothing, token-equal blocks unifying to one.
//! - more than one distinct survivor is a conflict, every one reported, and
//!   [`meta_conflicts`] emits `meta-inheritance-conflict`.
//!
//! Order- and path-independent: neither the `extends:` order nor the path
//! reaching an ancestor changes the answer. The same whole-block rule as
//! `location:` ([`crate::location_check::effective_locations`]).
//!
//! A meta TYPE is identity, a block is per-def data. A block's `type:` is read
//! relative to its declaring def's repo (a bare `type: myMeta` on a peer def
//! names the peer's `myMeta`), then resolved to its `(name, ClosureHash)`
//! identity in that owner's graph, the same `TypeId` the required-meta check
//! uses. So an in-sync own `myMeta` and peer `myMeta::peer` are ONE meta type,
//! and a diverged pair is two.
//!
//! The walk targets the host's `extends:` chain, never the chains of the meta
//! type-defs the blocks name ([[type-def meta::au-type-system]]).

use std::collections::{BTreeMap, BTreeSet};

use tb_diagnostics::{Diagnostic, Severity, Span};

use crate::closure_id::ClosureHash;
use crate::codes;
use crate::def_closure::{DefClosure, DefKey};
use crate::graph::TypeGraph;
use crate::instance::{InlineValue, InstanceField, InstanceValue};
use crate::resolution::PeerGraphResolver;
use crate::typedef::{MetaBlock, TypeDef, TypeName};

/// A meta type's grouping key: its identity when it resolves, else the one
/// written form it came from, so two different unresolvable types stay apart.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum MetaKey {
    Resolved(TypeName, ClosureHash),
    Unresolved(DefKey),
}

/// One meta type's effective blocks on a type-def.
#[derive(Debug)]
pub struct MetaEntry<'g> {
    /// The meta type's name.
    pub name: TypeName,
    /// Its closure hash, the identity half beside `name`. `None` when the meta
    /// type does not resolve (diagnosed elsewhere), the block still surfaces.
    pub hash: Option<ClosureHash>,
    /// The resolved written forms the surviving blocks name, each `name` read
    /// in the repo it resolves in. More than one when in-sync copies in several
    /// repos are named.
    pub forms: Vec<DefKey>,
    /// The surviving blocks, each with the def that declares it, key-ordered.
    /// Never empty. More than one is a conflict.
    pub blocks: Vec<(DefKey, &'g MetaBlock)>,
}

impl MetaEntry<'_> {
    /// True when unrelated parents surface distinct blocks of this meta type:
    /// the surviving blocks come from two or more defs. Distinct blocks on ONE
    /// def are a `duplicate-meta-block`, not an inheritance conflict.
    pub fn is_conflict(&self) -> bool {
        self.blocks
            .iter()
            .map(|(k, _)| k)
            .collect::<BTreeSet<_>>()
            .len()
            > 1
    }
}

/// The written form a block's meta type names, relative to its declaring def's
/// repo.
fn block_form(block: &MetaBlock, declaring: &DefKey) -> DefKey {
    DefKey {
        name: block.type_name.clone(),
        repo: block.repo.clone().unwrap_or_else(|| declaring.repo.clone()),
    }
}

/// The identity a written form resolves to in its owner's graph.
fn key_of(resolver: &dyn PeerGraphResolver, form: &DefKey) -> MetaKey {
    match resolver
        .graph_of(&form.repo)
        .and_then(|g| g.closure_id(&form.name))
    {
        Some(hash) => MetaKey::Resolved(form.name.clone(), hash),
        None => MetaKey::Unresolved(form.clone()),
    }
}

/// True when some def in the closure carries at least one meta block.
fn carries_meta(td: &TypeDef) -> bool {
    td.meta_blocks.as_ref().is_some_and(|b| !b.is_empty())
}

/// The effective meta of `host` read in `own_repo`, one entry per meta type
/// with at least one surviving block, ordered by meta type.
///
/// Empty when `host` does not resolve, or no def in its closure carries a
/// block (the common case, answered without a shadowing pass).
pub fn effective_meta<'g>(
    resolver: &'g dyn PeerGraphResolver,
    own_repo: &str,
    host: &TypeName,
) -> Vec<MetaEntry<'g>> {
    let closure = DefClosure::walk(
        resolver,
        [DefKey {
            name: host.clone(),
            repo: own_repo.to_string(),
        }],
    );
    if closure.none(carries_meta) {
        return Vec::new();
    }
    let mut keys: BTreeSet<MetaKey> = BTreeSet::new();
    for (key, td) in closure.iter() {
        for block in td.meta_blocks.iter().flatten() {
            keys.insert(key_of(resolver, &block_form(block, key)));
        }
    }
    keys.into_iter()
        .filter_map(|mk| {
            let entry = entry_for(&closure, resolver, &mk);
            (!entry.blocks.is_empty()).then_some(entry)
        })
        .collect()
}

/// The surviving blocks of one meta type over a closure.
fn entry_for<'g>(
    closure: &DefClosure<'g>,
    resolver: &dyn PeerGraphResolver,
    mk: &MetaKey,
) -> MetaEntry<'g> {
    // Every block of this meta type on the def. One def can name one identity
    // twice (an own and an in-sync peer form), and each is kept rather than
    // silently dropping the second.
    let blocks_of = |key: &DefKey, td: &'g TypeDef| -> Vec<&'g MetaBlock> {
        td.meta_blocks
            .iter()
            .flatten()
            .filter(|b| key_of(resolver, &block_form(b, key)) == *mk)
            .collect()
    };
    let declares = |key: &DefKey, td: &TypeDef| match &td.meta_blocks {
        Some(blocks) if blocks.is_empty() => true,
        Some(blocks) => blocks
            .iter()
            .any(|b| key_of(resolver, &block_form(b, key)) == *mk),
        None => false,
    };
    let mut blocks: Vec<(DefKey, &'g MetaBlock)> = Vec::new();
    let mut forms: BTreeSet<DefKey> = BTreeSet::new();
    for (key, td) in closure.survivors(declares) {
        // A surviving `meta: []` declares the type with no block.
        for block in blocks_of(key, td) {
            forms.insert(block_form(block, key));
            if !blocks
                .iter()
                .any(|(k, b)| block_eq(resolver, (b, &k.repo), (block, &key.repo)))
            {
                blocks.push((key.clone(), block));
            }
        }
    }
    let (name, hash) = match mk {
        MetaKey::Resolved(name, hash) => (name.clone(), Some(*hash)),
        MetaKey::Unresolved(form) => (form.name.clone(), None),
    };
    MetaEntry {
        name,
        hash,
        forms: forms.into_iter().collect(),
        blocks,
    }
}

/// The surviving blocks of ONE meta type on `host`, the single-type query.
///
/// `meta_repo` is the meta type's `::repo` qualifier as written from
/// `own_repo`, `None` for an own meta type. The query resolves to the meta
/// type's identity, so it also finds blocks naming an in-sync copy of it in
/// another repo. Empty when none surfaces; more than one is a conflict, see
/// [`MetaEntry::is_conflict`].
pub fn lookup_meta<'g>(
    resolver: &'g dyn PeerGraphResolver,
    own_repo: &str,
    host: &TypeName,
    meta_type: &TypeName,
    meta_repo: Option<&str>,
) -> Vec<(DefKey, &'g MetaBlock)> {
    let wanted = key_of(
        resolver,
        &DefKey {
            name: meta_type.clone(),
            repo: meta_repo.unwrap_or(own_repo).to_string(),
        },
    );
    effective_meta(resolver, own_repo, host)
        .into_iter()
        .find(|e| entry_key(e) == wanted)
        .map(|e| e.blocks)
        .unwrap_or_default()
}

fn entry_key(e: &MetaEntry) -> MetaKey {
    match e.hash {
        Some(h) => MetaKey::Resolved(e.name.clone(), h),
        None => MetaKey::Unresolved(e.forms[0].clone()),
    }
}

/// `meta-inheritance-conflict` for every def in `repo`'s own graph whose
/// effective meta holds two or more distinct blocks of one meta type.
///
/// Anchored at the def, `related` at each surviving block. The meta type and
/// each source render relative to `repo`, bare when owned there, `name::repo`
/// otherwise.
pub fn meta_conflicts(
    resolver: &dyn PeerGraphResolver,
    repo: &str,
    graph: &TypeGraph,
) -> Vec<Diagnostic> {
    let authored = |k: &DefKey| {
        if k.repo == repo {
            k.name.as_str().to_string()
        } else {
            format!("{}::{}", k.name.as_str(), k.repo)
        }
    };
    let mut diags = Vec::new();
    for (name, td) in graph.iter() {
        for entry in effective_meta(resolver, repo, name) {
            if !entry.is_conflict() {
                continue;
            }
            // An in-sync meta type may be named by several forms; prefer the
            // one this repo would write bare.
            let form = entry
                .forms
                .iter()
                .find(|f| f.repo == repo)
                .unwrap_or(&entry.forms[0]);
            let meta = authored(form);
            let sources: Vec<String> = entry.blocks.iter().map(|(k, _)| authored(k)).collect();
            let related: Vec<Span> = entry
                .blocks
                .iter()
                .filter_map(|(k, b)| {
                    resolver
                        .graph_of(&k.repo)
                        .and_then(|g| g.get(&k.name))
                        .map(|def| Span::new(def.source_path.clone(), b.block_span))
                })
                .collect();
            diags.push(Diagnostic {
                code: codes::META_INHERITANCE_CONFLICT,
                severity: Severity::Warning,
                span: Span::new(td.source_path.clone(), td.source_span),
                message: format!(
                    "type-def '{}' inherits {} different '{}' blocks from unrelated parents ({}); declare its own '{}' block to choose",
                    td.name.as_str(),
                    entry.blocks.len(),
                    meta,
                    sources.join(", "),
                    meta
                ),
                related,
                fix: None,
            });
        }
    }
    diags
}

/// `duplicate-meta-block` for a def carrying two blocks that name ONE meta
/// identity through DIFFERENT written forms, an own `myMeta` beside an in-sync
/// `myMeta::peer`, or a bare and a self-qualified form.
///
/// The singleton rule is per meta type ([[type-def meta::au-type-system]]), and a
/// meta type is its identity. A same-form pair is the own-graph structure
/// pass's (`load_checks`), which needs no resolver, so it is skipped here and
/// never double-fires. An unresolvable form has no identity to share.
pub fn duplicate_meta_identities(
    resolver: &dyn PeerGraphResolver,
    repo: &str,
    graph: &TypeGraph,
) -> Vec<Diagnostic> {
    let authored = |f: &DefKey| {
        if f.repo == repo {
            f.name.as_str().to_string()
        } else {
            format!("{}::{}", f.name.as_str(), f.repo)
        }
    };
    let mut diags = Vec::new();
    for (_, td) in graph.iter() {
        let host = DefKey {
            name: td.name.clone(),
            repo: repo.to_string(),
        };
        // Keyed by identity; the WRITTEN form (name, qualifier as authored) is
        // what the own-graph pass keys on, so an equal written form is its case.
        type Written<'a> = (&'a TypeName, Option<&'a str>);
        let mut first: BTreeMap<MetaKey, (Written, DefKey, &MetaBlock)> = BTreeMap::new();
        for block in td.meta_blocks.iter().flatten() {
            let written: Written = (&block.type_name, block.repo.as_deref());
            let form = block_form(block, &host);
            let key = key_of(resolver, &form);
            if matches!(key, MetaKey::Unresolved(_)) {
                continue;
            }
            match first.get(&key) {
                Some((prev_written, prev_form, prev)) if *prev_written != written => {
                    diags.push(Diagnostic {
                        code: codes::DUPLICATE_META_BLOCK,
                        severity: Severity::Error,
                        span: Span::new(td.source_path.clone(), block.block_span),
                        message: format!(
                            "duplicate `meta:` sub-region for '{}' on type-def '{}': '{}' names the same meta type",
                            written_form(block),
                            td.name.as_str(),
                            authored(prev_form)
                        ),
                        related: vec![Span::new(td.source_path.clone(), prev.block_span)],
                        fix: None,
                    });
                }
                Some(_) => {}
                None => {
                    first.insert(key, (written, form, block));
                }
            }
        }
    }
    diags
}

/// A block's meta type as authored, `name` or `name::repo`.
fn written_form(block: &MetaBlock) -> String {
    match &block.repo {
        Some(r) => format!("{}::{}", block.type_name.as_str(), r),
        None => block.type_name.as_str().to_string(),
    }
}

/// Token-equal blocks of one meta type: structurally equal fields, spans and
/// docstrings aside, each block read relative to its declaring repo. The
/// written `type:` is not compared, the caller already grouped by identity. A
/// record's `^:` id is not part of its value
/// ([[type value container::au-type-system]]), so it is ignored too.
fn block_eq(
    resolver: &dyn PeerGraphResolver,
    (a, a_repo): (&MetaBlock, &str),
    (b, b_repo): (&MetaBlock, &str),
) -> bool {
    fields_eq(resolver, (&a.fields, a_repo), (&b.fields, b_repo))
}

fn fields_eq(
    resolver: &dyn PeerGraphResolver,
    (a, a_repo): (&[InstanceField], &str),
    (b, b_repo): (&[InstanceField], &str),
) -> bool {
    let keyed = |fs: &[InstanceField]| -> BTreeMap<String, InstanceValue> {
        fs.iter()
            .map(|f| (f.key.clone(), f.value.clone()))
            .collect()
    };
    let (ka, kb) = (keyed(a), keyed(b));
    ka.len() == kb.len()
        && ka.iter().all(|(k, va)| {
            kb.get(k)
                .is_some_and(|vb| value_eq(resolver, (va, a_repo), (vb, b_repo)))
        })
}

fn value_eq(
    resolver: &dyn PeerGraphResolver,
    (a, a_repo): (&InstanceValue, &str),
    (b, b_repo): (&InstanceValue, &str),
) -> bool {
    use InstanceValue as V;
    match (a, b) {
        (V::Sequence(x), V::Sequence(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|(p, q)| value_eq(resolver, (&p.value, a_repo), (&q.value, b_repo)))
        }
        (V::Mapping(x), V::Mapping(y)) => {
            claim_keys(resolver, x, a_repo) == claim_keys(resolver, y, b_repo)
                && fields_eq(resolver, (&x.fields, a_repo), (&y.fields, b_repo))
        }
        // An unmodelled value has no known equality: never equal, so a
        // conflict is reported rather than hidden.
        (V::NotYetSupported, _) | (_, V::NotYetSupported) => false,
        (V::Sequence(_), _) | (V::Mapping(_), _) => false,
        _ => a == b,
    }
}

/// A nested record's claim as resolved identities, each name read relative to
/// the repo of the block that wrote it, so a peer's bare `foo` and an
/// importer's `foo::peer` compare equal.
fn claim_keys(
    resolver: &dyn PeerGraphResolver,
    record: &InlineValue,
    repo: &str,
) -> Option<Vec<MetaKey>> {
    record.type_claim.as_ref().map(|c| {
        c.iter()
            .map(|n| {
                key_of(
                    resolver,
                    &DefKey {
                        name: n.name.clone(),
                        repo: n.repo.clone().unwrap_or_else(|| repo.to_string()),
                    },
                )
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::def_closure::SingleGraph;
    use crate::graph::build_graph;
    use crate::resolution::PeerGraphResolver;
    use crate::typedef::{FieldDecl, FieldName, ParentClaim, ParentClaimForm, TypeNameClaim};
    use std::path::PathBuf;
    use tb_diagnostics::ByteRange;

    fn td(name: &str, parents: &[&str]) -> TypeDef {
        let parent_claim = if parents.is_empty() {
            None
        } else {
            Some(ParentClaim {
                form: if parents.len() == 1 {
                    ParentClaimForm::BareName
                } else {
                    ParentClaimForm::List
                },
                value_span: ByteRange::new(0, 0),
            })
        };
        TypeDef {
            shape: None,
            name: TypeName(name.into()),
            source_path: PathBuf::from(format!("/v/{name}.type.yaml")),
            source_span: ByteRange::new(0, 0),
            parent_claim,
            parents: parents
                .iter()
                .map(|p| TypeNameClaim::own(TypeName((*p).into()), ByteRange::new(0, 0)))
                .collect(),
            fields: vec![],
            sealed: vec![],
            declared_abstract: false,
            meta_blocks: None,
            required_meta: Vec::new(),
            body: None,
            doc: None,
            ..Default::default()
        }
    }

    fn with_meta(mut t: TypeDef, blocks: Vec<MetaBlock>) -> TypeDef {
        t.meta_blocks = Some(blocks);
        t
    }

    /// Mark `meta: []` (the [[type-def meta::au-type-system]] suppression form) on a type-def.
    fn with_suppressed_meta(mut t: TypeDef) -> TypeDef {
        t.meta_blocks = Some(vec![]);
        t
    }

    fn meta_block(type_name: &str) -> MetaBlock {
        MetaBlock {
            type_name: TypeName(type_name.into()),
            repo: None,
            type_name_span: ByteRange::new(0, 0),
            block_span: ByteRange::new(0, 0),
            fields: vec![],
            body_span: ByteRange::new(0, 0),
            doc: None,
            field_docs: Default::default(),
        }
    }

    /// Build a single-field meta-type-def. Just needs to exist so the host's
    /// `meta:` entry resolves to something nameable.
    fn meta_typedef(name: &str) -> TypeDef {
        TypeDef {
            shape: None,
            name: TypeName(name.into()),
            source_path: PathBuf::from(format!("/v/{name}.type.yaml")),
            source_span: ByteRange::new(0, 0),
            parent_claim: None,
            parents: vec![],
            fields: vec![FieldDecl {
                name: FieldName("tldr".into()),
                optional: true,
                raw_shape: "String".into(),
                name_span: ByteRange::new(0, 0),
                shape_span: ByteRange::new(0, 0),
                entry_span: ByteRange::new(0, 0),
                parsed_shape: Ok(tb_grammar::Shape::Primitive(tb_grammar::Primitive::String)),
                doc: None,
            }],
            sealed: vec![],
            declared_abstract: false,
            meta_blocks: None,
            required_meta: Vec::new(),
            body: None,
            doc: None,
            ..Default::default()
        }
    }

    fn field(key: &str, value: &str) -> crate::instance::InstanceField {
        crate::instance::InstanceField {
            key: key.into(),
            key_span: ByteRange::new(0, 0),
            value: crate::instance::InstanceValue::String(value.into()),
            value_span: ByteRange::new(0, 0),
            nav_links: vec![],
        }
    }

    fn meta_block_with(type_name: &str, tldr: &str) -> MetaBlock {
        let mut b = meta_block(type_name);
        b.fields = vec![field("tldr", tldr)];
        b
    }

    fn tn(s: &str) -> TypeName {
        TypeName(s.into())
    }

    #[test]
    fn host_own_meta_wins() {
        // Host declares its own display-meta — `lookup_meta` returns it
        // directly, no ancestor walk.
        let g = build_graph(vec![
            meta_typedef("display-meta"),
            with_meta(td("decision", &[]), vec![meta_block("display-meta")]),
        ])
        .graph;
        let r = SingleGraph(&g);
        let found = lookup_meta(&r, "v", &tn("decision"), &tn("display-meta"), None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1.type_name.as_str(), "display-meta");
    }

    fn peer_meta_block(type_name: &str, repo: &str) -> MetaBlock {
        MetaBlock {
            type_name: TypeName(type_name.into()),
            repo: Some(repo.into()),
            type_name_span: ByteRange::new(0, 0),
            block_span: ByteRange::new(0, 0),
            fields: vec![],
            body_span: ByteRange::new(0, 0),
            doc: None,
            field_docs: Default::default(),
        }
    }

    struct TwoRepos(BTreeMap<&'static str, crate::graph::TypeGraph>);
    impl PeerGraphResolver for TwoRepos {
        fn graph_of(&self, repo: &str) -> Option<&crate::graph::TypeGraph> {
            self.0.get(repo)
        }
    }

    /// A host in `v` carrying an own `display-meta` block and a
    /// `display-meta::other` block with a different value; `other` defines its
    /// own `display-meta`, identical to `v`'s or not.
    fn own_and_peer(peer_meta: TypeDef) -> TwoRepos {
        let mut own = meta_block_with("display-meta", "own");
        own.block_span = ByteRange::new(1, 2);
        let mut peer = peer_meta_block("display-meta", "other");
        peer.fields = vec![field("tldr", "peer")];
        peer.block_span = ByteRange::new(3, 4);
        TwoRepos(BTreeMap::from([
            (
                "v",
                build_graph(vec![
                    meta_typedef("display-meta"),
                    with_meta(td("host", &[]), vec![own, peer]),
                ])
                .graph,
            ),
            ("other", build_graph(vec![peer_meta]).graph),
        ]))
    }

    #[test]
    fn a_diverged_same_named_peer_meta_is_a_distinct_type() {
        let mut diverged = meta_typedef("display-meta");
        diverged.fields[0].raw_shape = "Number".into();
        diverged.fields[0].parsed_shape =
            Ok(tb_grammar::Shape::Primitive(tb_grammar::Primitive::Number));
        let r = own_and_peer(diverged);
        let own = lookup_meta(&r, "v", &tn("host"), &tn("display-meta"), None);
        let peer = lookup_meta(&r, "v", &tn("host"), &tn("display-meta"), Some("other"));
        assert_eq!((own.len(), own[0].1.block_span.start), (1, 1));
        assert_eq!((peer.len(), peer[0].1.block_span.start), (1, 3));
        assert_eq!(effective_meta(&r, "v", &tn("host")).len(), 2);
    }

    #[test]
    fn an_in_sync_same_named_peer_meta_is_one_type() {
        // Identical `display-meta` in both repos: one identity, so the own and
        // the `::other` block are two blocks of ONE meta type on one def, both
        // kept (a `duplicate-meta-block`, not an inheritance conflict), and
        // both queries answer the same.
        let r = own_and_peer(meta_typedef("display-meta"));
        let entries = effective_meta(&r, "v", &tn("host"));
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0].forms.len(), 2, "named from both repos");
        let own = lookup_meta(&r, "v", &tn("host"), &tn("display-meta"), None);
        let peer = lookup_meta(&r, "v", &tn("host"), &tn("display-meta"), Some("other"));
        assert_eq!(own.len(), 2);
        assert_eq!(own.len(), peer.len());
        assert!(
            !entries[0].is_conflict(),
            "one def's two blocks are a duplicate"
        );
    }

    #[test]
    fn an_unsupported_value_never_unifies() {
        let mut a = meta_block("display-meta");
        a.fields = vec![field("tldr", "x")];
        a.fields[0].value = crate::instance::InstanceValue::NotYetSupported;
        let b = a.clone();
        let g = build_graph(vec![
            meta_typedef("display-meta"),
            with_meta(td("a", &[]), vec![a]),
            with_meta(td("b", &[]), vec![b]),
            td("host", &["a", "b"]),
        ])
        .graph;
        let r = SingleGraph(&g);
        assert_eq!(
            lookup_meta(&r, "v", &tn("host"), &tn("display-meta"), None).len(),
            2
        );
    }

    #[test]
    fn parent_fallback_when_host_absent() {
        // Host has no `meta:` of its own; parent declares display-meta.
        // Walk falls through to parent and returns parent's block.
        let g = build_graph(vec![
            meta_typedef("display-meta"),
            with_meta(td("note", &[]), vec![meta_block("display-meta")]),
            td("decision", &["note"]),
        ])
        .graph;
        let r = SingleGraph(&g);
        let found = lookup_meta(&r, "v", &tn("decision"), &tn("display-meta"), None);
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn returns_none_when_no_ancestor_declares() {
        // Neither host nor any ancestor declares the requested meta.
        // Walk exhausts → None.
        let g = build_graph(vec![
            meta_typedef("display-meta"),
            td("note", &[]),
            td("decision", &["note"]),
        ])
        .graph;
        let r = SingleGraph(&g);
        assert!(lookup_meta(&r, "v", &tn("decision"), &tn("display-meta"), None).is_empty());
    }

    #[test]
    fn suppression_marker_stops_walk() {
        // Host declares `meta: []` ([[type-def meta::au-type-system]] suppression). Parent declares
        // display-meta. Walk halts at host's suppression → None.
        let g = build_graph(vec![
            meta_typedef("display-meta"),
            with_meta(td("note", &[]), vec![meta_block("display-meta")]),
            with_suppressed_meta(td("decision", &["note"])),
        ])
        .graph;
        let r = SingleGraph(&g);
        assert!(lookup_meta(&r, "v", &tn("decision"), &tn("display-meta"), None).is_empty());
    }

    #[test]
    fn host_with_partial_meta_resolves_others_via_walk() {
        // Host declares ONLY runtime-meta; parent declares display-meta.
        // Querying for runtime-meta → host's; querying for display-meta
        // walks up → parent's. Locks the "has metas, different names"
        // case from suppression (which would stop the walk).
        let g = build_graph(vec![
            meta_typedef("display-meta"),
            meta_typedef("runtime-meta"),
            with_meta(td("note", &[]), vec![meta_block("display-meta")]),
            with_meta(td("decision", &["note"]), vec![meta_block("runtime-meta")]),
        ])
        .graph;
        let r = SingleGraph(&g);
        let host_owned = lookup_meta(&r, "v", &tn("decision"), &tn("runtime-meta"), None);
        let inherited = lookup_meta(&r, "v", &tn("decision"), &tn("display-meta"), None);
        assert_eq!(host_owned.len(), 1);
        assert_eq!(host_owned[0].1.type_name.as_str(), "runtime-meta");
        assert_eq!(inherited.len(), 1);
        assert_eq!(inherited[0].1.type_name.as_str(), "display-meta");
    }

    #[test]
    fn descendant_redeclares_after_suppressing_ancestor() {
        // Chain: note(display-meta) → decision(meta:[]) → committed(display-meta).
        // Query at `committed` for display-meta → committed's own; for
        // runtime-meta → walks up, hits decision's `meta:[]`, returns
        // None. Mirrors [[type-def meta::au-type-system]] "Other names remain suppressed for that
        // subtree until something further down re-declares them."
        let mut display_for_committed = meta_block("display-meta");
        display_for_committed.block_span = ByteRange::new(100, 200);
        let g = build_graph(vec![
            meta_typedef("display-meta"),
            meta_typedef("runtime-meta"),
            with_meta(td("note", &[]), vec![meta_block("display-meta")]),
            with_suppressed_meta(td("decision", &["note"])),
            with_meta(td("committed", &["decision"]), vec![display_for_committed]),
        ])
        .graph;
        let r = SingleGraph(&g);
        let display_at_committed =
            lookup_meta(&r, "v", &tn("committed"), &tn("display-meta"), None);
        assert_eq!(display_at_committed.len(), 1);
        // Verify we got committed's block specifically — span is 100..200,
        // not 0..0 like note's.
        assert_eq!(display_at_committed[0].1.block_span.start, 100);

        let runtime_at_committed =
            lookup_meta(&r, "v", &tn("committed"), &tn("runtime-meta"), None);
        assert!(runtime_at_committed.is_empty());
    }

    #[test]
    fn unrelated_parents_with_distinct_blocks_are_a_conflict_in_either_order() {
        // host extends [a, b], each declaring a different display-meta block.
        // Neither shadows the other, so BOTH surface, whatever the listing order.
        let mut a_block = meta_block("display-meta");
        a_block.block_span = ByteRange::new(10, 20);
        a_block.fields = vec![field("tldr", "a")];
        let mut b_block = meta_block("display-meta");
        b_block.block_span = ByteRange::new(30, 40);
        b_block.fields = vec![field("tldr", "b")];
        for parents in [["a", "b"], ["b", "a"]] {
            let g = build_graph(vec![
                meta_typedef("display-meta"),
                with_meta(td("a", &[]), vec![a_block.clone()]),
                with_meta(td("b", &[]), vec![b_block.clone()]),
                td("host", &parents),
            ])
            .graph;
            let r = SingleGraph(&g);
            let found = lookup_meta(&r, "v", &tn("host"), &tn("display-meta"), None);
            let starts: Vec<usize> = found.iter().map(|(_, b)| b.block_span.start).collect();
            assert_eq!(starts, vec![10, 30], "parents {parents:?}");
            let diags = meta_conflicts(&r, "v", &g);
            assert_eq!(diags.len(), 1, "{diags:?}");
            assert_eq!(diags[0].code, codes::META_INHERITANCE_CONFLICT);
            assert_eq!(diags[0].related.len(), 2);
            assert!(diags[0].message.contains("(a, b)"), "{}", diags[0].message);
        }
    }

    #[test]
    fn token_equal_blocks_from_unrelated_parents_unify() {
        let g = build_graph(vec![
            meta_typedef("display-meta"),
            with_meta(td("a", &[]), vec![meta_block_with("display-meta", "same")]),
            with_meta(td("b", &[]), vec![meta_block_with("display-meta", "same")]),
            td("host", &["a", "b"]),
        ])
        .graph;
        let r = SingleGraph(&g);
        assert_eq!(
            lookup_meta(&r, "v", &tn("host"), &tn("display-meta"), None).len(),
            1
        );
        assert!(meta_conflicts(&r, "v", &g).is_empty());
    }

    #[test]
    fn a_break_shadows_its_ancestor_on_every_path() {
        // host extends [broken, other]; both extend note, which declares
        // display-meta; broken declares `meta: []`. broken is note's strict
        // descendant, so note's block is shadowed even though other reaches it.
        let g = build_graph(vec![
            meta_typedef("display-meta"),
            with_meta(td("note", &[]), vec![meta_block("display-meta")]),
            with_suppressed_meta(td("broken", &["note"])),
            td("other", &["note"]),
            td("host", &["broken", "other"]),
        ])
        .graph;
        let r = SingleGraph(&g);
        assert!(lookup_meta(&r, "v", &tn("host"), &tn("display-meta"), None).is_empty());
        assert!(effective_meta(&r, "v", &tn("host")).is_empty());
    }

    #[test]
    fn a_nearer_block_on_a_sibling_branch_does_not_beat_a_deeper_one() {
        // host extends [a, b]; a -> x declares, b declares. Unrelated, so both,
        // depth and listing order irrelevant.
        let g = build_graph(vec![
            meta_typedef("display-meta"),
            with_meta(td("x", &[]), vec![meta_block_with("display-meta", "x")]),
            td("a", &["x"]),
            with_meta(td("b", &[]), vec![meta_block_with("display-meta", "b")]),
            td("host", &["a", "b"]),
        ])
        .graph;
        let r = SingleGraph(&g);
        let from: Vec<String> = lookup_meta(&r, "v", &tn("host"), &tn("display-meta"), None)
            .into_iter()
            .map(|(k, _)| k.name.as_str().to_string())
            .collect();
        assert_eq!(from, vec!["b", "x"]);
    }

    #[test]
    fn walk_does_not_recurse_into_encountered_metas() {
        // [[type-def meta::au-type-system]] lock: if the meta-type-def `display-meta` itself carries
        // a `meta:` block for `runtime-meta`, querying the host for
        // `runtime-meta` must NOT return display-meta's runtime-meta.
        // The walk targets the host's TYPE chain, not the meta-type-def's
        // own meta chain.
        let g = build_graph(vec![
            // `display-meta` itself carries a `runtime-meta` sub-region.
            // If the walk wrongly recursed into encountered metas, this
            // would be returned when querying the host for runtime-meta.
            with_meta(
                meta_typedef("display-meta"),
                vec![meta_block("runtime-meta")],
            ),
            meta_typedef("runtime-meta"),
            // Host declares only display-meta; no ancestor declares
            // runtime-meta.
            with_meta(td("decision", &[]), vec![meta_block("display-meta")]),
        ])
        .graph;
        let r = SingleGraph(&g);
        assert!(lookup_meta(&r, "v", &tn("decision"), &tn("runtime-meta"), None).is_empty());
        // And display-meta should still resolve.
        assert!(!lookup_meta(&r, "v", &tn("decision"), &tn("display-meta"), None).is_empty());
    }

    #[test]
    fn unknown_host_returns_none() {
        // Defensive: looking up a host that isn't in the graph returns
        // None. Callers can pass any TypeName without pre-checking.
        let g = build_graph(vec![meta_typedef("display-meta")]).graph;
        let r = SingleGraph(&g);
        assert!(lookup_meta(&r, "v", &tn("nonexistent"), &tn("display-meta"), None).is_empty());
    }
}
