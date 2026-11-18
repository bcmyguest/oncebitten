use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::rule::{load_rule, Rule};

const EXAMPLES_SOURCE: &str = "examples";
const LOCAL_SOURCE: &str = "local";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layers {
    pub local: Option<PathBuf>,
    pub project: Option<PathBuf>,
    pub sources: Vec<(String, PathBuf)>,
    pub examples: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layer {
    Local,
    Project,
    Source(String),
    Example,
}

impl Layer {
    pub fn label(&self) -> &str {
        match self {
            Layer::Local => LOCAL_SOURCE,
            Layer::Project => "project",
            Layer::Source(name) => name,
            Layer::Example => EXAMPLES_SOURCE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub id: String,
    pub layer: Layer,
    pub rule: Rule,
}

pub fn resolve(layers: &Layers) -> Result<Vec<Resolved>, Vec<anyhow::Error>> {
    resolve_with(layers, false).map(|(rules, _)| rules)
}

/// Resolves like `resolve`, but a rule directory that fails to load drops only
/// that rule. Returns the valid rules and the load errors of the dropped ones.
/// Clashes among the valid rules still fail.
pub fn resolve_skipping_broken(
    layers: &Layers,
) -> Result<(Vec<Resolved>, Vec<anyhow::Error>), Vec<anyhow::Error>> {
    resolve_with(layers, true)
}

fn resolve_with(
    layers: &Layers,
    skip_broken: bool,
) -> Result<(Vec<Resolved>, Vec<anyhow::Error>), Vec<anyhow::Error>> {
    let mut errors = Vec::new();
    let mut broken = Vec::new();
    let local = match &layers.local {
        Some(dir) => take(load_dir(dir), &mut broken),
        None => Vec::new(),
    };
    let project = match &layers.project {
        Some(dir) => take(load_dir(dir), &mut broken),
        None => Vec::new(),
    };
    let mut candidates = Vec::new();
    for (name, dir) in &layers.sources {
        for rule in take(load_dir(dir), &mut broken) {
            candidates.push(Resolved {
                id: format!("{name}/{}", rule.id),
                layer: Layer::Source(name.clone()),
                rule,
            });
        }
    }
    let examples = match &layers.examples {
        Some(dir) => take(load_dir(dir), &mut broken),
        None => Vec::new(),
    };
    for rule in examples {
        candidates.push(Resolved {
            id: format!("{EXAMPLES_SOURCE}/{}", rule.id),
            layer: Layer::Example,
            rule,
        });
    }
    let locals: Vec<Resolved> = local
        .into_iter()
        .map(|rule| Resolved {
            id: format!("{LOCAL_SOURCE}/{}", rule.id),
            layer: Layer::Local,
            rule,
        })
        .collect();
    let skipped = if skip_broken {
        broken
    } else {
        errors = broken;
        Vec::new()
    };
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut owners: BTreeMap<&str, &Resolved> = BTreeMap::new();
    for candidate in locals.iter().chain(&candidates) {
        if let Some(first) = owners.insert(candidate.id.as_str(), candidate) {
            errors.push(anyhow!(
                "`{}` is defined twice: {} and {}",
                candidate.id,
                first.rule.file.display(),
                candidate.rule.file.display()
            ));
        }
    }

    let project_ids: BTreeSet<String> = project.iter().map(|rule| rule.id.clone()).collect();
    let source_ids: BTreeSet<String> = candidates
        .iter()
        .filter(|c| c.layer != Layer::Example)
        .map(|c| c.rule.id.clone())
        .collect();
    // A local rule that repeats a shared rule's bare id yields to it, unless
    // it overrides a rule with that same bare id on purpose.
    let kept: Vec<Resolved> = locals
        .into_iter()
        .filter(|resolved| {
            let id = resolved.rule.id.as_str();
            let clashes = project_ids.contains(id) || source_ids.contains(id);
            let overrides_same_id = resolved
                .rule
                .overrides
                .as_ref()
                .and_then(|o| o.target.split_once('/'))
                .is_some_and(|(_, bare)| bare == id);
            !clashes || overrides_same_id
        })
        .collect();
    let local_ids: BTreeSet<String> = kept.iter().map(|r| r.rule.id.clone()).collect();

    let mut replaced: BTreeSet<String> = BTreeSet::new();
    for rule in kept.iter().map(|l| &l.rule).chain(&project) {
        let Some(overrides) = &rule.overrides else {
            continue;
        };
        if !candidates.iter().any(|c| c.id == overrides.target) {
            errors.push(anyhow!(
                "{}: overrides `{}`, which names no source or example rule",
                rule.file.display(),
                overrides.target
            ));
        }
        replaced.insert(overrides.target.clone());
    }
    for rule in &project {
        let target = rule.overrides.as_ref().map(|o| o.target.as_str());
        for candidate in candidates
            .iter()
            .filter(|c| c.layer != Layer::Example && c.rule.id == rule.id)
        {
            if target != Some(candidate.id.as_str()) {
                errors.push(anyhow!(
                    "{}: project rule `{}` clashes with `{}`; add overrides = \"{}\" and a reason",
                    rule.file.display(),
                    rule.id,
                    candidate.id,
                    candidate.id
                ));
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut out = kept;
    out.extend(project.into_iter().map(|rule| Resolved {
        id: rule.id.clone(),
        layer: Layer::Project,
        rule,
    }));
    for candidate in candidates {
        if replaced.contains(candidate.id.as_str()) {
            continue;
        }
        let shadowed = candidate.layer == Layer::Example
            && (project_ids.contains(candidate.rule.id.as_str())
                || local_ids.contains(candidate.rule.id.as_str())
                || source_ids.contains(candidate.rule.id.as_str()));
        if !shadowed {
            out.push(candidate);
        }
    }
    Ok((out, skipped))
}

fn take(loaded: (Vec<Rule>, Vec<anyhow::Error>), errors: &mut Vec<anyhow::Error>) -> Vec<Rule> {
    errors.extend(loaded.1);
    loaded.0
}

fn load_dir(dir: &Path) -> (Vec<Rule>, Vec<anyhow::Error>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            return (
                Vec::new(),
                vec![anyhow!("reading {}: {error}", dir.display())],
            )
        }
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir() && !is_hidden(path))
        .collect();
    paths.sort();
    let mut rules = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        match load_rule(&path) {
            Ok(rule) => rules.push(rule),
            Err(error) => errors.push(error),
        }
    }
    (rules, errors)
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}
