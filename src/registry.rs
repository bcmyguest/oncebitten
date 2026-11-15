use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::rule::{load_rule, Rule};

const EXAMPLES_SOURCE: &str = "examples";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layers {
    pub project: Option<PathBuf>,
    pub sources: Vec<(String, PathBuf)>,
    pub examples: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layer {
    Project,
    Source(String),
    Example,
}

impl Layer {
    pub fn label(&self) -> &str {
        match self {
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
    let mut errors = Vec::new();
    let project = match &layers.project {
        Some(dir) => take(load_dir(dir), &mut errors),
        None => Vec::new(),
    };
    let mut candidates = Vec::new();
    for (name, dir) in &layers.sources {
        for rule in take(load_dir(dir), &mut errors) {
            candidates.push(Resolved {
                id: format!("{name}/{}", rule.id),
                layer: Layer::Source(name.clone()),
                rule,
            });
        }
    }
    let examples = match &layers.examples {
        Some(dir) => take(load_dir(dir), &mut errors),
        None => Vec::new(),
    };
    for rule in examples {
        candidates.push(Resolved {
            id: format!("{EXAMPLES_SOURCE}/{}", rule.id),
            layer: Layer::Example,
            rule,
        });
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut owners: BTreeMap<&str, &Resolved> = BTreeMap::new();
    for candidate in &candidates {
        if let Some(first) = owners.insert(candidate.id.as_str(), candidate) {
            errors.push(anyhow!(
                "`{}` is defined twice: {} and {}",
                candidate.id,
                first.rule.file.display(),
                candidate.rule.file.display()
            ));
        }
    }

    let mut replaced: BTreeSet<String> = BTreeSet::new();
    for rule in &project {
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

    let project_ids: BTreeSet<String> = project.iter().map(|rule| rule.id.clone()).collect();
    let source_ids: BTreeSet<String> = candidates
        .iter()
        .filter(|c| c.layer != Layer::Example)
        .map(|c| c.rule.id.clone())
        .collect();
    let mut out: Vec<Resolved> = project
        .into_iter()
        .map(|rule| Resolved {
            id: rule.id.clone(),
            layer: Layer::Project,
            rule,
        })
        .collect();
    for candidate in candidates {
        if replaced.contains(candidate.id.as_str()) {
            continue;
        }
        let shadowed = candidate.layer == Layer::Example
            && (project_ids.contains(candidate.rule.id.as_str())
                || source_ids.contains(candidate.rule.id.as_str()));
        if !shadowed {
            out.push(candidate);
        }
    }
    Ok(out)
}

fn take(
    loaded: Result<Vec<Rule>, Vec<anyhow::Error>>,
    errors: &mut Vec<anyhow::Error>,
) -> Vec<Rule> {
    match loaded {
        Ok(rules) => rules,
        Err(found) => {
            errors.extend(found);
            Vec::new()
        }
    }
}

fn load_dir(dir: &Path) -> Result<Vec<Rule>, Vec<anyhow::Error>> {
    let entries = std::fs::read_dir(dir)
        .map_err(|error| vec![anyhow!("reading {}: {error}", dir.display())])?;
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
    if errors.is_empty() {
        Ok(rules)
    } else {
        Err(errors)
    }
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}
