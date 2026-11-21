use super::Router;
use crate::{configuration::Configuration, extensions::Catalog, mcp::McpRegistry, store::Store};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    time::Duration,
};

#[derive(Default)]
pub struct Statistics {
    session: Option<String>,
    after: i64,
    samples: VecDeque<Value>,
    calls: u64,
    requests: u64,
    seconds: f64,
    generation: f64,
    measured: u64,
    generated: u64,
    recorded_cost: f64,
    usage_incomplete: bool,
    has_fake: bool,
    unpriced_by_model: BTreeMap<String, crate::models::Usage>,
}

impl Statistics {
    pub(super) fn read(
        &mut self,
        store: &Store,
        configuration: &Configuration,
        fallback_model: &str,
        session: Option<&str>,
    ) -> Result<Value> {
        if self.session.as_deref() != session {
            *self = Self {
                session: session.map(str::to_string),
                after: -1,
                ..Self::default()
            };
        }
        if let Some(id) = session {
            let (until, events) = store.statistics_events(id, self.after)?;
            for e in events {
                if e.r#type == "tool_call_start" {
                    self.calls += 1
                } else if e.r#type == "assistant_message" {
                    let usage = &e.data["usage"];
                    let output = usage["output_tokens"].as_u64().unwrap_or(0);
                    let available = usage["available"] != false;
                    let turn_usage = crate::models::Usage {
                        input_tokens: usage["input_tokens"].as_u64().unwrap_or(0),
                        output_tokens: output,
                        cache_read_tokens: usage["cache_read_tokens"].as_u64().unwrap_or(0),
                        cache_write_tokens: usage["cache_write_tokens"].as_u64().unwrap_or(0),
                        available,
                    };
                    let event_model = e.data["model"].as_str().unwrap_or(fallback_model);
                    if event_model == "fake" {
                        self.has_fake = true;
                    }
                    if !turn_usage.available {
                        self.usage_incomplete = true;
                    }
                    let sample_cost = if event_model == "fake" || !turn_usage.available {
                        None
                    } else if let Some(recorded) = e.data["cost_usd"]
                        .as_f64()
                        .filter(|n| n.is_finite() && *n >= 0.0)
                    {
                        self.recorded_cost += recorded;
                        Some(recorded)
                    } else {
                        self.unpriced_by_model
                            .entry(event_model.to_string())
                            .or_default()
                            .add(&turn_usage);
                        configuration
                            .resolve_pricing(event_model)
                            .ok()
                            .flatten()
                            .and_then(|pricing| pricing.cost(&turn_usage))
                    };
                    let seconds = e.data["request_seconds"].as_f64();
                    let generation = e.data["generation_seconds"].as_f64();
                    self.requests += 1;
                    let request_tps = seconds
                        .filter(|s| *s > 0.0 && available)
                        .map(|s| output as f64 / s);
                    let tps = generation
                        .filter(|s| *s > 0.0 && available)
                        .map(|s| output as f64 / s);
                    if request_tps.is_some() {
                        self.seconds += seconds.unwrap();
                        self.measured += output
                    }
                    if tps.is_some() {
                        self.generation += generation.unwrap();
                        self.generated += output
                    }
                    self.samples.push_back(json!({"round":self.requests,"input":usage["input_tokens"],"output":output,"cached":usage["cache_read_tokens"],"available":available,"seconds":seconds,"generationSeconds":generation,"firstTokenSeconds":e.data["first_token_seconds"],"requestTps":request_tps,"tps":tps,"costUsd":sample_cost}));
                    if self.samples.len() > 32 {
                        self.samples.pop_front();
                    }
                }
            }
            self.after = until;
        }
        let total_cost = if self.requests == 0 || self.usage_incomplete || self.has_fake {
            None
        } else {
            let mut sum = self.recorded_cost;
            let mut complete = true;
            for (model, usage) in &self.unpriced_by_model {
                if let Some(pricing) = configuration.resolve_pricing(model)?
                    && let Some(part) = pricing.cost(usage)
                {
                    sum += part;
                } else {
                    complete = false;
                    break;
                }
            }
            complete.then_some(sum)
        };
        Ok(
            json!({"toolCalls":self.calls,"requests":self.requests,"modelSeconds":self.seconds,"tps":if self.generation>0.0{Some(self.generated as f64/self.generation)}else{None},"requestTps":if self.seconds>0.0{Some(self.measured as f64/self.seconds)}else{None},"lastTps":self.samples.back().map(|v|v["tps"].clone()).unwrap_or(Value::Null),"samples":self.samples,"cost":total_cost}),
        )
    }
}

fn per_token_million(value: &Value) -> Option<f64> {
    let raw = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse::<f64>().ok()))?;
    (raw.is_finite() && raw >= 0.0).then_some(raw * 1_000_000.0)
}

fn discovered_pricing(model: &Value) -> Option<crate::eval_stats::Pricing> {
    let pricing = model.get("pricing").filter(|value| value.is_object())?;
    if let Ok(Some(direct)) = crate::eval_stats::Pricing::from_value(pricing) {
        return Some(direct);
    }
    let input = per_token_million(&pricing["prompt"])?;
    let output = per_token_million(&pricing["completion"])?;
    let cache_read = if pricing.get("input_cache_read").is_none_or(Value::is_null) {
        0.0
    } else {
        per_token_million(&pricing["input_cache_read"])?
    };
    let cache_write = if pricing.get("input_cache_write").is_none_or(Value::is_null) {
        0.0
    } else {
        per_token_million(&pricing["input_cache_write"])?
    };
    Some(crate::eval_stats::Pricing {
        input,
        output,
        cache_read,
        cache_write,
    })
}

fn discovered_model(model: &Value) -> Value {
    let id = model["id"].as_str().unwrap_or_default();
    let catalog = crate::configuration::catalog_model(id);
    let window = [
        &model["context_window"],
        &model["context_length"],
        &model["contextWindow"],
        &catalog["context_window"],
    ]
    .iter()
    .find_map(|value| value.as_u64().filter(|value| *value > 1))
    .unwrap_or(128_000);
    let output = [
        &model["max_output_tokens"],
        &model["max_completion_tokens"],
        &model["maxOutputTokens"],
        &model["top_provider"]["max_completion_tokens"],
        &catalog["max_output_tokens"],
    ]
    .iter()
    .find_map(|value| value.as_u64().filter(|value| *value > 0))
    .unwrap_or(8192)
    .min(window - 1);
    let levels = model["effort"]["supported_levels"].as_array();
    let supports = model["supportsEffort"]
        .as_bool()
        .or_else(|| model["supports_effort"].as_bool())
        .or_else(|| levels.map(|levels| !levels.is_empty()))
        .or_else(|| catalog["supports_effort"].as_bool())
        .unwrap_or(false);
    let modalities = model["input_modalities"]
        .as_array()
        .or_else(|| model["architecture"]["input_modalities"].as_array())
        .or_else(|| catalog["input_modalities"].as_array());
    let has = |name: &str, capability: &str| {
        model["capabilities"][capability]
            .as_bool()
            .unwrap_or_else(|| {
                modalities.is_some_and(|values| values.iter().any(|value| value == name))
            })
    };
    let mut result = json!({
        "modelId":id,
        "name":model["display_name"].as_str().or_else(|| model["name"].as_str()).unwrap_or(id),
        "contextWindow":window,
        "maxOutputTokens":output,
        "supportsEffort":supports,
        "capabilities":{"images":has("image", "images"),"pdf":has("pdf", "pdf"),"audio":has("audio", "audio"),"video":has("video", "video")}
    });
    let mut compat = model
        .get("parameterCompat")
        .or_else(|| catalog.get("parameter_compat"))
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Some(levels) = levels {
        let order = ["minimal", "low", "medium", "high", "xhigh", "max"];
        let supported = order
            .iter()
            .enumerate()
            .filter(|(_, level)| levels.iter().any(|value| value == **level))
            .collect::<Vec<_>>();
        if !supported.is_empty() {
            let mut mappings = serde_json::Map::new();
            for (index, effort) in order.iter().enumerate().skip(1) {
                let selected = supported
                    .iter()
                    .rev()
                    .find(|(rank, _)| *rank <= index)
                    .or_else(|| supported.first())
                    .unwrap()
                    .1;
                mappings.insert((*effort).into(), json!(selected));
            }
            compat["reasoningEffort"] = Value::Object(mappings);
        }
    }
    if !compat.as_object().unwrap().is_empty() {
        result["parameterCompat"] = compat;
    }
    if let Some(pricing) = discovered_pricing(model) {
        result["pricing"] = pricing.to_config_value();
    }
    result
}

pub async fn network(method: &str, p: &Value, configuration: &Configuration) -> Result<Value> {
    if method == "refreshMcp" {
        let workspace = Router::workspace(p)?;
        let catalog = Catalog::scan(&workspace, configuration.clone(), true)?;
        let registry = McpRegistry::prepare(catalog.servers).await;
        registry.close().await;
        return Ok(
            json!({"servers":registry.statuses_with_diagnostics().await,"tools":registry.tools.keys().collect::<Vec<_>>() }),
        );
    }
    let draft = &p["service"];
    let existing = configuration.read()?["services"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == draft["id"])
        .cloned()
        .unwrap_or(json!({}));
    let mut service = existing.clone();
    for (k, v) in draft.as_object().context("service must be an object")? {
        if k != "credentialRef" {
            service[k] = v.clone();
        }
    }
    if service["apiKey"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .is_empty()
    {
        service["apiKey"] = existing["apiKey"].clone();
    }
    let (url, key) = configuration.credentials(&service)?;
    if key.is_empty() {
        bail!("请填写 API 密钥")
    };
    let parsed = reqwest::Url::parse(&url)?;
    if !["http", "https"].contains(&parsed.scheme()) {
        bail!("接口地址必须是 HTTP(S) URL")
    }
    let style = service["apiStyle"].as_str().unwrap_or("openai");
    let anthropic = style == "anthropic";
    let responses = style == "openai_responses";
    let base = if anthropic && !url.trim_end_matches('/').ends_with("/v1") {
        format!("{}/v1", url.trim_end_matches('/'))
    } else {
        url.trim_end_matches('/').to_string()
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?;
    let probe = method == "testServiceConnection";
    let request = if probe {
        let models = service["models"].as_array();
        let model = p["modelId"]
            .as_str()
            .filter(|id| !id.trim().is_empty())
            .or_else(|| p["model"].as_str().filter(|id| !id.trim().is_empty()))
            .or_else(|| {
                models
                    .and_then(|models| models.first())
                    .and_then(|model| model["modelId"].as_str())
            })
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .context("请先输入或配置模型 ID，再测试连接")?;
        let configured =
            models.and_then(|models| models.iter().find(|entry| entry["modelId"] == model));
        if !anthropic
            && !responses
            && !crate::provider::chat_supported(
                model,
                configured
                    .map(|entry| &entry["parameterCompat"])
                    .unwrap_or(&Value::Null),
            )
        {
            bail!("该模型需要 Responses API，当前 Chat Completions 接口不支持它")
        }
        if responses {
            let body = json!({"model":model,"input":"Hi","max_output_tokens":16,"stream":false,"store":false});
            client.post(format!("{base}/responses")).json(&body)
        } else {
            let mut body =
                json!({"model":model,"messages":[{"role":"user","content":"Hi"}],"stream":false});
            let completion = !anthropic
                && configured
                    .and_then(|entry| entry["parameterCompat"]["maxCompletionTokens"].as_bool())
                    .unwrap_or_else(|| crate::provider::openai_reasoning_family(model).is_some());
            body[if completion {
                "max_completion_tokens"
            } else {
                "max_tokens"
            }] = json!(16);
            client
                .post(format!(
                    "{base}/{}",
                    if anthropic {
                        "messages"
                    } else {
                        "chat/completions"
                    }
                ))
                .json(&body)
        }
    } else {
        client.get(format!("{base}/models"))
    };
    let request = if anthropic {
        request
            .header("x-api-key", &key)
            .header("anthropic-version", "2023-06-01")
    } else {
        request.bearer_auth(&key)
    };
    let mut response = request.send().await.map_err(|error| {
        anyhow::anyhow!(
            "连接失败，请检查地址、网络和接口格式: {:#}",
            error.without_url()
        )
    })?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let mut bytes = Vec::new();
        while bytes.len() <= 4096 {
            let Ok(Some(chunk)) = response.chunk().await else {
                break;
            };
            let remaining = 4097usize.saturating_sub(bytes.len());
            bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        }
        let truncated = bytes.len() > 4096;
        bytes.truncate(4096);
        let mut body = String::from_utf8_lossy(&bytes).replace(&key, "[redacted]");
        if truncated {
            body.push_str("…[truncated]");
        }
        bail!("连接失败（HTTP {status}）：{body}")
    }
    if probe {
        return Ok(json!({"message":"连接成功","models":[]}));
    }
    let data: Value = response.json().await?;
    let models = data["data"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter(|model| model["id"].is_string())
        .map(discovered_model)
        .collect::<Vec<_>>();
    Ok(json!({"message":format!("已连接，发现 {} 个模型",models.len()),"models":models}))
}
