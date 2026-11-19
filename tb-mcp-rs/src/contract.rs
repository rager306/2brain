//! The answer contract, served as MCP prompts and workflow resources.
//!
//! `tb_search` returns ranked hits and `tb_neighborhood` returns a subgraph; both are
//! inputs to an answer, and nothing in the tool surface is under contract to produce one
//! (`docs/research/arscontexta-skills-2026-11-16.md`, the `ask` skill comparison). The
//! contract is text, and MCP carries text without growing the tool catalogue: three
//! `prompts/get` workflows and two static `2brain://workflow/<name>` resources.
//!
//! This is deliberately not `resources.rs`'s business. That module owns the URI scheme
//! and the member table for engine-backed files, where every read is an engine round
//! trip with a hash. The text here is static: no engine, no IO, no member, and its
//! failure mode - a missing entry - is a defect the tests catch, not a read error.

use rmcp::ErrorData;
use rmcp::model::{
    GetPromptResult, JsonObject, Prompt, PromptArgument, PromptMessage, Resource, Role,
};
use serde_json::Value;

/// What a client should know before choosing a tool - the instructions string
/// `initialize` returns, and the same genus as the prompts below: static text
/// the server serves, kept here so every client-facing word lives in one place.
pub(crate) const INSTRUCTIONS: &str = "Read-only access to an 2brain workspace, plus gated mutations. Every result is the engine's own result object under the read's key: tb_overview answers {\"overview\": ...}, tb_type answers {\"type\": ...}. Start with tb_overview, then tb_members for member roots, tb_types and tb_type for vocabulary, and tb_files or tb_content to locate text. Paged tools take limit and offset; tb_type_tree, tb_content, tb_instances_of, and tb_validate_value are unpaged and can return large answers. Mutations are disabled unless the operator granted them at launch; when enabled they need a prior tb_content read of the same path and the hash it returned. A lost reply is outcome_unknown and must not be retried blindly; tb_reconcile reports whether it landed.";

/// One workflow: a named template with exactly one argument substituted at read time.
#[derive(Debug)]
struct Workflow {
    name: &'static str,
    description: &'static str,
    argument: &'static str,
    argument_description: &'static str,
    placeholder: &'static str,
    template: &'static str,
}

/// The three workflows, in `prompts/list` order: retrieval, then intake, then handoff.
const WORKFLOWS: [Workflow; 3] = [
    Workflow {
        name: "recall",
        description: "The retrieval workflow: orient, search, widen, read whole, answer in the six-part shape with citations by path, and say plainly when the base does not know.",
        argument: "question",
        argument_description: "The question to answer from the workspace.",
        placeholder: "{question}",
        template: RECALL,
    },
    Workflow {
        name: "intake",
        description: "The write workflow: find the claim first, pick the type honestly, write with a description that says why it matters, link it into a family, confirm with tb_diagnostics.",
        argument: "record",
        argument_description: "What to record: the claim, decision, or observation, in the writer's words.",
        placeholder: "{record}",
        template: INTAKE,
    },
    Workflow {
        name: "handoff",
        description: "The session-end workflow: decisions, evidence by path, what stays unverified, and one concrete next action, written for a reader with none of this session's context.",
        argument: "state",
        argument_description: "Where the work stands: decisions, evidence, and open threads, compressed or pasted.",
        placeholder: "{state}",
        template: HANDOFF,
    },
];

const RECALL: &str = "\
Answer this question from the workspace: {question}

Work the steps in order. Each step names the tool that does it.

1. Orient on the base before searching. tb_overview says what the workspace holds; \
tb_members names the members and their roots; tb_types lists the vocabulary the notes \
use. Skip this only if you already hold that vocabulary from earlier in the session.
2. Search before you read. tb_search ranks notes against the question's words; tb_grep \
answers \"which notes contain this exact phrase\" when the words are a quote, a name, or \
an id. Read the hit descriptions, not only the paths.
3. Widen when the page is ambiguous. tb_neighborhood walks the reference graph around \
the strongest hit; tb_references lists one file's incoming and outgoing edges; \
tb_follow resolves a wikilink before you trust it. One widening step is usual; if the \
note the answer lives in is still not named after two, say so in the answer rather \
than guessing.
4. Read the note the answer lives in. tb_content on it, whole, before quoting. An \
answer built from search excerpts alone is an answer about excerpts, not about the note.
5. Answer in the six-part shape (2brain://workflow/answer-shape): the direct answer \
first; backing cited by member and path, quoting only what you read; implications in \
the reader's own vocabulary; tensions where notes disagree, never averaged into false \
balance; what to explore next; and sources consulted with counts.
6. State it explicitly when the base does not know. Name the nearest note you did find \
and stop there. Do not fabricate coverage, and label an unverified hunch as one.

A deep answer that weaves four notes beats a shallow one that lists ten.";

const INTAKE: &str = "\
Record this into the workspace: {record}

1. Check for an existing note on the same claim before writing. tb_search with the \
claim's own words; tb_grep for the exact phrase; tb_frontmatter on the near hits, where \
a typed instance carries its claim under `type` and a plain note does not. A duplicate \
is not an observation: when a note already holds the claim, extend it or link to it, \
and say so in your answer.
2. Pick the type honestly. tb_types and tb_type give the workspace's vocabulary; \
tb_candidates names files that already satisfy a type's fields without claiming it. \
When no type fits, write a plain note and say so - do not bend the claim into the \
nearest-looking type.
3. Write it. tb_create_file for a new path (it refuses a taken path rather than \
overwriting); tb_edit_file or tb_write_file for an existing one, which first need a \
tb_content read of the same path and the hash it returned as expected_hash. The \
description says why the record matters, not that it exists. Every refusal names its \
stage; 2brain://workflow/write-gates lists them and what clears each.
4. Link it into a family. Address the family note and its siblings with [[wiki-links]]; \
tb_follow resolves a link before you write it, tb_references shows the family's edges \
afterwards. A note nothing links to is an orphan the graph cannot route a reader to.
5. Confirm with tb_diagnostics scoped to the changed file. An empty page is the exit \
condition; report a diagnostic you cannot clear instead of hiding it.";

const HANDOFF: &str = "\
Write the handoff. Where the work stands: {state}

The reader is another agent with none of this session's context, the same tool \
surface, and one chance to read what you write.

1. What was decided. Each decision in one line a stranger can act on.
2. What the evidence was. The member and path of every note a decision rests on, so \
the reader can re-open it with tb_content instead of trusting this summary.
3. What is unverified. Open questions, write-gate refusals, outcome_unknown mutations \
still owed a tb_reconcile, and diagnostics left standing.
4. The next action. One concrete step, with its tool, its member, and its path.

Every line that matters names a file or a tool; a line that names neither is a wish, \
not a handoff.";

/// The two static resources: `(name, title, intent, text)`.
const WORKFLOW_RESOURCES: [(&str, &str, &str, &str); 2] = [
    (
        "answer-shape",
        "Answer shape",
        "The six-part answer contract and its honesty standard",
        ANSWER_SHAPE,
    ),
    (
        "write-gates",
        "Write gates",
        "What the write gate refuses, stage by stage, and what clears each stage",
        WRITE_GATES,
    ),
];

const ANSWER_SHAPE: &str = "\
# Answer shape

The tools return inputs to an answer; nothing here produces one for you. An answer to \
a question about this workspace carries six parts, in this order.

1. **Direct answer first.** Say what the answer is, not what you searched. \"The hub \
of the intake family is X\" - never \"I searched and found three notes\".
2. **Backing, cited.** Every claim names the note it came from, by member and path, \
and quotes only what a tb_content read actually returned.
3. **Implications in the reader's vocabulary.** Translate; a reader should not need \
the graph's type names to act on the answer.
4. **Tensions.** Where notes disagree, present the disagreement. Do not average two \
positions into a balance neither holds.
5. **What to explore next.** Departure points that exist in the base - a note, a type, \
a family - not a wish list.
6. **Sources consulted.** How many notes were read, how many candidates were \
rejected, and why the winner won.

## Honesty

When the workspace lacks coverage, say the workspace lacks coverage, name the nearest \
adjacent note, and stop. Do not assemble a plausible-looking answer from tangential \
notes. An unverified hunch is labelled as one.

Read fewer sources more deeply: a shallow answer citing ten notes is worse than a \
deep answer that weaves four.";

const WRITE_GATES: &str = "\
# Write gates

The server is fail-closed. Without a launch grant every mutating tool is refused \
before the engine is contacted, and every refusal names its stage as `policy/<stage>`. \
No later stage runs once an earlier one denies, so the first refusal is the whole \
diagnosis.

## The grants, decided at launch

- `allow_mutations` - the 16 mutating tools exist at all. Without it they are absent \
from `tools/list` and a call is refused by launch policy.
- `allow_composition` - `tb_add_dep` and `tb_remove_dep`, on top of mutations: \
changing what the workspace depends on is a second trust decision.
- `allow_workspace_shape` - `tb_add_workspace_member`, `tb_remove_workspace_member`, \
and `tb_set_workspace_member_role`, on top of mutations: changing which members are \
served changes what every later read sees.
- member list - a policy file may name the members a mutation may target.

## A write to an existing file passes, in order

1. `arguments` - an empty `repo` or `path` is refused.
2. `path_scope` - the path must stay inside the member: no absolute paths, no `..`.
3. `member_grant` - \"mutations are not allowed in workspace member X\": this launch \
does not hold the grant for that member.
4. `prior_read` - \"read this file with tb_content before writing it\": a write is \
based on a read this server served, so no caller writes a file it has not seen.
5. `freshness` - \"`expected_hash` does not match the hash returned by the last \
tb_content read\": the file must not have changed since the read the write builds on.
6. `editability` - the engine answers whether it authors this member.
7. `capacity` - one mutation runs at a time.

## A create passes the same order, with absence in place of the read pair

`absence` - the path must not exist: \"file already exists; use tb_write_file to \
change it\". A create has no prior text to compare against, so the guarantee is the \
opposite one: the path must be free.

## After the write

Diagnostics are advisory, and tb_diagnostics scoped to the changed file is how a \
write is confirmed. A reply lost in flight is `outcome_unknown` and must not be \
retried blindly: tb_reconcile reports what actually happened. Every mutation decision \
is recorded to the launch's audit trail.";

/// The catalogue `prompts/list` advertises. Static, in `WORKFLOWS` order.
pub(crate) fn prompts() -> Vec<Prompt> {
    WORKFLOWS
        .iter()
        .map(|workflow| {
            Prompt::new(
                workflow.name,
                Some(workflow.description),
                Some(vec![
                    PromptArgument::new(workflow.argument)
                        .with_description(workflow.argument_description)
                        .with_required(true),
                ]),
            )
        })
        .collect()
}

/// One workflow with the caller's argument substituted into its template.
///
/// A missing required argument and an unknown prompt name are refusals that name what
/// would have worked; extra arguments are ignored, because refusing them would fail a
/// client the protocol lets send more than the template needs.
pub(crate) fn prompt(
    name: &str,
    arguments: Option<&JsonObject>,
) -> Result<GetPromptResult, ErrorData> {
    let workflow = WORKFLOWS.iter().find(|w| w.name == name).ok_or_else(|| {
        ErrorData::invalid_params(
            format!("unknown prompt '{name}'; prompts/list names the three workflows"),
            None,
        )
    })?;
    let value = arguments
        .and_then(|arguments| arguments.get(workflow.argument))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ErrorData::invalid_params(
                format!(
                    "prompt '{}' requires the '{}' argument",
                    name, workflow.argument
                ),
                None,
            )
        })?;
    let message = PromptMessage::new_text(
        Role::User,
        workflow.template.replace(workflow.placeholder, value),
    );
    Ok(GetPromptResult::new(vec![message]).with_description(workflow.description))
}

/// The workflow resources `resources/list` carries ahead of the catalogue.
pub(crate) fn workflow_resources() -> Vec<Resource> {
    WORKFLOW_RESOURCES
        .iter()
        .map(|(name, title, intent, _text)| {
            Resource::new(format!("2brain://workflow/{name}"), *title)
                .with_mime_type("text/markdown")
                .with_description(*intent)
        })
        .collect()
}

/// The text of a `2brain://workflow/<name>` read, without contacting the engine.
///
/// `None` when the URI is not in the workflow namespace at all, so the caller falls
/// through to the member-backed read; `Some(Err(..))` when it is a workflow URI but
/// names nothing served. The scheme is `2brain://` only (ADR-0005).
pub(crate) fn workflow_resource(uri: &str) -> Option<Result<&'static str, ErrorData>> {
    let rest = uri.strip_prefix("2brain://workflow/")?;
    if rest.is_empty() {
        return Some(Err(ErrorData::invalid_params(
            "a workflow resource needs a name; resources/list carries the two served",
            None,
        )));
    }
    Some(
        WORKFLOW_RESOURCES
            .iter()
            .find(|(name, ..)| *name == rest)
            .map(|(_, _, _, text)| *text)
            .ok_or_else(|| {
                ErrorData::resource_not_found(
                    format!("no workflow resource '{rest}'; resources/list carries the two served"),
                    None,
                )
            }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_workflow_substitutes_its_one_argument_and_leaves_no_placeholder() {
        for workflow in &WORKFLOWS {
            let arguments = serde_json::from_str::<JsonObject>(&format!(
                "{{\"{}\": \"SAMPLE\"}}",
                workflow.argument
            ))
            .unwrap();
            let result = prompt(workflow.name, Some(&arguments)).unwrap();
            assert!(result.messages[0].role == Role::User);
            let rmcp::model::ContentBlock::Text(text) = &result.messages[0].content else {
                panic!(
                    "a workflow answer is text: {:?}",
                    result.messages[0].content
                );
            };
            assert!(text.text.contains("SAMPLE"), "{workflow:?}");
            assert!(!text.text.contains(workflow.placeholder), "{workflow:?}");
        }
    }

    #[test]
    fn a_missing_argument_and_an_unknown_name_are_refusals_that_name_the_way_out() {
        let error = prompt("recall", None).unwrap_err();
        assert!(error.message.contains("'question'"), "{}", error.message);
        let error = prompt("no-such-workflow", None).unwrap_err();
        assert!(error.message.contains("prompts/list"), "{}", error.message);
    }

    #[test]
    fn the_workflow_resources_are_reachable_by_the_one_scheme_and_unknown_names_refuse() {
        for (name, _, _, expected) in &WORKFLOW_RESOURCES {
            let served = workflow_resource(&format!("2brain://workflow/{name}")).unwrap();
            assert_eq!(served.unwrap(), *expected);
        }
        // The retired scheme is not a resource any more.
        let old = concat!("arsum", "bris://workflow/answer-shape");
        assert!(workflow_resource(old).is_none());
        assert!(
            workflow_resource("2brain://workflow/nothing")
                .unwrap()
                .is_err()
        );
        assert!(workflow_resource("2brain://workflow/").unwrap().is_err());
        assert!(workflow_resource("2brain://vault/inbox/a.md").is_none());
        assert!(workflow_resource("file:///etc/passwd").is_none());
    }

    #[test]
    fn the_catalogue_names_three_workflows_with_one_required_argument_each() {
        let listed = prompts();
        assert_eq!(listed.len(), 3);
        for (workflow, prompt) in WORKFLOWS.iter().zip(&listed) {
            assert_eq!(prompt.name, workflow.name);
            let arguments = prompt.arguments.as_deref().unwrap();
            assert_eq!(arguments.len(), 1);
            assert_eq!(arguments[0].required, Some(true));
            assert_eq!(arguments[0].name, workflow.argument);
        }
    }
}
