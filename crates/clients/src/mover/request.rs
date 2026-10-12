//! TESTCore `Mover/Request/RequestMoverStart.cs` 이식.

use serde::{Deserialize, Serialize};

use super::data::{SourceConfig, TargetConfig};

/// Mover 시작 요청. `ToString()`은 `ToJsonString()`과 같은 JSON이다.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct RequestMoverStart {
    pub user_id: Option<String>,
    #[serde(rename = "Type")]
    pub kind: Option<String>,
    pub source: Option<SourceConfig>,
    pub target: Option<TargetConfig>,
}

impl RequestMoverStart {
    /// 원본 `ToString()`: `ToJsonString()`
    pub fn to_json_string(&self) -> String {
        testrust_common::to_dotnet_json(self)
    }
}
