//! The mutation gate's pure reduction.
//!
//! `policy::MUTATION_STAGES` and `policy::CREATE_STAGES` say *in what order* a mutation is
//! checked. This module says *what each check answers* when it needs nothing but the caller's
//! input, the launch policy and what this process has served. The stages that need the engine
//! (`editability`, `absence`) and the stage that needs a resource (`capacity`) stay in the
//! handler layer: they are dispatch, and a reduction that performs IO cannot be tested without
//! a socket.
//!
//! Splitting them buys exactly one thing, and it is the thing the plan asked for: the ordering
//! and the answers become assertable as data, so a change to a message or an order shows up in a
//! test instead of in a client's retry loop.

use crate::policy::{Denial, Stage};

/// What the reduction may look at. Everything here is already in hand when the gate runs, so a
/// test builds one directly.
pub(crate) struct Input<'a> {
    pub repo: &'a str,
    pub path: &'a str,
    /// The hash the caller says it is writing over; empty for a create.
    pub expected_hash: &'a str,
    /// What the read ledger holds for `(repo, path)`, if anything.
    pub recorded: Option<String>,
    /// Whether the launch policy permits mutations in this member.
    pub permits_member: bool,
}

/// The progress a pure stage can record for the stages that follow it.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Progress {
    /// The hash the ledger served, which `freshness` compares against.
    pub recorded: Option<String>,
}

/// One pure stage's answer: deny with a stage-tagged reason, or carry progress forward.
pub(crate) fn reduce(stages: &[Stage], input: &Input<'_>) -> Result<Progress, Denial> {
    let mut progress = Progress::default();
    for stage in stages {
        match stage {
            Stage::Arguments => {
                if input.repo.is_empty() {
                    return Err(Denial::new(Stage::Arguments, "'repo' must not be empty"));
                }
                if input.path.is_empty() {
                    return Err(Denial::new(Stage::Arguments, "'path' must not be empty"));
                }
            }
            Stage::PathScope => {
                if let Err(error) = crate::relative_path(input.path) {
                    return Err(Denial::new(Stage::PathScope, error));
                }
            }
            Stage::MemberGrant => {
                if !input.permits_member {
                    return Err(Denial::new(
                        Stage::MemberGrant,
                        format!(
                            "mutations are not allowed in workspace member {}",
                            input.repo
                        ),
                    ));
                }
            }
            Stage::PriorRead => {
                let Some(recorded) = input.recorded.clone() else {
                    return Err(Denial::new(
                        Stage::PriorRead,
                        "read this file with tb_content before writing it",
                    ));
                };
                progress.recorded = Some(recorded);
            }
            Stage::Freshness => {
                if progress.recorded.as_deref() != Some(input.expected_hash) {
                    return Err(Denial::new(
                        Stage::Freshness,
                        "'expected_hash' does not match the hash returned by the last tb_content read",
                    ));
                }
            }
            // Dispatch, not reduction: the engine answers whether it can author the member,
            // whether the path is absent, and whether a slot is free.
            Stage::Editability | Stage::Absence | Stage::Capacity => {}
        }
    }
    Ok(progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{CREATE_STAGES, MUTATION_STAGES};

    fn input<'a>(recorded: Option<&str>, permits_member: bool) -> Input<'a> {
        Input {
            repo: "vault",
            path: "inbox/a.md",
            expected_hash: "abc",
            recorded: recorded.map(str::to_owned),
            permits_member,
        }
    }

    #[test]
    fn a_satisfied_mutation_gate_passes_and_carries_the_served_hash() {
        let progress = reduce(&MUTATION_STAGES, &input(Some("abc"), true)).expect("passes");
        assert_eq!(progress.recorded.as_deref(), Some("abc"));
    }

    #[test]
    fn an_unread_file_is_prior_read_not_freshness() {
        let denial = reduce(&MUTATION_STAGES, &input(None, true)).unwrap_err();
        assert_eq!(denial.stage, Stage::PriorRead);
        assert!(denial.text().starts_with("policy/prior_read: "));
    }

    #[test]
    fn a_stale_hash_is_freshness_not_prior_read() {
        let denial = reduce(&MUTATION_STAGES, &input(Some("older"), true)).unwrap_err();
        assert_eq!(
            denial.stage,
            Stage::Freshness,
            "the two refusals must stay distinguishable: {denial:?}"
        );
    }

    #[test]
    fn a_member_without_the_grant_is_refused_before_the_read_check() {
        // The order is the contract: a caller without the grant learns the grant is missing,
        // not that it forgot to read.
        let denial = reduce(&MUTATION_STAGES, &input(None, false)).unwrap_err();
        assert_eq!(denial.stage, Stage::MemberGrant);
    }

    #[test]
    fn the_create_gate_does_not_ask_for_a_prior_read() {
        let progress = reduce(&CREATE_STAGES, &input(None, true)).expect("a create needs no read");
        assert_eq!(progress.recorded, None);
    }

    #[test]
    fn empty_arguments_are_named_before_anything_else() {
        let mut empty = input(Some("abc"), true);
        empty.repo = "";
        let denial = reduce(&MUTATION_STAGES, &empty).unwrap_err();
        assert_eq!(denial.stage, Stage::Arguments);
        assert!(denial.text().contains("'repo' must not be empty"));
    }

    #[test]
    fn a_path_that_escapes_the_member_is_refused_at_its_stage() {
        for path in ["../outside.md", "/absolute.md"] {
            let mut escaping = input(Some("abc"), true);
            escaping.path = path;
            let denial = reduce(&MUTATION_STAGES, &escaping).unwrap_err();
            assert_eq!(denial.stage, Stage::PathScope, "{path}");
        }
    }
}
