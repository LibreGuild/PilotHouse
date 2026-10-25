//! Templates: what an agent is asked to do with a pull request, and how its
//! answer should look. PilotHouse ships some (review, security review,
//! explain, test plan); the user can edit them, put them back as shipped,
//! and add their own (Settings → Templates).
//!
//! - A **review** template's text is the reviewing guide: what to look for,
//!   what each severity means, what counts as evidence. PilotHouse adds the
//!   pull request, the repository's own rules (`.pilothouse/review.md` on the
//!   base branch) and the fixed answer format its findings card reads, so a
//!   template can change how a review is done but never break the card.
//! - A **prompt** template's text is a request about the pull request
//!   (explain it, plan its testing); the answer is Markdown, with Mermaid
//!   diagrams where they help.
//!
//! Stored in the `review.templates` setting: the user's own, and edited
//! copies of the shipped ones by their id.

use anyhow::{Result, bail};
use ph_store::Store;
use serde::{Deserialize, Serialize};

const KEY: &str = "review.templates";

/// What a template makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateKind {
    /// A review with findings (the findings card).
    Review,
    /// A free answer about the pull request.
    Prompt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    /// Stable: the shipped ones' ids are fixed, the user's are made from
    /// the name.
    pub id: String,
    pub name: String,
    /// One line, shown under the name.
    pub description: String,
    pub kind: TemplateKind,
    /// The guide (review) or the request (prompt).
    pub instructions: String,
}

/// The shipped templates' ids, in the order they are listed.
pub const REVIEW: &str = "review";
pub const SECURITY: &str = "security";
pub const EXPLAIN: &str = "explain";
pub const TEST_PLAN: &str = "test-plan";

/// What is stored: the user's templates and edited shipped ones.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    templates: Vec<Template>,
    /// Shipped templates the user removed from the list.
    hidden: Vec<String>,
}

/// The templates as shipped.
pub fn builtin() -> Vec<Template> {
    vec![
        Template {
            id: REVIEW.into(),
            name: "Review this PR".into(),
            description: "Bugs, risks and gaps, each with evidence and a fix".into(),
            kind: TemplateKind::Review,
            instructions: REVIEW_GUIDE.into(),
        },
        Template {
            id: SECURITY.into(),
            name: "Security review".into(),
            description: "Injection, auth, secrets, unsafe input and dependencies".into(),
            kind: TemplateKind::Review,
            instructions: SECURITY_GUIDE.into(),
        },
        Template {
            id: EXPLAIN.into(),
            name: "Explain this PR".into(),
            description: "What it changes and how, with diagrams".into(),
            kind: TemplateKind::Prompt,
            instructions: EXPLAIN_REQUEST.into(),
        },
        Template {
            id: TEST_PLAN.into(),
            name: "Test plan".into(),
            description: "How to check it by hand, and what tests are missing".into(),
            kind: TemplateKind::Prompt,
            instructions: TEST_PLAN_REQUEST.into(),
        },
    ]
}

/// Whether `id` is one PilotHouse ships.
pub fn is_builtin(id: &str) -> bool {
    builtin().iter().any(|template| template.id == id)
}

/// Every template, in order: the shipped ones (as edited), then the user's.
pub fn all(store: &Store) -> Vec<Template> {
    let saved: Saved = store.setting(KEY);
    let mut list: Vec<Template> = builtin()
        .into_iter()
        .filter(|template| !saved.hidden.contains(&template.id))
        .map(|shipped| {
            saved
                .templates
                .iter()
                .find(|edited| edited.id == shipped.id)
                .cloned()
                .unwrap_or(shipped)
        })
        .collect();
    list.extend(
        saved
            .templates
            .iter()
            .filter(|template| !is_builtin(&template.id))
            .cloned(),
    );
    list
}

/// Template `id`, as the user has it (falls back to the shipped review).
pub fn get(store: &Store, id: &str) -> Option<Template> {
    all(store).into_iter().find(|template| template.id == id)
}

/// The template a plain "Review with agent" uses.
pub fn default_review(store: &Store) -> Template {
    get(store, REVIEW)
        .or_else(|| {
            all(store)
                .into_iter()
                .find(|template| template.kind == TemplateKind::Review)
        })
        .unwrap_or_else(|| builtin().remove(0))
}

/// Saves `template` (new, or an edit of one listed).
pub fn save(store: &Store, template: Template) -> Result<()> {
    if template.name.trim().is_empty() {
        bail!("A template needs a name.");
    }
    if template.instructions.trim().is_empty() {
        bail!("A template needs its instructions.");
    }
    let mut saved: Saved = store.setting(KEY);
    saved.hidden.retain(|id| *id != template.id);
    match saved
        .templates
        .iter_mut()
        .find(|known| known.id == template.id)
    {
        Some(known) => *known = template,
        None => saved.templates.push(template),
    }
    store.set_setting(KEY, &saved)
}

/// A new template's id, from its name (unique among those listed).
pub fn new_id(store: &Store, name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let base = if slug.is_empty() {
        "template".to_string()
    } else {
        slug
    };
    let taken: Vec<String> = all(store).into_iter().map(|template| template.id).collect();
    let mut id = format!("custom-{base}");
    let mut n = 2;
    while taken.contains(&id) || is_builtin(&id) {
        id = format!("custom-{base}-{n}");
        n += 1;
    }
    id
}

/// Puts a shipped template back as shipped.
pub fn reset(store: &Store, id: &str) -> Result<()> {
    let mut saved: Saved = store.setting(KEY);
    saved.templates.retain(|template| template.id != id);
    saved.hidden.retain(|hidden| hidden != id);
    store.set_setting(KEY, &saved)
}

/// Removes a template: the user's is deleted, a shipped one hidden.
pub fn remove(store: &Store, id: &str) -> Result<()> {
    let mut saved: Saved = store.setting(KEY);
    saved.templates.retain(|template| template.id != id);
    if is_builtin(id) && !saved.hidden.iter().any(|hidden| hidden == id) {
        saved.hidden.push(id.to_string());
    }
    store.set_setting(KEY, &saved)
}

/// Whether the shipped template `id` was edited.
pub fn is_edited(store: &Store, id: &str) -> bool {
    let saved: Saved = store.setting(KEY);
    is_builtin(id)
        && saved.templates.iter().any(|template| {
            template.id == id && builtin().iter().all(|shipped| shipped != template)
        })
}

/// Where a repository keeps its own review rules, read from the base branch
/// (never the pull request's head, which its author controls).
pub const TEAM_RULES: &str = ".pilothouse/review.md";

const REVIEW_GUIDE: &str = "\
Review the change as a careful senior engineer on this team would, against the whole codebase and not just the diff: read the callers of what changed, the tests, the configuration and the documentation it touches.

Look for, most serious first:
- Correctness: wrong behavior, broken edge cases (empty, null, very large, concurrent, unicode, time zones), off-by-one errors, wrong error handling, resource leaks.
- Data: loss or corruption, migrations that cannot be rolled back, writes that are not atomic, races.
- Security: injection, authentication and authorization gaps, secrets in code or logs, unsafe input from users or the network.
- Compatibility: changed APIs, file formats, configuration or behavior that callers or users rely on.
- Performance: work in hot paths, repeated queries, unbounded growth, blocking calls where they hurt.
- Tests: behavior the change adds or fixes without a test, and tests that would pass whatever the code did.
- Clarity, only when it will cause mistakes: misleading names, dead code, duplicated logic.

Severity:
- critical: must not merge. Data loss, a security hole, a crash or outage on a common path, a broken build.
- major: should be fixed before merging. Wrong behavior users will meet, a regression, a missing test for risky logic.
- minor: worth fixing, not blocking. An uncommon edge case, weak error messages, small performance waste.
- nit: polish. Naming, comments, small simplifications. Keep these few.

Every finding needs evidence: the code that shows it (quote the lines), the inputs or call path that trigger it, or the test that would fail. If you cannot show it, leave it out or ask it as a question in the summary. Do not report what you only suspect, and do not repeat one problem as several findings.

Every finding needs a fix: concrete enough to apply, as a short code snippet when that is clearest.

Also say in the summary what the change does well, whether it does what its description says, and anything a reviewer should check by hand.";

const SECURITY_GUIDE: &str = "\
Review the change for security only, as an application security engineer would. Follow untrusted data (request bodies, query strings, headers, files, environment, messages from other services, pull request text) from where it enters to where it is used.

Look for:
- Injection: SQL, shell commands, paths (traversal), templates, HTML and JavaScript (XSS), regular expressions that can backtrack for ever, deserialization of untrusted data.
- Authentication and authorization: missing or wrong checks, checks on the client only, objects reachable by id without an owner check, privilege changes.
- Secrets: keys, tokens or passwords in code, tests, logs, error messages or URLs; weak or home-made cryptography; predictable random values used for security.
- Network and files: requests to addresses a user controls (SSRF), TLS checks turned off, files written with loose permissions, temporary files that can be swapped.
- Dependencies: new or updated packages, their versions, and scripts they run on install.
- Denial of service: unbounded reads, allocations or loops driven by input.

Severity:
- critical: exploitable now by someone outside the trust boundary, or a secret exposed.
- major: exploitable with some access or preconditions, or a missing check on a sensitive action.
- minor: defense in depth missing, or risky code not reachable today.
- nit: hardening suggestions.

Every finding needs evidence: the path from input to the dangerous use, with the lines quoted, and how it would be exploited. Every finding needs a fix: the check, the escaping or the API to use instead. Do not report issues outside the change unless the change makes them reachable.";

const EXPLAIN_REQUEST: &str = "\
Explain this pull request to a teammate who has not seen it, so they can review it with confidence. Read the diff and the code around it first.

Answer in Markdown, in this order:

## In short
Two or three sentences: what changes, and why (from the description, the commits and the code).

## How it works
The flow of the change. Draw it with a Mermaid diagram: a `flowchart` for how parts connect or how data moves, a `sequenceDiagram` for a request or an event going through several components, a `classDiagram` or `erDiagram` for changed types or tables, a `stateDiagram-v2` for changed states. Use one or two diagrams that make it clearer, not more. Write each as a fenced code block marked `mermaid`, keep node labels short and avoid characters that need escaping.

## What changed, file by file
A table with each changed file (or group of files), what changed in it and why. Group small related files.

## Things to look at
What deserves a careful look in review: risky logic, behavior changes for users or callers, migrations, configuration, missing tests. Point at files and lines.

## How to try it
The steps to see the change working.

Keep it concrete: name functions, types and files, and quote code only when it explains something.";

const TEST_PLAN_REQUEST: &str = "\
Write a test plan for this pull request. Read the change and the code around it first, and find the existing tests that cover it.

Answer in Markdown:

## What to check by hand
Numbered steps a reviewer can follow to see the change work: setup, actions, and what they should see. Include the edge cases most likely to break (empty input, errors, permissions, large data, concurrency) and how to try each.

## Automated tests
A table: the behavior, whether a test covers it now (name the test), and what test to add when none does. Put the riskiest gaps first.

## Regressions to watch
What existing behavior could break, and how to check it still works.

If a diagram makes the scenarios clearer (a state machine, a flow with branches), add one as a fenced `mermaid` code block.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_templates_can_be_edited_reset_and_hidden() {
        let store = Store::open_in_memory().unwrap();
        let ids: Vec<String> = all(&store).into_iter().map(|t| t.id).collect();
        assert_eq!(ids, [REVIEW, SECURITY, EXPLAIN, TEST_PLAN]);

        let mut review = get(&store, REVIEW).unwrap();
        review.instructions = "Check the CHANGELOG is updated.".into();
        save(&store, review).unwrap();
        assert!(is_edited(&store, REVIEW));
        assert_eq!(
            default_review(&store).instructions,
            "Check the CHANGELOG is updated."
        );
        reset(&store, REVIEW).unwrap();
        assert!(!is_edited(&store, REVIEW));
        assert_eq!(default_review(&store).instructions, REVIEW_GUIDE);

        remove(&store, TEST_PLAN).unwrap();
        assert!(get(&store, TEST_PLAN).is_none());
        reset(&store, TEST_PLAN).unwrap();
        assert!(get(&store, TEST_PLAN).is_some());
    }

    #[test]
    fn the_users_own_templates_come_after() {
        let store = Store::open_in_memory().unwrap();
        let id = new_id(&store, "Release checklist!");
        assert_eq!(id, "custom-release-checklist");
        save(
            &store,
            Template {
                id: id.clone(),
                name: "Release checklist".into(),
                description: String::new(),
                kind: TemplateKind::Prompt,
                instructions: "Is the version bumped?".into(),
            },
        )
        .unwrap();
        assert_eq!(all(&store).last().unwrap().id, id);
        assert_eq!(
            new_id(&store, "Release checklist"),
            "custom-release-checklist-2"
        );
        remove(&store, &id).unwrap();
        assert!(get(&store, &id).is_none());
        // Names and instructions are needed.
        let empty = Template {
            id: "custom-x".into(),
            name: " ".into(),
            description: String::new(),
            kind: TemplateKind::Prompt,
            instructions: "x".into(),
        };
        assert!(save(&store, empty).is_err());
    }
}
