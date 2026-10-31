use crate::models::Usage;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};

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
    if !usage.available {
        return None;
    }
    Some(
        (usage
            .input_tokens
            .saturating_sub(usage.cache_read_tokens)
            .saturating_sub(usage.cache_write_tokens) as f64
            * rates["input"].as_f64()?
            + usage.output_tokens as f64 * rates["output"].as_f64()?
            + usage.cache_read_tokens as f64 * rates["cache_read"].as_f64()?
            + usage.cache_write_tokens as f64 * rates["cache_write"].as_f64()?)
            / 1_000_000.0,
    )
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
}
