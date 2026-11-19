//! The owner-relative def closure: a type-def's ancestors walked as DEFS, each
//! read in the repo that owns it.
//!
//! The resolution fold ([`crate::resolution`]) keys a node by `TypeId`, the
//! content identity, so two same-named defs in different repos that hash equal
//! fold to ONE node, carrying whichever def the fold reached first. That is
//! right for identity and wrong for what the hash leaves out: `meta:` and
//! `location:` are descriptive per-def data, so two in-sync defs can carry
//! different blocks. This walk never merges them.
//!
//! A node is `(name, repo)`, the def `name` read in `repo`'s own graph.
//! - a claim or parent written bare stays in the repo of the def that wrote it.
//! - `name::repo` crosses to that repo.
//!
//! It serves the whole-block inheritance both keys share
//! ([[type-def meta::tb-type-system]], [[type-def location::tb-type-system]]):
//! a declaration is shadowed by any declaration on a strict descendant in the
//! closure, see [`DefClosure::survivors`].

use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet};

use crate::graph::TypeGraph;
use crate::instance::TypeClaim;
use crate::resolution::PeerGraphResolver;
use crate::typedef::{TypeDef, TypeName};

/// One def in the walk, `name` read in `repo`. Ordered name-first, so a
/// survivor list reads in type-name order, repo breaking a same-name tie.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DefKey {
    pub name: TypeName,
    pub repo: String,
}

#[derive(Debug)]
struct DefEntry<'g> {
    def: &'g TypeDef,
    parents: Vec<DefKey>,
}

/// The owner-relative closure of a set of roots, every def reachable over
/// parent edges, the roots included.
#[derive(Debug, Default)]
pub struct DefClosure<'g> {
    nodes: BTreeMap<DefKey, DefEntry<'g>>,
    /// Every node's strict ancestor set, computed once on first use and shared
    /// by every [`DefClosure::survivors`] call, so a caller asking per key (one
    /// call per meta type) walks the closure once, not once per call.
    ancestors: OnceCell<BTreeMap<DefKey, BTreeSet<DefKey>>>,
}

/// A one-graph resolver, for a caller holding a single repo's graph: every repo
/// name resolves to it. A `::repo` edge then reads the same graph, so it suits
/// only a single-repo world (tb-core's own tests, a graph with no qualified
/// edges).
pub struct SingleGraph<'g>(pub &'g TypeGraph);

impl PeerGraphResolver for SingleGraph<'_> {
    fn graph_of(&self, _repo: &str) -> Option<&TypeGraph> {
        Some(self.0)
    }
}

impl<'g> DefClosure<'g> {
    /// Walk from `roots`, each a def read in its repo. A root or parent whose
    /// repo or def does not resolve contributes nothing, the gates and load
    /// checks own that diagnostic. Cycle-safe, each def is visited once.
    pub fn walk(
        resolver: &'g dyn PeerGraphResolver,
        roots: impl IntoIterator<Item = DefKey>,
    ) -> Self {
        let mut nodes: BTreeMap<DefKey, DefEntry<'g>> = BTreeMap::new();
        let mut work: Vec<DefKey> = roots.into_iter().collect();
        while let Some(key) = work.pop() {
            if nodes.contains_key(&key) {
                continue;
            }
            let Some(def) = resolver.graph_of(&key.repo).and_then(|g| g.get(&key.name)) else {
                continue;
            };
            let parents: Vec<DefKey> = def
                .parents
                .iter()
                .map(|p| DefKey {
                    name: p.name.clone(),
                    repo: p.repo.clone().unwrap_or_else(|| key.repo.clone()),
                })
                .collect();
            work.extend(parents.iter().cloned());
            nodes.insert(key, DefEntry { def, parents });
        }
        DefClosure {
            nodes,
            ancestors: OnceCell::new(),
        }
    }

    /// The closure of an authored claim made in `own_repo`: each claimed name
    /// read in its `::repo`, else in `own_repo`.
    pub fn of_claim(
        resolver: &'g dyn PeerGraphResolver,
        own_repo: &str,
        claim: &TypeClaim,
    ) -> Self {
        Self::walk(
            resolver,
            claim.iter().map(|c| DefKey {
                name: c.name.clone(),
                repo: c.repo.clone().unwrap_or_else(|| own_repo.to_string()),
            }),
        )
    }

    /// Every def in the closure, key-ordered.
    pub fn iter(&self) -> impl Iterator<Item = (&DefKey, &'g TypeDef)> + '_ {
        self.nodes.iter().map(|(k, e)| (k, e.def))
    }

    /// The def at `key`, when it is in the closure.
    pub fn get(&self, key: &DefKey) -> Option<&'g TypeDef> {
        self.nodes.get(key).map(|e| e.def)
    }

    /// True when no def in the closure satisfies `pred`, the cheap early exit
    /// before a shadowing pass.
    pub fn none(&self, pred: impl Fn(&TypeDef) -> bool) -> bool {
        !self.nodes.values().any(|e| pred(e.def))
    }

    /// The strict ancestors of `key` within the closure.
    pub fn ancestors_of(&self, key: &DefKey) -> BTreeSet<DefKey> {
        self.ancestor_sets().get(key).cloned().unwrap_or_default()
    }

    fn ancestor_sets(&self) -> &BTreeMap<DefKey, BTreeSet<DefKey>> {
        self.ancestors.get_or_init(|| {
            self.nodes
                .keys()
                .map(|k| (k.clone(), self.walk_ancestors(k)))
                .collect()
        })
    }

    fn walk_ancestors(&self, key: &DefKey) -> BTreeSet<DefKey> {
        let mut out: BTreeSet<DefKey> = BTreeSet::new();
        let mut stack: Vec<&DefKey> = self
            .nodes
            .get(key)
            .map(|e| e.parents.iter().collect())
            .unwrap_or_default();
        while let Some(k) = stack.pop() {
            if out.insert(k.clone()) {
                if let Some(e) = self.nodes.get(k) {
                    stack.extend(e.parents.iter());
                }
            }
        }
        out
    }

    /// The declarations that survive shadowing, key-ordered.
    ///
    /// `declares` picks the defs that declare the key in question, given each
    /// def's key since a declaration may be relative to its repo. A declaring
    /// def is shadowed when another declaring def is its strict descendant, one
    /// whose ancestors include it, so the survivors are the declarations no
    /// more specific declaration overrides. Order- and path-independent, the
    /// walk's edge order never enters.
    pub fn survivors(
        &self,
        declares: impl Fn(&DefKey, &TypeDef) -> bool,
    ) -> Vec<(&DefKey, &'g TypeDef)> {
        let declaring: Vec<&DefKey> = self
            .nodes
            .iter()
            .filter(|(k, e)| declares(k, e.def))
            .map(|(k, _)| k)
            .collect();
        let sets = self.ancestor_sets();
        declaring
            .iter()
            .filter(|x| {
                !declaring
                    .iter()
                    .any(|y| y != *x && sets.get(*y).is_some_and(|anc| anc.contains(**x)))
            })
            .map(|k| (*k, self.nodes[*k].def))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::build_graph;
    use crate::typedef::{TypeNameClaim, parse_type_def};
    use std::path::Path;
    use tb_diagnostics::ByteRange;

    fn graph(defs: &[(&str, &str)]) -> TypeGraph {
        let parsed = defs
            .iter()
            .map(|(path, src)| {
                let docs = tb_parser::yaml::parse(src).unwrap();
                parse_type_def(Path::new(path), src, 0, &docs[0])
                    .type_def
                    .unwrap()
            })
            .collect();
        build_graph(parsed).graph
    }

    struct Map(BTreeMap<&'static str, TypeGraph>);
    impl PeerGraphResolver for Map {
        fn graph_of(&self, repo: &str) -> Option<&TypeGraph> {
            self.0.get(repo)
        }
    }

    fn key(name: &str, repo: &str) -> DefKey {
        DefKey {
            name: TypeName(name.into()),
            repo: repo.into(),
        }
    }

    fn claim(name: &str, repo: Option<&str>) -> TypeClaim {
        let authored = match repo {
            Some(r) => format!("{name}::{r}"),
            None => name.to_string(),
        };
        TypeClaim::Bare(TypeNameClaim::parse(&authored, ByteRange::new(0, 0)))
    }

    fn keys(c: &DefClosure) -> Vec<DefKey> {
        c.iter().map(|(k, _)| k.clone()).collect()
    }

    fn has_location(_: &DefKey, td: &TypeDef) -> bool {
        td.location.is_some()
    }

    #[test]
    fn a_single_repo_walk_reads_every_ancestor_once_through_a_diamond() {
        let g = graph(&[
            ("/r/type/base.type.yaml", "fields: {}\n"),
            ("/r/type/a.type.yaml", "extends: base\n"),
            ("/r/type/b.type.yaml", "extends: base\n"),
            ("/r/type/leaf.type.yaml", "extends: [a, b]\n"),
        ]);
        let r = SingleGraph(&g);
        let c = DefClosure::of_claim(&r, "r", &claim("leaf", None));
        assert_eq!(
            keys(&c),
            vec![
                key("a", "r"),
                key("b", "r"),
                key("base", "r"),
                key("leaf", "r")
            ]
        );
        assert_eq!(
            c.ancestors_of(&key("leaf", "r")),
            [key("a", "r"), key("b", "r"), key("base", "r")].into()
        );
    }

    #[test]
    fn a_qualified_parent_is_read_in_its_repo_and_its_bare_parents_stay_there() {
        let app = graph(&[("/v/app/type/card.type.yaml", "extends: note::base\n")]);
        let base = graph(&[
            ("/v/base/type/root.type.yaml", "fields: {}\n"),
            ("/v/base/type/note.type.yaml", "extends: root\n"),
        ]);
        let r = Map([("app", app), ("base", base)].into());
        let c = DefClosure::of_claim(&r, "app", &claim("card", None));
        assert_eq!(
            keys(&c),
            vec![key("card", "app"), key("note", "base"), key("root", "base")]
        );
    }

    #[test]
    fn in_sync_twins_are_two_defs_each_carrying_its_own_block() {
        // One identity (the location is out of the hash), two defs, two blocks.
        let app = graph(&[
            (
                "/v/app/type/note.type.yaml",
                "fields:\n  t: String\nlocation:\n  path: \"notes/\"\n",
            ),
            (
                "/v/app/type/both.type.yaml",
                "extends: [note, note::base]\n",
            ),
        ]);
        let base = graph(&[(
            "/v/base/type/note.type.yaml",
            "fields:\n  t: String\nlocation:\n  path: \"elsewhere/\"\n",
        )]);
        let r = Map([("app", app), ("base", base)].into());

        let own = DefClosure::of_claim(&r, "app", &claim("note", None));
        let peer = DefClosure::of_claim(&r, "app", &claim("note", Some("base")));
        let path = |c: &DefClosure| {
            let s = c.survivors(has_location);
            assert_eq!(s.len(), 1);
            s[0].1
                .location
                .as_ref()
                .unwrap()
                .path
                .as_ref()
                .unwrap()
                .raw
                .clone()
        };
        assert_eq!(path(&own), "notes/");
        assert_eq!(path(&peer), "elsewhere/");

        // Both twins as parents: unrelated, so both survive.
        let both = DefClosure::of_claim(&r, "app", &claim("both", None));
        let s: Vec<DefKey> = both
            .survivors(has_location)
            .into_iter()
            .map(|(k, _)| k.clone())
            .collect();
        assert_eq!(s, vec![key("note", "app"), key("note", "base")]);
    }

    #[test]
    fn a_descendant_declaration_shadows_on_every_path() {
        // leaf extends [broken, other]; broken and other both extend base; only
        // base and broken declare. broken is base's strict descendant, so base
        // is shadowed even though other also reaches it.
        let g = graph(&[
            ("/r/type/base.type.yaml", "location:\n  path: \"a/\"\n"),
            ("/r/type/broken.type.yaml", "extends: base\nlocation: {}\n"),
            ("/r/type/other.type.yaml", "extends: base\n"),
            ("/r/type/leaf.type.yaml", "extends: [broken, other]\n"),
        ]);
        let r = SingleGraph(&g);
        let c = DefClosure::of_claim(&r, "r", &claim("leaf", None));
        let s: Vec<DefKey> = c
            .survivors(has_location)
            .into_iter()
            .map(|(k, _)| k.clone())
            .collect();
        assert_eq!(s, vec![key("broken", "r")]);
    }

    #[test]
    fn a_nearer_declaration_on_a_sibling_branch_does_not_shadow_a_deeper_one() {
        // leaf extends [a, b]; a -> x -> y declares, b declares. Neither is the
        // other's descendant, so depth and listing order are irrelevant: both.
        let g = graph(&[
            ("/r/type/y.type.yaml", "location:\n  path: \"y/\"\n"),
            ("/r/type/x.type.yaml", "extends: y\n"),
            ("/r/type/a.type.yaml", "extends: x\n"),
            ("/r/type/b.type.yaml", "location:\n  path: \"b/\"\n"),
            ("/r/type/leaf.type.yaml", "extends: [a, b]\n"),
        ]);
        let r = SingleGraph(&g);
        let c = DefClosure::of_claim(&r, "r", &claim("leaf", None));
        let s: Vec<DefKey> = c
            .survivors(has_location)
            .into_iter()
            .map(|(k, _)| k.clone())
            .collect();
        assert_eq!(s, vec![key("b", "r"), key("y", "r")]);
    }

    #[test]
    fn an_unresolvable_root_or_parent_contributes_nothing() {
        let g = graph(&[("/r/type/leaf.type.yaml", "extends: [ghost, far::nowhere]\n")]);
        let r = Map([("r", g)].into());
        let c = DefClosure::of_claim(&r, "r", &claim("leaf", None));
        assert_eq!(keys(&c), vec![key("leaf", "r")]);
        assert!(
            DefClosure::of_claim(&r, "r", &claim("ghost", None))
                .iter()
                .next()
                .is_none()
        );
    }
}
