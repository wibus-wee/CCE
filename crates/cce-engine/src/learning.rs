use cce_core::{CceError, LearningEventStage, LearningFeedback, LearningReceipt, Result};
use cce_store::ArtifactKind;
use chrono::Utc;
use uuid::Uuid;

use crate::{CceEngine, SearchResult};

impl CceEngine {
    pub(crate) fn capture_search_result(&self, result: &mut SearchResult) -> Result<()> {
        let trajectory_id = Uuid::now_v7().to_string();
        result.trajectory_id = Some(trajectory_id.clone());
        let bytes = serde_json::to_vec(result)?;
        let artifact = self
            .store()
            .artifacts()
            .put_bytes(ArtifactKind::Trace, &bytes)?;
        self.store().record_trajectory(
            &trajectory_id,
            &result.request.repository_id,
            &result.request.snapshot_id,
            &result.request.query,
            result.plan.intent,
            &artifact,
        )?;
        Ok(())
    }

    pub fn record_learning_feedback(&self, feedback: LearningFeedback) -> Result<LearningReceipt> {
        if feedback.trajectory_id.trim().is_empty() {
            return Err(CceError::Configuration(
                "learning feedback requires a trajectoryId".to_owned(),
            ));
        }
        if feedback.dwell_ms.is_some_and(|value| value > 86_400_000) {
            return Err(CceError::Configuration(
                "learning feedback dwellMs must not exceed 24 hours".to_owned(),
            ));
        }
        if serde_json::to_vec(&feedback.metadata)?.len() > 64 * 1024 {
            return Err(CceError::Configuration(
                "learning feedback metadata exceeds 64 KiB".to_owned(),
            ));
        }
        let bytes = self
            .store()
            .trajectory_bytes(&feedback.trajectory_id)?
            .ok_or_else(|| {
                CceError::Configuration(format!(
                    "unknown learning trajectory {}",
                    feedback.trajectory_id
                ))
            })?;
        let trace: SearchResult = serde_json::from_slice(&bytes).map_err(|error| {
            CceError::ArtifactCorrupt(format!("invalid learning trajectory: {error}"))
        })?;
        let document_required = matches!(
            feedback.stage,
            LearningEventStage::ShownToModel
                | LearningEventStage::OpenedByAgent
                | LearningEventStage::CitedOrUsed
                | LearningEventStage::EditedOrAffected
        );
        if document_required && feedback.document_id.is_none() {
            return Err(CceError::Configuration(format!(
                "learning stage {:?} requires documentId",
                feedback.stage
            )));
        }
        if let Some(document_id) = &feedback.document_id
            && !trace.hits.iter().any(|hit| &hit.document_id == document_id)
        {
            return Err(CceError::Configuration(format!(
                "document {document_id} was not exposed by trajectory {}",
                feedback.trajectory_id
            )));
        }
        let receipt = LearningReceipt {
            event_id: Uuid::now_v7().to_string(),
            trajectory_id: feedback.trajectory_id.clone(),
            recorded_at: Utc::now().to_rfc3339(),
        };
        self.store().record_learning_feedback(&feedback, &receipt)?;
        Ok(receipt)
    }
}
