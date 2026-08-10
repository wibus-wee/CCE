use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LearningEventStage {
    ShownToModel,
    OpenedByAgent,
    CitedOrUsed,
    EditedOrAffected,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LearningFeedback {
    pub trajectory_id: String,
    pub stage: LearningEventStage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dwell_ms: Option<u64>,
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LearningReceipt {
    pub event_id: String,
    pub trajectory_id: String,
    pub recorded_at: String,
}
