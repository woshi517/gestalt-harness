use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnRouteDecision {
    Continue,
    Stop { reason: String },
    Route { target: String },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TurnRouterRequest {
    pub session_id: String,
    pub turn_index: usize,
}

#[async_trait]
pub trait TurnRouter: Send + Sync {
    async fn route_turn(&self, request: TurnRouterRequest) -> crate::Result<TurnRouteDecision>;
}
