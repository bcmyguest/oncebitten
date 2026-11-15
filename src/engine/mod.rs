use std::path::PathBuf;

pub mod astgrep;

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
