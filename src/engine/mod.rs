use std::path::PathBuf;

pub mod astgrep;
pub mod diff;
pub mod native;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Fired(Vec<Finding>),
    Clean,
    CouldNotRun(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: Option<PathBuf>,
    pub line: Option<usize>,
    pub message: String,
}
