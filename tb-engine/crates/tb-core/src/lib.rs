//! Type-graph, closure resolution, validation, and candidate scanning.

pub mod body;
pub mod body_validate;
pub mod candidates;
pub mod canonical;
pub mod closure;
pub mod closure_id;
pub mod codes;
pub mod def_closure;
pub mod graph;
pub mod instance;
pub mod load_checks;
pub mod location;
pub mod location_check;
pub mod meta;
pub mod provenance;
pub mod record_targets;
pub mod resolution;
pub mod typedef;
pub mod validate;

pub use body::{
    BodyItem, BodyTemplate, FieldClaim, FillsContract, is_body_declaring, parse_body_value,
    splice_effective_body,
};
pub use body_validate::{
    SectionPresenceInfo, collect_body_typed_block_claims, collect_body_typed_block_seeds,
    compute_section_presence, validate_body, validate_docstring_links,
};
pub use candidates::{Candidate, CandidateScope, rank, scan, scan_top_level};
pub use canonical::{CanonicalHash, canonical_form};
pub use closure::{
    EffectiveShape, EffectiveShapeError, FieldOrigin, OriginId, OriginInfo, closure_of,
    effective_shape, effective_shape_resolved, field_type_refs, folded_closure_ids,
};
pub use closure_id::ClosureHash;
pub use graph::{GraphBuildResult, TypeGraph, build_graph};
pub use instance::{
    DocOrigin, DocstringLink, InlineValue, Instance, InstanceField, InstanceParseResult,
    InstanceValue, NavLink, Note, SequenceElement, TypeClaim, parse_block_record, parse_instance,
    parse_instance_stamped, parse_note,
};
pub use load_checks::{
    check_redundant_claims, is_valid_type_name, run_body_typing_checks, run_graph_structure_checks,
    run_inheritance_checks, run_location_checks,
};
pub use meta::lookup_meta;
pub use provenance::{
    Contribution, ContributionValue, Location, Surface, TupleElement, ValueContainer,
    effective_values, elaborate_fields,
};
pub use record_targets::{
    FieldSpan, Located, LocatedRecord, LocatedSequence, NestedRecord, PathSegment, RecordTarget,
    RecordTargets, ValueKind, collect_record_target_occurrences, collect_record_targets,
    enumerate_nested_records, locate_field_path, record_slot_admits_reference_at,
    shape_is_inline_or_reference, slot_shape_at,
};
pub use resolution::{
    PeerGraphResolver, ResolutionGraph, ResolvedNode, TypeId, cross_repo_type_chain_cycles,
};
pub use typedef::{
    FieldDecl, FieldName, MetaBlock, ParentClaim, ParentClaimForm, ParseResult, TypeDef, TypeName,
    TypeNameClaim, parse_type_def, type_name_from_path,
};
pub use validate::{
    CrossRepoResolver, CrossRepoTarget, MapRefData, MetaMarker, PeerType, QualifiedDemand, RefData,
    UnmetRequiredMeta, ValidateContext, unmet_required_meta, validate, validate_meta_bodies,
};
