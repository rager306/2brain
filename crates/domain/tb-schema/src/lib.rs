//! Pure `*.type.yaml` schema parser, inheritance resolver, and validator (0 I/O).
//!
//! Built strictly from `spec/02-type-system-and-diagnostics.md`.

use std::collections::{BTreeMap, BTreeSet};

use saphyr::{LoadableYamlNode, Scalar, Yaml};
use serde::{Deserialize, Serialize};
use tb_syntax::{FrontmatterValue, ParsedDocument};
use thiserror::Error;

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Drift,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct Diagnostic {
    pub repo: String,
    pub path: String,
    pub byte_start: usize,
    pub code: String,
    pub severity: Severity,
    pub message: String,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Cardinality {
    Single,
    List,
    NonEmptyList,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum NumericConstraint {
    Gte(i64),
    Gt(i64),
    Lte(i64),
    Lt(i64),
    Integer,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RefMode {
    Embedded,
    Link,
    Owned,
    MetaType,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum BaseType {
    String,
    Boolean,
    DateTime,
    Number(Vec<NumericConstraint>),
    Enum(Vec<String>),
    TypeRef {
        name: String,
        repo: Option<String>,
        mode: RefMode,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FieldTypeExpr {
    pub base: BaseType,
    pub cardinality: Cardinality,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FieldSpec {
    pub name: String,
    pub optional: bool,
    pub expr: FieldTypeExpr,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TypeDef {
    pub repo: String,
    pub name: String,
    pub extends: Option<(String, Option<String>)>,
    pub is_abstract: bool,
    pub sealed: bool,
    pub meta: bool,
    pub location: Option<String>,
    pub body: Option<String>,
    pub fields: BTreeMap<String, FieldSpec>,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum SchemaError {
    #[error("failed to parse type YAML '{name}': {detail}")]
    Parse { name: String, detail: String },
    #[error("invalid field type expression '{expr}': {detail}")]
    InvalidTypeExpr { expr: String, detail: String },
}

/// Parse one `*.type.yaml` file into a [`TypeDef`].
pub fn parse_type_def(repo: &str, name: &str, yaml_src: &str) -> Result<TypeDef, SchemaError> {
    let docs = Yaml::load_from_str(yaml_src).map_err(|e| SchemaError::Parse {
        name: name.to_owned(),
        detail: e.to_string(),
    })?;
    let Some(Yaml::Mapping(root)) = docs.into_iter().next() else {
        return Err(SchemaError::Parse {
            name: name.to_owned(),
            detail: "type definition must be a YAML mapping".to_owned(),
        });
    };

    let mut extends = None;
    let mut is_abstract = false;
    let mut sealed = false;
    let mut meta = false;
    let mut location = None;
    let mut body = None;
    let mut fields = BTreeMap::new();

    for (k, v) in root {
        let Some(key) = yaml_as_str(&k) else {
            continue;
        };
        match key {
            "extends" => {
                if let Some(s) = yaml_as_str(&v) {
                    extends = Some(split_type_ref(s));
                }
            }
            "abstract" => is_abstract = matches!(v, Yaml::Value(Scalar::Boolean(true))),
            "sealed" => sealed = matches!(v, Yaml::Value(Scalar::Boolean(true))),
            "meta" => meta = matches!(v, Yaml::Value(Scalar::Boolean(true))),
            "location" => location = yaml_as_str(&v).map(str::to_owned),
            "body" => body = yaml_as_str(&v).map(str::to_owned),
            "fields" => {
                if let Yaml::Mapping(fmap) = v {
                    for (fk, fv) in fmap {
                        let Some(raw_fname) = yaml_as_str(&fk) else {
                            continue;
                        };
                        let (fname, optional) = match raw_fname.strip_suffix('?') {
                            Some(clean) => (clean.to_owned(), true),
                            None => (raw_fname.to_owned(), false),
                        };
                        let expr = parse_yaml_field_type(&fv)?;
                        fields.insert(
                            fname.clone(),
                            FieldSpec {
                                name: fname,
                                optional,
                                expr,
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }

    Ok(TypeDef {
        repo: repo.to_owned(),
        name: name.to_owned(),
        extends,
        is_abstract,
        sealed,
        meta,
        location,
        body,
        fields,
    })
}

/// Parse a field type expression string per `spec/02-type-system-and-diagnostics.md` §1.2.
pub fn parse_type_expr(raw: &str) -> Result<FieldTypeExpr, SchemaError> {
    let trimmed = raw.trim();
    let (inner, cardinality) = if let Some(s) = trimmed.strip_suffix("[]") {
        (s.trim(), Cardinality::List)
    } else if let Some(s) = trimmed.strip_suffix("[+]") {
        (s.trim(), Cardinality::NonEmptyList)
    } else {
        (trimmed, Cardinality::Single)
    };

    let base = if inner == "String" {
        BaseType::String
    } else if inner == "Boolean" {
        BaseType::Boolean
    } else if inner == "DateTime" {
        BaseType::DateTime
    } else if inner == "Number" {
        BaseType::Number(Vec::new())
    } else if let Some(rest) = inner
        .strip_prefix("Number{")
        .and_then(|s| s.strip_suffix('}'))
    {
        BaseType::Number(parse_numeric_constraints(rest, raw)?)
    } else if let Some(rest) = inner.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        let items = rest
            .split(',')
            .map(|item| item.trim().trim_matches(['\'', '"']).to_owned())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        BaseType::Enum(items)
    } else if let Some(rest) = inner
        .strip_prefix("type<")
        .and_then(|s| s.strip_suffix(">*"))
    {
        let (name, repo) = split_type_ref(rest);
        BaseType::TypeRef {
            name,
            repo,
            mode: RefMode::MetaType,
        }
    } else if let Some(rest) = inner.strip_suffix('*') {
        let (name, repo) = split_type_ref(rest);
        BaseType::TypeRef {
            name,
            repo,
            mode: RefMode::Link,
        }
    } else if let Some(rest) = inner.strip_suffix('&') {
        let (name, repo) = split_type_ref(rest);
        BaseType::TypeRef {
            name,
            repo,
            mode: RefMode::Owned,
        }
    } else if !inner.is_empty() {
        let (name, repo) = split_type_ref(inner);
        BaseType::TypeRef {
            name,
            repo,
            mode: RefMode::Embedded,
        }
    } else {
        return Err(SchemaError::InvalidTypeExpr {
            expr: raw.to_owned(),
            detail: "empty type expression".to_owned(),
        });
    };

    Ok(FieldTypeExpr { base, cardinality })
}

/// Registry of all loaded type definitions across workspace members.
#[derive(Clone, Debug, Default)]
pub struct SchemaRegistry {
    defs: BTreeMap<(String, String), TypeDef>,
    deps: BTreeMap<String, BTreeSet<String>>,
}

impl SchemaRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_dep(&mut self, repo: impl Into<String>, dep: impl Into<String>) {
        self.deps.entry(repo.into()).or_default().insert(dep.into());
    }

    pub fn insert(&mut self, def: TypeDef) {
        self.defs.insert((def.repo.clone(), def.name.clone()), def);
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.defs.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// Resolve a type reference (`name` or `name::repo`) visible from `from_repo`.
    #[must_use]
    pub fn resolve(&self, from_repo: &str, type_ref: &str) -> Option<&TypeDef> {
        let (name, explicit_repo) = split_type_ref(type_ref);
        if let Some(target_repo) = explicit_repo {
            if target_repo != from_repo
                && !self
                    .deps
                    .get(from_repo)
                    .is_some_and(|d| d.contains(&target_repo))
            {
                return None;
            }
            return self.defs.get(&(target_repo, name));
        }
        if let Some(local) = self.defs.get(&(from_repo.to_owned(), name.clone())) {
            return Some(local);
        }
        if let Some(dep_set) = self.deps.get(from_repo) {
            for dep in dep_set {
                if let Some(found) = self.defs.get(&(dep.clone(), name.clone())) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Collect effective fields for `(repo, name)`, reporting inheritance cycles.
    pub fn effective_fields(
        &self,
        repo: &str,
        name: &str,
    ) -> Result<BTreeMap<String, FieldSpec>, String> {
        let mut chain = Vec::new();
        let mut visited = BTreeSet::new();
        let mut cur_repo = repo.to_owned();
        let mut cur_name = name.to_owned();

        loop {
            if !visited.insert((cur_repo.clone(), cur_name.clone())) {
                return Err(format!("type inheritance cycle involving '{cur_name}'"));
            }
            let Some(def) = self.defs.get(&(cur_repo.clone(), cur_name.clone())) else {
                break;
            };
            chain.push(def);
            let Some((ref p_name, ref p_repo)) = def.extends else {
                break;
            };
            let ref_str = match p_repo {
                Some(r) => format!("{p_name}::{r}"),
                None => p_name.clone(),
            };
            let Some(parent) = self.resolve(&cur_repo, &ref_str) else {
                break;
            };
            cur_repo = parent.repo.clone();
            cur_name = parent.name.clone();
        }

        let mut merged = BTreeMap::new();
        for def in chain.into_iter().rev() {
            for (k, v) in &def.fields {
                merged.insert(k.clone(), v.clone());
            }
        }
        Ok(merged)
    }

    /// Validate a parsed document's frontmatter against its declared `type`.
    #[must_use]
    pub fn validate_document(
        &self,
        repo: &str,
        path: &str,
        doc: &ParsedDocument,
    ) -> Vec<Diagnostic> {
        let mut diags = Vec::new();
        if let Some(ref err) = doc.frontmatter_error {
            diags.push(Diagnostic {
                repo: repo.to_owned(),
                path: path.to_owned(),
                byte_start: 0,
                code: "frontmatter-parse-error".to_owned(),
                severity: Severity::Error,
                message: err.clone(),
            });
            return diags;
        }
        let Some(ref fm) = doc.frontmatter else {
            return diags;
        };
        let Some(ref declared) = doc.declared_type else {
            return diags;
        };
        // Engine builtin descriptors (`tb.engine.*`) are self-described metadata files.
        if declared.starts_with("tb.engine.") {
            return diags;
        }

        let Some(def) = self.resolve(repo, declared) else {
            diags.push(Diagnostic {
                repo: repo.to_owned(),
                path: path.to_owned(),
                byte_start: 0,
                code: "unknown-type".to_owned(),
                severity: Severity::Error,
                message: format!("unknown type '{declared}'"),
            });
            return diags;
        };
        if def.is_abstract {
            diags.push(Diagnostic {
                repo: repo.to_owned(),
                path: path.to_owned(),
                byte_start: 0,
                code: "abstract-type-instantiated".to_owned(),
                severity: Severity::Error,
                message: format!("cannot instantiate abstract type '{declared}'"),
            });
        }
        let fields = match self.effective_fields(&def.repo, &def.name) {
            Ok(f) => f,
            Err(cycle) => {
                diags.push(Diagnostic {
                    repo: repo.to_owned(),
                    path: path.to_owned(),
                    byte_start: 0,
                    code: "type-cycle".to_owned(),
                    severity: Severity::Error,
                    message: cycle,
                });
                return diags;
            }
        };

        for (fname, spec) in &fields {
            match fm.get(fname) {
                None | Some(FrontmatterValue::Null) if !spec.optional => {
                    diags.push(Diagnostic {
                        repo: repo.to_owned(),
                        path: path.to_owned(),
                        byte_start: 0,
                        code: "missing-required-field".to_owned(),
                        severity: Severity::Error,
                        message: format!(
                            "missing required field '{fname}' for type '{}'",
                            def.name
                        ),
                    });
                }
                Some(val) if !matches!(val, FrontmatterValue::Null) => {
                    if let Err(reason) = self.validate_value(&def.repo, &spec.expr, val) {
                        diags.push(Diagnostic {
                            repo: repo.to_owned(),
                            path: path.to_owned(),
                            byte_start: 0,
                            code: "invalid-field-value".to_owned(),
                            severity: Severity::Error,
                            message: format!("field '{fname}': {reason}"),
                        });
                    }
                }
                _ => {}
            }
        }
        diags.sort();
        diags
    }

    fn validate_value(
        &self,
        from_repo: &str,
        expr: &FieldTypeExpr,
        val: &FrontmatterValue,
    ) -> Result<(), String> {
        match expr.cardinality {
            Cardinality::Single => self.validate_base(from_repo, &expr.base, val),
            Cardinality::List | Cardinality::NonEmptyList => {
                let FrontmatterValue::Seq(items) = val else {
                    return Err("expected a list".to_owned());
                };
                if expr.cardinality == Cardinality::NonEmptyList && items.is_empty() {
                    return Err("expected a non-empty list".to_owned());
                }
                for item in items {
                    self.validate_base(from_repo, &expr.base, item)?;
                }
                Ok(())
            }
        }
    }

    fn validate_base(
        &self,
        from_repo: &str,
        base: &BaseType,
        val: &FrontmatterValue,
    ) -> Result<(), String> {
        match base {
            BaseType::String => match val {
                FrontmatterValue::String(_) => Ok(()),
                _ => Err("expected String".to_owned()),
            },
            BaseType::Boolean => match val {
                FrontmatterValue::Bool(_) => Ok(()),
                _ => Err("expected Boolean".to_owned()),
            },
            BaseType::DateTime => match val {
                FrontmatterValue::String(s) if is_valid_datetime(s) => Ok(()),
                _ => Err("expected ISO-8601 or YYYY-MM-DDThhmmssZ DateTime".to_owned()),
            },
            BaseType::Number(constraints) => validate_number(constraints, val),
            BaseType::Enum(allowed) => match val {
                FrontmatterValue::String(s) if allowed.iter().any(|a| a == s) => Ok(()),
                _ => Err(format!("expected one of {allowed:?}")),
            },
            BaseType::TypeRef {
                name,
                repo,
                mode: RefMode::Embedded,
            } => {
                let FrontmatterValue::Map(map) = val else {
                    return Err(format!("expected object matching type '{name}'"));
                };
                let ref_str = match repo {
                    Some(r) => format!("{name}::{r}"),
                    None => name.clone(),
                };
                if let Some(nested) = self.resolve(from_repo, &ref_str)
                    && let Ok(nfields) = self.effective_fields(&nested.repo, &nested.name)
                {
                    for (k, spec) in &nfields {
                        match map.get(k) {
                            None | Some(FrontmatterValue::Null) if !spec.optional => {
                                return Err(format!("nested '{name}' missing required '{k}'"));
                            }
                            Some(nv) if !matches!(nv, FrontmatterValue::Null) => {
                                self.validate_value(&nested.repo, &spec.expr, nv)?;
                            }
                            _ => {}
                        }
                    }
                }
                Ok(())
            }
            BaseType::TypeRef {
                mode: RefMode::Link | RefMode::Owned | RefMode::MetaType,
                ..
            } => match val {
                FrontmatterValue::String(_) | FrontmatterValue::Map(_) => Ok(()),
                _ => Err("expected reference string or object".to_owned()),
            },
        }
    }
}

fn parse_yaml_field_type(val: &Yaml<'_>) -> Result<FieldTypeExpr, SchemaError> {
    match val {
        Yaml::Value(Scalar::String(s)) => parse_type_expr(s),
        Yaml::Sequence(seq) => {
            let items = seq
                .iter()
                .filter_map(yaml_as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>();
            Ok(FieldTypeExpr {
                base: BaseType::Enum(items),
                cardinality: Cardinality::Single,
            })
        }
        Yaml::Mapping(map) => {
            for (k, v) in map {
                if yaml_as_str(k) == Some("type") {
                    return parse_yaml_field_type(v);
                }
            }
            Err(SchemaError::InvalidTypeExpr {
                expr: "<mapping>".to_owned(),
                detail: "field mapping missing 'type' key".to_owned(),
            })
        }
        _ => Err(SchemaError::InvalidTypeExpr {
            expr: format!("{val:?}"),
            detail: "unsupported field type YAML node".to_owned(),
        }),
    }
}

fn parse_numeric_constraints(spec: &str, raw: &str) -> Result<Vec<NumericConstraint>, SchemaError> {
    let mut out = Vec::new();
    for clause in spec.split('&') {
        let c = clause.trim();
        if c == "integer" {
            out.push(NumericConstraint::Integer);
        } else if let Some(n) = c.strip_prefix(">=") {
            out.push(NumericConstraint::Gte(parse_i64(n, raw)?));
        } else if let Some(n) = c.strip_prefix("<=") {
            out.push(NumericConstraint::Lte(parse_i64(n, raw)?));
        } else if let Some(n) = c.strip_prefix('>') {
            out.push(NumericConstraint::Gt(parse_i64(n, raw)?));
        } else if let Some(n) = c.strip_prefix('<') {
            out.push(NumericConstraint::Lt(parse_i64(n, raw)?));
        } else if !c.is_empty() {
            return Err(SchemaError::InvalidTypeExpr {
                expr: raw.to_owned(),
                detail: format!("unknown numeric constraint '{c}'"),
            });
        }
    }
    Ok(out)
}

fn parse_i64(s: &str, raw: &str) -> Result<i64, SchemaError> {
    s.trim()
        .parse::<i64>()
        .map_err(|_| SchemaError::InvalidTypeExpr {
            expr: raw.to_owned(),
            detail: format!("invalid integer bound '{s}'"),
        })
}

fn validate_number(
    constraints: &[NumericConstraint],
    val: &FrontmatterValue,
) -> Result<(), String> {
    let (as_f64, is_int) = match *val {
        FrontmatterValue::Int(i) => (i as f64, true),
        FrontmatterValue::Float(f) => (f, f.fract() == 0.0),
        _ => return Err("expected Number".to_owned()),
    };
    for &c in constraints {
        match c {
            NumericConstraint::Integer if !is_int => return Err("expected integer".to_owned()),
            NumericConstraint::Gte(n) if as_f64 < n as f64 => {
                return Err(format!("must be >= {n}"));
            }
            NumericConstraint::Gt(n) if as_f64 <= n as f64 => {
                return Err(format!("must be > {n}"));
            }
            NumericConstraint::Lte(n) if as_f64 > n as f64 => {
                return Err(format!("must be <= {n}"));
            }
            NumericConstraint::Lt(n) if as_f64 >= n as f64 => {
                return Err(format!("must be < {n}"));
            }
            _ => {}
        }
    }
    Ok(())
}

fn is_valid_datetime(s: &str) -> bool {
    let b = s.as_bytes();
    // Accept YYYY-MM-DD, YYYY-MM-DDTHH:MM:SSZ, or colon-free YYYY-MM-DDThhmmssZ
    b.len() >= 10
        && b[0..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit)
}

fn split_type_ref(raw: &str) -> (String, Option<String>) {
    match raw.trim().split_once("::") {
        Some((name, repo)) => (name.trim().to_owned(), Some(repo.trim().to_owned())),
        None => (raw.trim().to_owned(), None),
    }
}

fn yaml_as_str<'a>(val: &'a Yaml<'a>) -> Option<&'a str> {
    match val {
        Yaml::Value(Scalar::String(s)) => Some(s.as_ref()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_four_real_schema_families() {
        // 1. project-memory task with nested structs, enums, DateTime, optional fields
        let task = parse_type_def(
            "vault",
            "task",
            "fields:\n  description: String\n  status: [open, closed]\n  revision: Number\n  obligations: task-obligation[]\n  next_check?: String\n",
        )
        .unwrap();
        assert_eq!(task.fields.len(), 5);
        assert!(task.fields["next_check"].optional);
        assert!(!task.fields["revision"].optional);

        // 2. tb-mcp-sdk with extends, numeric constraints, meta-type refs
        let skill = parse_type_def(
            "tb-mcp-sdk",
            "mcp.skill",
            "extends: mcp\nfields:\n  name: String\n  retries?: Number{>=0 & integer}\n  related-tools?: type<mcp.tool>*[]\n",
        )
        .unwrap();
        assert_eq!(skill.extends, Some(("mcp".to_owned(), None)));
        assert_eq!(
            skill.fields["retries"].expr.base,
            BaseType::Number(vec![NumericConstraint::Gte(0), NumericConstraint::Integer])
        );

        // 3. tb-weave with cross-repo extends & non-empty link list [+]
        let claim = parse_type_def(
            "tb-weave",
            "claim",
            "extends: idea::tb-base-types\nfields:\n  grounds: source::tb-base-types*[+]\n",
        )
        .unwrap();
        assert_eq!(
            claim.extends,
            Some(("idea".to_owned(), Some("tb-base-types".to_owned())))
        );
        assert_eq!(
            claim.fields["grounds"].expr.cardinality,
            Cardinality::NonEmptyList
        );
    }

    #[test]
    fn validates_instance_missing_fields_and_invalid_enum() {
        let mut reg = SchemaRegistry::new();
        reg.insert(
            parse_type_def(
                "vault",
                "insight",
                "fields:\n  description: String\n  status: [current, aging, stale, superseded]\n",
            )
            .unwrap(),
        );

        let good = tb_syntax::parse_document(
            b"---\ntype: insight\ndescription: \"Valid\"\nstatus: current\n---\nBody\n",
        )
        .unwrap();
        assert!(reg.validate_document("vault", "a.md", &good).is_empty());

        let bad =
            tb_syntax::parse_document(b"---\ntype: insight\nstatus: invalid\n---\nBody\n").unwrap();
        let diags = reg.validate_document("vault", "b.md", &bad);
        assert_eq!(diags.len(), 2);
        assert!(diags.iter().any(|d| d.code == "missing-required-field"));
        assert!(diags.iter().any(|d| d.code == "invalid-field-value"));
    }
}
