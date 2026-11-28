//! Shared pure helpers for projecting [`IndexedFile`] metadata into BM25 fields,
//! type-aware [`FreshnessMeta`], and hybrid document text (single source of truth across
//! `tb-fs-git`, `tb-app::enrich`, and `two-brain`).

use std::collections::BTreeMap;

use tb_search::FreshnessMeta;
use tb_syntax::FrontmatterValue;

use crate::IndexedFile;

/// Extract the first non-empty string field matching any key in `keys`.
#[must_use]
pub fn frontmatter_str<'a>(
    fm: &'a BTreeMap<String, FrontmatterValue>,
    keys: &[&str],
) -> Option<&'a str> {
    for &k in keys {
        if let Some(FrontmatterValue::String(s)) = fm.get(k) {
            let t = s.trim();
            if !t.is_empty() {
                return Some(t);
            }
        }
    }
    None
}

/// Collect all non-empty string values (scalar or sequence elements) for the given frontmatter keys.
#[must_use]
pub fn collect_frontmatter_strings(
    fm: Option<&BTreeMap<String, FrontmatterValue>>,
    keys: &[&str],
) -> Vec<String> {
    let Some(fm) = fm else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for key in keys {
        match fm.get(*key) {
            Some(FrontmatterValue::String(s)) => {
                let t = s.trim();
                if !t.is_empty() {
                    out.push(t.to_owned());
                }
            }
            Some(FrontmatterValue::Seq(items)) => {
                for item in items {
                    if let FrontmatterValue::String(s) = item {
                        let t = s.trim();
                        if !t.is_empty() {
                            out.push(t.to_owned());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Extract separate BM25 enrichment fields: `(keywords_and_aliases, search_questions_and_questions)`.
#[must_use]
pub fn extract_enrichment_fields(file: &IndexedFile) -> (String, String) {
    let fm = file.doc.frontmatter.as_ref();
    let kw = collect_frontmatter_strings(fm, &["keywords", "aliases"]).join(" ");
    let sq = collect_frontmatter_strings(fm, &["search_questions", "questions"]).join(" ");
    (kw, sq)
}

/// Extract type-aware [`FreshnessMeta`] from an [`IndexedFile`].
#[must_use]
pub fn extract_freshness_meta(file: &IndexedFile) -> FreshnessMeta {
    let Some(fm) = file.doc.frontmatter.as_ref() else {
        return FreshnessMeta::from_fields(
            None,
            None,
            file.declared_type.as_deref(),
            &file.locator.path,
            None,
        );
    };
    let status = frontmatter_str(fm, &["status"]);
    let freshness = frontmatter_str(fm, &["freshness"]);
    let doc_type = file
        .declared_type
        .as_deref()
        .or_else(|| frontmatter_str(fm, &["type"]));
    let date_str = frontmatter_str(fm, &["updated", "modified", "captured", "created", "date"]);
    FreshnessMeta::from_fields(status, freshness, doc_type, &file.locator.path, date_str)
}

/// Assemble rich note text (`stem + description + title/keywords/aliases/questions + body`)
/// for hybrid semantic embeddings and `SEM-48` negation detection.
#[must_use]
pub fn render_hybrid_doc_text(file: &IndexedFile) -> String {
    let mut buf = String::new();
    let file_name = file
        .locator
        .path
        .rsplit('/')
        .next()
        .unwrap_or(&file.locator.path);
    let stem = file_name
        .strip_suffix(".md")
        .unwrap_or(file_name)
        .replace(['-', '_'], " ");
    buf.push_str(&stem);
    if let Some(ref desc) = file.description {
        buf.push('\n');
        buf.push_str(desc);
    }
    for part in collect_frontmatter_strings(
        file.doc.frontmatter.as_ref(),
        &[
            "title",
            "keywords",
            "aliases",
            "search_questions",
            "questions",
        ],
    ) {
        buf.push('\n');
        buf.push_str(&part);
    }
    buf.push('\n');
    buf.push_str(file.body());
    buf
}
