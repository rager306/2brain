//! Filesystem port, frontmatter splitter, markdown body scanner, YAML AST with source spans, file-kind classifier.
//!
//! This crate is the engine's only point of contact with bytes-on-disk. Every
//! other crate consumes parsed structures, never raw I/O.

pub mod body;
pub mod codes;
pub mod file_kind;
pub mod frontmatter;
pub mod fs;
pub mod identity;
pub mod scope;
pub mod yaml;

pub use body::{
    BodyEvent, derive_section_paths, is_valid_block_id, scan_body, scan_wikilink_spans,
    take_wikilink_scan_steps,
};
pub use codes::{
    AUIGNORE_EMPTY_SCOPE, AUIGNORE_LOAD_ERROR, DUPLICATE_KEY_IN_MAPPING, FILE_TOO_LARGE,
    FRONTMATTER_UNTERMINATED, REPO_FILE_NOT_UTF8, REPO_FILE_READ_ERROR, REPO_WALK_ERROR,
    YAML_PARSE_ERROR,
};
pub use file_kind::{
    FileKind, classify, classify_by_path, is_instance_candidate_path, is_pure_yaml_instance_path,
    is_under_type_dir,
};
pub use frontmatter::{
    FrontmatterError, FrontmatterSplit, split_frontmatter, whole_as_frontmatter,
};
pub use fs::{
    FileSystem, MemoryFileSystem, RealFileSystem, ScopeBoundaries, SubtreeAudit, Walk, WalkError,
    audit_subtree,
};
pub use identity::{
    CREATE_IDENTITY_DIR, IDENTITY_DIR, IDENTITY_MARKER, identity_dir_for_write_with,
    identity_dir_with, identity_marker_with, is_identity_dir_name,
};
pub use scope::{DEFAULT_EXCLUDE_DIR_NAMES, FLOOR_DIR_NAMES, WalkFilter};
pub use yaml::{
    DuplicateKey, MarkedYaml, SourceIndexGuard, YamlData, YamlError, index_source, parse,
    scan_duplicate_keys, span_to_byte_range, take_cp_fallback_count,
};
