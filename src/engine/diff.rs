use std::path::PathBuf;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use super::{Finding, Outcome};

pub fn run(when_changed: &[String], require_changed: &[String], changed: &[String]) -> Outcome {
    let (trigger, required) = match (glob_set(when_changed), glob_set(require_changed)) {
        (Ok(trigger), Ok(required)) => (trigger, required),
        (Err(error), _) | (_, Err(error)) => return Outcome::CouldNotRun(error),
    };
    let triggers: Vec<&String> = changed
        .iter()
        .filter(|path| trigger.is_match(path))
        .collect();
    let Some(first) = triggers.first() else {
        return Outcome::Clean;
    };
    if changed.iter().any(|path| required.is_match(path)) {
        return Outcome::Clean;
    }
    Outcome::Fired(vec![Finding {
        path: Some(PathBuf::from(first)),
        line: None,
        message: format!(
            "changed {} without a change matching {}",
            triggers
                .iter()
                .map(|path| path.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            require_changed.join(", ")
        ),
        text: None,
    }])
}

/// Checks that every pattern compiles the way `run` compiles it.
pub fn check_globs(patterns: &[String]) -> Result<(), String> {
    glob_set(patterns).map(|_| ())
}

fn glob_set(patterns: &[String]) -> Result<GlobSet, String> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map_err(|error| format!("glob `{pattern}`: {error}"))?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|error| format!("glob set: {error}"))
}
