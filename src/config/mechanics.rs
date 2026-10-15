//! Per-language flow mechanics: which node kinds play which role.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Mechanics {
    pub outside: Outside,
    pub nodes: Nodes,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Outside {
    /// File holding the module path, such as go.mod.
    pub module_file: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Nodes {
    pub assign: Vec<String>,
    pub call: Vec<String>,
    #[serde(rename = "return")]
    pub return_: Vec<String>,
    pub field: Vec<String>,
    pub param: Vec<String>,
    pub import: Vec<String>,
    pub branch: Vec<String>,
    #[serde(rename = "loop")]
    pub loop_: Vec<String>,
    pub assert: Vec<String>,
    pub composite: Vec<String>,
}
