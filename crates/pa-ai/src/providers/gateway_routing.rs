//! Vercel AI Gateway routing request option shared by the adapters that talk
//! to the Gateway (`/v1/chat/completions` and `/v1/messages`).

use pa_types::ai::VercelGatewayRouting;
use serde_json::{json, Map, Value};

/// The top-level `providerOptions` request field for configured routing, or
/// `None` when no `only`/`order` preference is set. `only` is a hard
/// allow-list and stays a separate key from the `order` preference.
pub(crate) fn provider_options(routing: &VercelGatewayRouting) -> Option<Value> {
    let mut gateway = Map::new();
    if let Some(only) = &routing.only {
        gateway.insert("only".into(), json!(only));
    }
    if let Some(order) = &routing.order {
        gateway.insert("order".into(), json!(order));
    }
    (!gateway.is_empty()).then(|| json!({ "gateway": Value::Object(gateway) }))
}
