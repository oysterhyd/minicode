use crate::{configuration::Configuration, models::Usage, store::Store};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};

const PRICING_ERROR: &str =
    "模型定价需要非负数字（input/output/cacheRead/cacheWrite，单位 USD / 百万 tokens）";

/// Per-million-token pricing in USD for one model.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Pricing {
    pub input: f64,
    pub output: f64,
    #[serde(alias = "cacheRead")]
    pub cache_read: f64,
    #[serde(alias = "cacheWrite")]
    pub cache_write: f64,
}

fn rate_field(
    object: &serde_json::Map<String, Value>,
    primary: &str,
    alias: Option<&str>,
    required: bool,
) -> Result<f64> {
    let raw = match object.get(primary) {
        Some(value) if !value.is_null() => Some(value),
        _ => alias
            .and_then(|key| object.get(key))
            .filter(|value| !value.is_null()),
    };
    let Some(raw) = raw else {
        if required {
            bail!("{PRICING_ERROR}");
        }
        return Ok(0.0);
    };
    raw.as_f64()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .context(PRICING_ERROR)
}

impl Pricing {
    /// Parse pricing metadata from camelCase or snake_case JSON (`Null` / `{}` -> `None`).
    pub fn from_value(value: &Value) -> Result<Option<Self>> {
        if value.is_null() {
            return Ok(None);
        }
        let object = value.as_object().context(PRICING_ERROR)?;
        if object.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            input: rate_field(object, "input", None, true)?,
            output: rate_field(object, "output", None, true)?,
            cache_read: rate_field(object, "cacheRead", Some("cache_read"), false)?,
            cache_write: rate_field(object, "cacheWrite", Some("cache_write"), false)?,
        }))
    }

    pub fn to_config_value(&self) -> Value {
        json!({
            "input": self.input,
            "output": self.output,
            "cacheRead": self.cache_read,
            "cacheWrite": self.cache_write,
        })
    }

    /// Compute turn cost in USD, returning `None` if usage was not reported.
    pub fn cost(&self, usage: &Usage) -> Option<f64> {
        if !usage.available {
            return None;
        }
        Some(
            (usage
                .input_tokens
                .saturating_sub(usage.cache_read_tokens)
                .saturating_sub(usage.cache_write_tokens) as f64
                * self.input
                + usage.output_tokens as f64 * self.output
                + usage.cache_read_tokens as f64 * self.cache_read
                + usage.cache_write_tokens as f64 * self.cache_write)
                / 1_000_000.0,
        )
    }
}

pub fn format_usd(usd: f64) -> String {
    if usd == 0.0 {
        "$0.00".into()
    } else if usd >= 0.01 {
        format!("${usd:.4}")
    } else {
        format!("${usd:.6}")
    }
}

pub fn prices(path: Option<&Path>) -> Result<Value> {
    let Some(path) = path else {
        return Ok(json!({}));
    };
    let prices: Value = serde_json::from_slice(&fs::read(path)?)?;
    for rates in prices
        .as_object()
        .context("prices must be a model-to-rates object")?
        .values()
    {
        for key in ["input", "output", "cache_read", "cache_write"] {
            if !rates[key]
                .as_f64()
                .is_some_and(|n| n.is_finite() && n >= 0.0)
            {
                bail!(
                    "prices require non-negative input/output/cache_read/cache_write USD per million tokens"
                );
            }
        }
    }
    Ok(prices)
}

pub fn cost(usage: &Usage, rates: &Value) -> Option<f64> {
    Pricing::from_value(rates)
        .ok()
        .flatten()
        .and_then(|pricing| pricing.cost(usage))
}

pub fn session_cost(
    store: &Store,
    configuration: &Configuration,
    session_id: &str,
    fallback_model: &str,
) -> Result<Option<f64>> {
    let (_, events) = store.statistics_events(session_id, -1)?;
    let mut requests = 0u64;
    let mut total = 0.0;
    for event in events {
        if event.r#type != "assistant_message" {
            continue;
        }
        requests += 1;
        let usage = Usage {
            input_tokens: event.data["usage"]["input_tokens"].as_u64().unwrap_or(0),
            output_tokens: event.data["usage"]["output_tokens"].as_u64().unwrap_or(0),
            cache_read_tokens: event.data["usage"]["cache_read_tokens"]
                .as_u64()
                .unwrap_or(0),
            cache_write_tokens: event.data["usage"]["cache_write_tokens"]
                .as_u64()
                .unwrap_or(0),
            available: event.data["usage"]["available"] != false,
        };
        let model = event.data["model"].as_str().unwrap_or(fallback_model);
        if model == "fake" || !usage.available {
            return Ok(None);
        }
        if let Some(recorded) = event.data["cost_usd"]
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)
        {
            total += recorded;
            continue;
        }
        let Some(pricing) = configuration.resolve_pricing(model)? else {
            return Ok(None);
        };
        let Some(turn_cost) = pricing.cost(&usage) else {
            return Ok(None);
        };
        total += turn_cost;
    }
    Ok((requests > 0).then_some(total))
}

fn moments(values: impl Iterator<Item = f64>) -> Value {
    let values = values.collect::<Vec<_>>();
    if values.is_empty() {
        return json!({"n":0,"mean":null,"variance":null});
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance = (values.len() > 1).then(|| {
        values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (values.len() - 1) as f64
    });
    json!({"n":values.len(),"mean":mean,"variance":variance})
}

fn aggregate(records: &[&Value]) -> Value {
    let mut group = json!({"model":records[0]["model"],"ablation":records[0]["ablation"],"baseline":records[0]["baseline"],"runs":records.len(),"passed":records.iter().filter(|r|r["passed"]==true).count()});
    for key in ["seconds", "input_tokens", "output_tokens", "cost_usd"] {
        group[key] = moments(records.iter().filter_map(|r| r[key].as_f64()));
    }
    group["success"] = moments(
        records
            .iter()
            .map(|r| if r["passed"] == true { 1.0 } else { 0.0 }),
    );
    let measured = records
        .iter()
        .filter(|r| r["usage_complete"] == true)
        .collect::<Vec<_>>();
    let input = measured
        .iter()
        .map(|r| r["usage"]["input_tokens"].as_u64().unwrap_or(0))
        .sum::<u64>();
    let cached = measured
        .iter()
        .map(|r| r["usage"]["cache_read_tokens"].as_u64().unwrap_or(0))
        .sum::<u64>();
    group["cache_hit_rate"] = if input == 0 {
        Value::Null
    } else {
        json!(cached as f64 / input as f64)
    };
    group["usage_samples"] = json!(measured.len());
    group
}

pub fn statistics(results: &[Value], config: &Value) -> Value {
    let mut groups: BTreeMap<(String, String, String), Vec<&Value>> = BTreeMap::new();
    let mut tasks: BTreeMap<(String, String, String, String), Vec<&Value>> = BTreeMap::new();
    for record in results {
        let text = |key| record[key].as_str().unwrap_or_default().to_string();
        groups
            .entry((text("model"), text("ablation"), text("baseline")))
            .or_default()
            .push(record);
        tasks
            .entry((
                text("model"),
                text("ablation"),
                text("baseline"),
                text("task"),
            ))
            .or_default()
            .push(record);
    }
    json!({"schema_version":1,"version":config["version"],"agent_sha256":config["agent_sha256"],"variance":"sample (n-1); null for fewer than two samples","groups":groups.values().map(|r|aggregate(r)).collect::<Vec<_>>(),"tasks":tasks.iter().map(|((_,_,_,task),r)|{let mut v=aggregate(r);v["task"]=json!(task);v}).collect::<Vec<_>>()})
}

pub fn summary(statistics: &Value) -> String {
    let mut summary = String::from(
        "# MiniCode evaluation\n\nVariance uses n-1. Cache hit rate is cached input / measured input. Unknown usage or prices produce null cost; offline usage is synthetic. See statistics.json for per-task repeats.\n\n| Model | Mechanism | Baseline | Pass / runs | Mean seconds | Variance seconds | Mean input | Variance input | Cache hit rate | Mean USD |\n| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n",
    );
    let number = |v: &Value| {
        v.as_f64()
            .map(|n| format!("{n:.6}"))
            .unwrap_or_else(|| "unknown".into())
    };
    for group in statistics["groups"].as_array().unwrap() {
        summary.push_str(&format!(
            "| {} | {} | {} | {} / {} | {} | {} | {} | {} | {} | {} |\n",
            group["model"].as_str().unwrap().replace('|', "\\|"),
            group["ablation"].as_str().unwrap(),
            group["baseline"].as_str().unwrap(),
            group["passed"],
            group["runs"],
            number(&group["seconds"]["mean"]),
            number(&group["seconds"]["variance"]),
            number(&group["input_tokens"]["mean"]),
            number(&group["input_tokens"]["variance"]),
            number(&group["cache_hit_rate"]),
            number(&group["cost_usd"]["mean"])
        ));
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn e3_sample_variance_and_unknown_cost_are_explicit() {
        let stats = moments([1.0, 2.0, 3.0].into_iter());
        assert_eq!(stats["mean"], 2.0);
        assert_eq!(stats["variance"], 1.0);
        assert!(moments([1.0].into_iter())["variance"].is_null());
        assert!(
            cost(
                &Usage {
                    available: true,
                    ..Default::default()
                },
                &Value::Null
            )
            .is_none()
        );
        assert!(moments(std::iter::empty())["mean"].is_null());
    }

    #[test]
    fn pricing_normalization_camel_and_snake_case_and_rejection() {
        assert_eq!(Pricing::from_value(&Value::Null).unwrap(), None);
        assert_eq!(Pricing::from_value(&json!({})).unwrap(), None);
        let camel =
            Pricing::from_value(&json!({"input": 1.25, "output": 10.0, "cacheRead": 0.125}))
                .unwrap()
                .unwrap();
        assert_eq!(
            camel,
            Pricing {
                input: 1.25,
                output: 10.0,
                cache_read: 0.125,
                cache_write: 0.0,
            }
        );
        let snake = Pricing::from_value(
            &json!({"input": 1.25, "output": 10.0, "cache_read": 0.125, "cache_write": 1.5}),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            snake.to_config_value(),
            json!({"input": 1.25, "output": 10.0, "cacheRead": 0.125, "cacheWrite": 1.5})
        );
        for invalid in [
            json!({"input": -1.0, "output": 2.0}),
            json!({"input": "1.0", "output": 2.0}),
            json!({"input": 1.0}),
            json!({"input": 1.0, "output": 2.0, "cacheRead": "free"}),
            json!("bad"),
        ] {
            assert!(
                Pricing::from_value(&invalid).is_err(),
                "expected rejection for {invalid}"
            );
        }
        assert_eq!(format_usd(0.0), "$0.00");
        assert_eq!(format_usd(0.000084), "$0.000084");
        assert_eq!(format_usd(0.123456), "$0.1235");
    }

    #[test]
    fn session_cost_sums_multi_model_turns_and_excludes_fake_or_unpriced() {
        let dir = tempfile::tempdir().unwrap();
        let configuration = Configuration {
            path: dir.path().join("config.json"),
        };
        configuration
            .write(&json!({
                "services": [{
                    "id": "svc",
                    "name": "Svc",
                    "enabled": true,
                    "apiStyle": "openai_responses",
                    "baseUrl": "https://example.test/v1",
                    "apiKey": "key",
                    "models": [
                        {
                            "modelId": "priced-a",
                            "contextWindow": 200000,
                            "maxOutputTokens": 8192,
                            "pricing": {"input": 1.0, "output": 2.0, "cacheRead": 0.1, "cacheWrite": 0.0}
                        },
                        {
                            "modelId": "priced-b",
                            "contextWindow": 200000,
                            "maxOutputTokens": 8192,
                            "pricing": {"input": 3.0, "output": 15.0, "cacheRead": 0.3, "cacheWrite": 3.75}
                        },
                        {
                            "modelId": "unpriced",
                            "contextWindow": 200000,
                            "maxOutputTokens": 8192
                        }
                    ]
                }],
                "agents": []
            }))
            .unwrap();
        let store = Store::open(dir.path().join("session.db")).unwrap();
        let session = store.create(".", "svc::priced-a", "commandcode").unwrap();
        store
            .event(
                &session,
                "assistant_message",
                &json!({
                    "model": "svc::priced-a",
                    "usage": {"input_tokens": 100, "output_tokens": 10, "cache_read_tokens": 40, "cache_write_tokens": 0, "available": true}
                }),
            )
            .unwrap();
        store
            .event(
                &session,
                "assistant_message",
                &json!({
                    "model": "svc::priced-b",
                    "usage": {"input_tokens": 200, "output_tokens": 20, "cache_read_tokens": 0, "cache_write_tokens": 100, "available": true}
                }),
            )
            .unwrap();
        // Turn 1: (60*1 + 10*2 + 40*0.1) / 1e6 = 0.000084
        // Turn 2: (100*3 + 20*15 + 100*3.75) / 1e6 = (300 + 300 + 375) / 1e6 = 0.000975
        // Total = 0.001059
        let total = session_cost(&store, &configuration, &session, "svc::priced-a")
            .unwrap()
            .unwrap();
        assert!((total - 0.001059).abs() < 1e-12, "got {total}");

        // Adding an unpriced turn makes session total cost unavailable (None).
        store
            .event(
                &session,
                "assistant_message",
                &json!({
                    "model": "svc::unpriced",
                    "usage": {"input_tokens": 50, "output_tokens": 10, "available": true}
                }),
            )
            .unwrap();
        assert_eq!(
            session_cost(&store, &configuration, &session, "svc::priced-a").unwrap(),
            None
        );

        // Fake session returns None even if another event had cost_usd.
        let fake_session = store.create(".", "fake", "fake").unwrap();
        store
            .event(
                &fake_session,
                "assistant_message",
                &json!({"model": "fake", "usage": {"input_tokens": 100, "output_tokens": 20, "available": true}}),
            )
            .unwrap();
        assert_eq!(
            session_cost(&store, &configuration, &fake_session, "fake").unwrap(),
            None
        );
    }
}
