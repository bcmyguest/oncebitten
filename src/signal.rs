#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    pub kind: &'static str,
    pub detail: String,
}
