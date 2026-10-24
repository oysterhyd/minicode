use crate::{
    attachments::Attachments,
    configuration::{Configuration, discover_commandcode},
    context,
    models::{Block, Message, Response, ToolSpec, Usage},
    tools::Sink,
    util,
};
use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    time::{Duration, Instant},
};

#[derive(Default)]
struct CallBuffer {
    id: String,
    name: String,
    arguments: String,
    initial: Value,
}

#[derive(Clone)]
pub struct Provider {
    pub name: String,
    pub model: String,
    pub configuration_id: String,
    pub style: String,
    pub url: String,
    key: String,
    pub window: u64,
    pub max_output: u64,
    pub modalities: Vec<String>,
    pub effort: String,
    pub prompt_scale: f64,
    pub fake: Option<Value>,
    pub index: usize,
    pub call_counter: usize,
    client: reqwest::Client,
}
impl Provider {
    pub fn new(model: &str, effort: &str, configuration: &Configuration) -> Result<Self> {
        let mut p = Self {
            name: "commandcode".into(),
            model: model.into(),
            configuration_id: model.into(),
            style: "openai".into(),
            url: String::new(),
            key: String::new(),
            window: 200_000,
            max_output: 8192,
            modalities: vec![],
            effort: effort.into(),
            prompt_scale: 1.0,
            fake: None,
            index: 0,
            call_counter: 0,
            // No total request timeout: it would also cap the streamed body.
            // Reads are bounded by idle_timeout and the run by its deadline.
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(20))
                .build()?,
        };
        if model == "fake" {
            p.name = "fake".into();
            p.modalities = vec!["image".into(), "pdf".into(), "audio".into(), "video".into()];
            p.fake = Some(
                if let Some(path) = std::env::var("MINICODE_FAKE_SCRIPT")
                    .ok()
                    .filter(|path| !path.trim().is_empty())
                {
                    let bytes = fs::read(path)?;
                    if bytes.len() > 16_000_000 {
                        bail!("fake script exceeds the size limit")
                    }
                    let v: Value = serde_json::from_slice(
                        bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes),
                    )?;
                    validate_fake(&v)?;
                    v
                } else {
                    json!({"turns":[],"exhausted_text":"(fake provider：未提供脚本，这里是一条演示回复。)"})
                },
            );
            return Ok(p);
        }
        let catalog: Value = serde_json::from_str(include_str!("assets/models.json"))?;
        if let Some((service, m)) = configuration.find_model(model)? {
            if service["enabled"] != true {
                bail!("model service is disabled")
            }
            p.style = util::string(&service, "apiStyle")?.into();
            p.name = if p.style == "anthropic" {
                "anthropic"
            } else {
                "commandcode"
            }
            .into();
            p.model = util::string(&m, "modelId")?.into();
            let (url, key) = configuration.credentials(&service);
            p.url = url;
            p.key = key;
            p.window = m["contextWindow"]
                .as_u64()
                .context("invalid context window")?;
            p.max_output = m["maxOutputTokens"]
                .as_u64()
                .context("invalid max output")?;
            for (key, kind) in [
                ("images", "image"),
                ("pdf", "pdf"),
                ("audio", "audio"),
                ("video", "video"),
            ] {
                if m["capabilities"][key] == true {
                    p.modalities.push(kind.into());
                }
            }
            if m["supportsEffort"] != true {
                p.effort = "off".into();
            }
        } else {
            let info = &catalog[model];
            if info.is_object() {
                p.window = info["context_window"].as_u64().unwrap_or(200_000);
                p.max_output = info["max_output_tokens"].as_u64().unwrap_or(8192);
                p.modalities =
                    serde_json::from_value(info["input_modalities"].clone()).unwrap_or_default();
            }
            if model.starts_with("claude") {
                p.style = "anthropic".into();
                p.name = "anthropic".into();
                p.url = std::env::var("ANTHROPIC_BASE_URL")
                    .unwrap_or("https://api.anthropic.com".into());
                p.key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
            } else if let Some((url, key)) = discover_commandcode() {
                p.url = url;
                p.key = key;
            }
        }
        p.key = p.key.trim().to_string();
        p.url = p.url.trim().trim_end_matches('/').to_string();
        if p.style == "anthropic" {
            p.effort = "off".into();
        }
        if p.key.is_empty() || p.url.is_empty() {
            bail!("请在设置中配置、启用并选择一个可用的 AI 模型")
        }
        let url = reqwest::Url::parse(&p.url)?;
        if !["http", "https"].contains(&url.scheme()) || url.host_str().is_none() {
            bail!("接口地址必须是 HTTP(S) URL")
        };
        Ok(p)
    }
    pub fn identity(&self) -> Option<String> {
        self.fake.as_ref().map(|v|{
            let turns=v["turns"].as_array().unwrap_or(&vec![]).iter().map(|turn|{
                let calls=turn["tool_calls"].as_array().unwrap_or(&vec![]).iter().map(|call|json!({"name":call["name"],"arguments":call.get("arguments").cloned().unwrap_or(json!({})),"id":call["id"]})).collect::<Vec<_>>();
                let mut entry=json!({"text":turn["text"],"tool_calls":calls,"input_tokens":turn["input_tokens"],"output_tokens":turn["output_tokens"],"cache_read_tokens":turn["cache_read_tokens"],"cache_write_tokens":turn["cache_write_tokens"],"stop_reason":turn["stop_reason"]});
                // Optional failure keys join the identity only when present, so
                // scripts written before they existed keep their resume identity.
                for key in ["error","panic"] {
                    if let Some(value)=turn.get(key) {
                        entry[key]=value.clone();
                    }
                }
                entry
            }).collect::<Vec<_>>();
            util::hash(json!({"turns":turns,"exhausted_text":v["exhausted_text"].as_str().unwrap_or("(fake provider: script exhausted)"),"default_input_tokens":v["default_input_tokens"].as_u64().unwrap_or(100),"default_output_tokens":v["default_output_tokens"].as_u64().unwrap_or(20)}).to_string().as_bytes())
        })
    }
    pub fn set_script(&mut self, script: Value) -> Result<()> {
        validate_fake(&script)?;
        self.fake = Some(script);
        Ok(())
    }
    pub async fn stream(
        &mut self,
        system: &str,
        messages: &[Message],
        specs: &[ToolSpec],
        attachments: &Attachments,
        emit: &Sink,
    ) -> Result<(Response, Value)> {
        if let Some(script) = &self.fake {
            let turn=script["turns"].as_array().and_then(|v|v.get(self.index)).cloned().unwrap_or_else(||json!({"text":script["exhausted_text"].as_str().unwrap_or("(fake provider: script exhausted)")}));
            self.index += 1;
            // Scripted failures let tests drive retry, error and panic paths
            // without a network endpoint.
            if let Some(message) = turn["panic"].as_str() {
                panic!("fake provider panic: {message}");
            }
            if let Some(error) = turn["error"].as_str() {
                bail!("{error}");
            }
            let mut blocks = vec![];
            if let Some(text) = turn["text"].as_str().filter(|s| !s.is_empty()) {
                let chars = text.chars().collect::<Vec<_>>();
                for chunk in chars.chunks(40) {
                    emit(json!({"event":"text_delta","text":chunk.iter().collect::<String>()}));
                }
                blocks.push(Block::text(text));
            }
            if let Some(calls) = turn["tool_calls"].as_array() {
                for call in calls {
                    let id = call["id"].as_str().map(str::to_string).unwrap_or_else(|| {
                        self.call_counter += 1;
                        format!("fake_tool_{}", self.call_counter)
                    });
                    blocks.push(Block::ToolUse {
                        id,
                        name: util::string(call, "name")?.into(),
                        input: call.get("arguments").cloned().unwrap_or(json!({})),
                    });
                }
            }
            let has_calls = blocks.iter().any(|b| matches!(b, Block::ToolUse { .. }));
            let usage = Usage {
                input_tokens: turn["input_tokens"]
                    .as_u64()
                    .unwrap_or(script["default_input_tokens"].as_u64().unwrap_or(100)),
                output_tokens: turn["output_tokens"]
                    .as_u64()
                    .unwrap_or(script["default_output_tokens"].as_u64().unwrap_or(20)),
                cache_read_tokens: turn["cache_read_tokens"].as_u64().unwrap_or(0),
                cache_write_tokens: turn["cache_write_tokens"].as_u64().unwrap_or(0),
                available: true,
            };
            return Ok((
                Response {
                    blocks,
                    stop_reason: turn["stop_reason"]
                        .as_str()
                        .unwrap_or(if has_calls { "tool_use" } else { "end_turn" })
                        .into(),
                    usage,
                },
                json!({"request_seconds":0.0,"generation_seconds":null,"first_token_seconds":null,"reasoning":""}),
            ));
        }
        let body = self.payload(system, messages, specs, attachments)?;
        let endpoint = if self.style == "anthropic" {
            if self.url.trim_end_matches('/').ends_with("/v1") {
                format!("{}/messages", self.url.trim_end_matches('/'))
            } else {
                format!("{}/v1/messages", self.url.trim_end_matches('/'))
            }
        } else {
            format!("{}/chat/completions", self.url.trim_end_matches('/'))
        };
        let started = Instant::now();
        let mut request = self.client.post(endpoint).json(&body);
        if self.style == "anthropic" {
            request = request
                .header("x-api-key", &self.key)
                .header("anthropic-version", "2023-06-01");
        } else {
            request = request.bearer_auth(&self.key);
        }
        let limit = idle_timeout();
        let response = idle(limit, request.send())
            .await?
            .map_err(|_| anyhow::anyhow!("provider connection failed"))?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            bail!("provider HTTP {status}")
        }
        let sse = response
            .headers()
            .get("content-type")
            .is_some_and(|s| s.to_str().unwrap_or("").contains("text/event-stream"));
        if !sse {
            let mut stream = response.bytes_stream();
            let mut bytes = vec![];
            while let Some(chunk) = idle(limit, stream.next()).await? {
                bytes.extend_from_slice(&chunk?);
                if bytes.len() > 16_000_000 {
                    bail!("provider response exceeds the capture limit")
                }
            }
            let value: Value = serde_json::from_slice(&bytes)?;
            let r = if self.style == "anthropic" {
                parse_anthropic(&value)?
            } else {
                parse_openai(&value)?
            };
            for block in &r.blocks {
                if let Block::Text { text } = block {
                    emit(json!({"event":"text_delta","text":text}));
                }
            }
            return Ok((
                r,
                json!({"request_seconds":started.elapsed().as_secs_f64(),"generation_seconds":null,"first_token_seconds":null,"reasoning":""}),
            ));
        }
        let mut stream = response.bytes_stream();
        let mut decoder = crate::sse::Decoder::default();
        let mut captured = 0;
        let mut content = String::new();
        let mut reasoning = String::new();
        let mut calls = BTreeMap::<usize, CallBuffer>::new();
        let mut order = Vec::new();
        let mut texts = BTreeMap::<usize, String>::new();
        let mut response_bytes = 0usize;
        let mut stop = None;
        let mut ended = false;
        let mut usage = Usage {
            available: false,
            ..Usage::default()
        };
        let mut first = None;
        let mut last = None;
        'stream: while let Some(chunk) = idle(limit, stream.next()).await? {
            let bytes = chunk.map_err(|_| anyhow::anyhow!("provider stream interrupted"))?;
            captured += bytes.len();
            if captured > 64_000_000 {
                bail!("SSE stream exceeds the capture limit")
            }
            for event in decoder.push(&bytes)? {
                let data = event.data.trim();
                if data == "[DONE]" {
                    ended = true;
                    break 'stream;
                }
                if data.is_empty() {
                    continue;
                }
                let value: Value =
                    serde_json::from_str(data).context("invalid provider SSE JSON")?;
                let mut text = None;
                let mut thought = None;
                if self.style == "anthropic" {
                    match value["type"].as_str() {
                        Some("error") => bail!("provider stream error"),
                        Some("message_start") => {
                            let initial = &value["message"]["usage"];
                            usage = parse_usage(initial, "anthropic");
                        }
                        Some("content_block_start") => {
                            let index = value["index"]
                                .as_u64()
                                .context("invalid content block index")?
                                as usize;
                            let block = &value["content_block"];
                            match block["type"].as_str() {
                                Some("tool_use") => {
                                    order.push(index);
                                    let id = util::string(block, "id")?.to_string();
                                    let name = util::string(block, "name")?.to_string();
                                    response_bytes += id.len() + name.len();
                                    calls.insert(
                                        index,
                                        CallBuffer {
                                            id,
                                            name,
                                            initial: block["input"].clone(),
                                            ..CallBuffer::default()
                                        },
                                    );
                                }
                                Some("text") => {
                                    order.push(index);
                                    text = block["text"].as_str();
                                }
                                Some("thinking") => thought = block["thinking"].as_str(),
                                _ => {}
                            }
                        }
                        Some("content_block_delta") => {
                            let delta = &value["delta"];
                            match delta["type"].as_str() {
                                Some("text_delta") => text = delta["text"].as_str(),
                                Some("thinking_delta") => thought = delta["thinking"].as_str(),
                                Some("input_json_delta") => {
                                    let index =
                                        value["index"].as_u64().context("invalid tool index")?
                                            as usize;
                                    let call = calls
                                        .get_mut(&index)
                                        .context("tool delta without start")?;
                                    let piece = delta["partial_json"].as_str().unwrap_or_default();
                                    response_bytes += piece.len();
                                    call.arguments.push_str(piece);
                                }
                                _ => {}
                            }
                        }
                        Some("message_delta") => {
                            stop = value["delta"]["stop_reason"].as_str().map(str::to_string);
                            let u = parse_usage(&value["usage"], "anthropic");
                            usage.output_tokens = u.output_tokens;
                            usage.available |= u.available;
                        }
                        Some("message_stop") => ended = true,
                        _ => {}
                    }
                } else {
                    if value.get("error").is_some() {
                        bail!("provider stream error")
                    }
                    if value["usage"].is_object() {
                        usage = parse_usage(&value["usage"], "openai")
                    }
                    if let Some(choice) = value["choices"].as_array().and_then(|v| v.first()) {
                        if let Some(reason) = choice["finish_reason"].as_str() {
                            if reason == "content_filter" {
                                bail!("provider rejected the response: content_filter")
                            }
                            stop = Some(
                                match reason {
                                    "tool_calls" => "tool_use",
                                    "length" => "max_tokens",
                                    _ => "end_turn",
                                }
                                .into(),
                            );
                        }
                        let delta = &choice["delta"];
                        text = delta["content"].as_str();
                        thought = delta["reasoning_content"]
                            .as_str()
                            .or(delta["reasoning"].as_str());
                        if let Some(deltas) = delta["tool_calls"].as_array() {
                            for d in deltas {
                                let index = d["index"]
                                    .as_u64()
                                    .map(|x| x as usize)
                                    .unwrap_or(calls.len());
                                calls.entry(index).or_insert_with(|| {
                                    order.push(index);
                                    CallBuffer::default()
                                });
                                let c = calls.get_mut(&index).unwrap();
                                if let Some(id) = d["id"].as_str().filter(|s| !s.is_empty()) {
                                    response_bytes += id.len();
                                    c.id = id.into();
                                }
                                if let Some(name) =
                                    d["function"]["name"].as_str().filter(|s| !s.is_empty())
                                {
                                    response_bytes += name.len();
                                    c.name = name.into();
                                }
                                if let Some(piece) = d["function"]["arguments"].as_str() {
                                    response_bytes += piece.len();
                                    c.arguments.push_str(piece);
                                }
                                first.get_or_insert(started.elapsed().as_secs_f64());
                                last = Some(started.elapsed().as_secs_f64());
                            }
                        }
                    }
                }
                if let Some(text) = text.filter(|s| !s.is_empty()) {
                    first.get_or_insert(started.elapsed().as_secs_f64());
                    last = Some(started.elapsed().as_secs_f64());
                    response_bytes += text.len();
                    if self.style == "anthropic" {
                        texts
                            .entry(value["index"].as_u64().unwrap_or(0) as usize)
                            .or_default()
                            .push_str(text);
                    } else {
                        content.push_str(text);
                    }
                    emit(json!({"event":"text_delta","text":text}));
                }
                if let Some(thought) = thought.filter(|s| !s.is_empty()) {
                    first.get_or_insert(started.elapsed().as_secs_f64());
                    last = Some(started.elapsed().as_secs_f64());
                    response_bytes += thought.len();
                    reasoning.push_str(thought);
                    emit(json!({"event":"reasoning_delta","text":thought}));
                }
                if calls.len() > 128 || response_bytes > 16_000_000 {
                    bail!("provider response exceeds the capture limit")
                }
                if self.style == "anthropic" && ended {
                    break 'stream;
                }
            }
        }
        if !ended && stop.is_none() {
            bail!("provider stream ended without a final response")
        }
        let mut blocks = vec![];
        if !content.is_empty() {
            blocks.push(Block::text(content))
        }
        let truncated = stop.as_deref() == Some("max_tokens");
        for index in order {
            if let Some(text) = texts.remove(&index).filter(|s| !s.is_empty()) {
                blocks.push(Block::text(text));
            }
            if let Some(call) = calls.remove(&index).filter(|_| !truncated) {
                let input = if call.arguments.is_empty() {
                    if call.initial.is_object() {
                        call.initial
                    } else {
                        json!({})
                    }
                } else {
                    serde_json::from_str(&call.arguments)
                        .context("provider returned malformed tool arguments JSON")?
                };
                if !input.is_object() {
                    bail!("provider tool arguments must be an object")
                }
                blocks.push(Block::ToolUse {
                    id: call.id,
                    name: call.name,
                    input,
                });
            }
        }
        if truncated && blocks.is_empty() {
            blocks.push(Block::text("模型输出被截断，未执行任何工具调用。"));
        }
        let estimated = context::estimate(system, messages, specs);
        if usage.available && usage.input_tokens > 0 && estimated > 0 {
            let observed = (usage.input_tokens as f64 / estimated as f64).clamp(0.5, 3.0);
            self.prompt_scale = (self.prompt_scale + observed) / 2.0;
        }
        Ok((
            Response {
                blocks,
                stop_reason: stop.unwrap_or_else(|| "end_turn".into()),
                usage,
            },
            json!({"request_seconds":started.elapsed().as_secs_f64(),"generation_seconds":first.zip(last).filter(|(a,b)|b>a).map(|(a,b)|b-a),"first_token_seconds":first,"reasoning":reasoning}),
        ))
    }
    pub fn payload(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolSpec],
        attachments: &Attachments,
    ) -> Result<Value> {
        let mut chat = vec![];
        if self.style == "openai" && !system.is_empty() {
            chat.push(json!({"role":"system","content":system}));
        }
        for message in messages {
            let media = message
                .content
                .iter()
                .filter(|b| matches!(b, Block::Media { .. }))
                .cloned()
                .collect::<Vec<_>>();
            attachments.check(&media, &self.modalities, &self.style)?;
            if self.style == "anthropic" {
                let mut content = vec![];
                for b in &message.content {
                    match b {
                        Block::Text { text } => content.push(json!({"type":"text","text":text})),
                        Block::Media { .. } => {
                            content.push(media_part(b, attachments, &self.style)?)
                        }
                        Block::ToolUse { id, name, input } => content
                            .push(json!({"type":"tool_use","id":id,"name":name,"input":input})),
                        Block::ToolResult {
                            tool_use_id,
                            content: text,
                            is_error,
                            images,
                        } => {
                            attachments.check(images, &self.modalities, &self.style)?;
                            let result = if images.is_empty() {
                                Value::String(text.clone())
                            } else {
                                let mut parts = vec![json!({"type":"text","text":text})];
                                for image in images {
                                    parts.push(media_part(image, attachments, &self.style)?);
                                }
                                Value::Array(parts)
                            };
                            let mut entry = json!({"type":"tool_result","tool_use_id":tool_use_id,"content":null,"is_error":is_error});
                            entry["content"] = result;
                            content.push(entry);
                        }
                    }
                }
                let mut entry = json!({"role":message.role,"content":null});
                entry["content"] = Value::Array(content);
                chat.push(entry);
                continue;
            }
            if message.role == "assistant" {
                let text = message.text();
                let calls=message.content.iter().filter_map(|b|if let Block::ToolUse{id,name,input}=b{Some(json!({"id":id,"type":"function","function":{"name":name,"arguments":input.to_string()}}))}else{None}).collect::<Vec<_>>();
                let mut entry = json!({"role":"assistant","content":if text.is_empty(){Value::Null}else{json!(text)}});
                if !calls.is_empty() {
                    entry["tool_calls"] = Value::Array(calls)
                }
                chat.push(entry);
            } else {
                let text = message.text();
                let mut results = 0;
                let mut content = vec![];
                for block in &message.content {
                    if let Block::ToolResult {
                        tool_use_id,
                        content: result,
                        images,
                        ..
                    } = block
                    {
                        results += 1;
                        chat.push(
                            json!({"role":"tool","tool_call_id":tool_use_id,"content":result}),
                        );
                        if !images.is_empty() {
                            attachments.check(images, &self.modalities, &self.style)?;
                            content.push(json!({"type":"text","text":format!("Images read by tool call {tool_use_id}: {result}")}));
                            for image in images {
                                content.push(media_part(image, attachments, &self.style)?);
                            }
                        }
                    }
                }
                for b in media {
                    content.push(media_part(&b, attachments, &self.style)?);
                }
                if results == 0 || !text.is_empty() || !content.is_empty() {
                    let value = if content.is_empty() {
                        Value::String(text)
                    } else {
                        if !text.is_empty() {
                            content.push(json!({"type":"text","text":text}));
                        }
                        Value::Array(content)
                    };
                    let mut entry = json!({"role":"user","content":null});
                    entry["content"] = value;
                    chat.push(entry);
                }
            }
        }
        let available = self
            .window
            .saturating_sub(
                (context::estimate(system, messages, tools) as f64 * self.prompt_scale) as u64
                    + 1024,
            )
            .max(1);
        if self.style == "anthropic" {
            chat = normalize_anthropic(chat);
        }
        let mut body = json!({"model":self.model,"messages":null,"stream":true,"max_tokens":self.max_output.min(available)});
        body["messages"] = Value::Array(chat);
        if self.style == "anthropic" {
            body["system"] = json!(system);
            if !tools.is_empty() {
                body["tools"]=json!(tools.iter().map(|s|json!({"name":s.name,"description":s.description,"input_schema":s.input_schema})).collect::<Vec<_>>());
            }
        } else {
            body["stream_options"] = json!({"include_usage":true});
            if self.effort != "off" {
                body["reasoning_effort"] = json!(self.effort)
            }
            if !tools.is_empty() {
                body["tools"]=json!(tools.iter().map(|s|json!({"type":"function","function":{"name":s.name,"description":s.description,"parameters":s.input_schema}})).collect::<Vec<_>>());
            }
        }
        Ok(body)
    }
}
/// Longest silence tolerated while waiting for response headers or the next
/// body chunk. MINICODE_PROVIDER_IDLE_TIMEOUT_S overrides the default.
fn idle_timeout() -> Duration {
    std::env::var("MINICODE_PROVIDER_IDLE_TIMEOUT_S")
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s > 0.0)
        .and_then(|s| Duration::try_from_secs_f64(s).ok())
        .unwrap_or(Duration::from_secs(120))
}
async fn idle<F: std::future::Future>(limit: Duration, future: F) -> Result<F::Output> {
    tokio::time::timeout(limit, future).await.map_err(|_| {
        anyhow::anyhow!(
            "provider stream idle timeout: no data for {}s",
            limit.as_secs_f64()
        )
    })
}
/// Strict Anthropic endpoints (Bedrock, some gateways) reject adjacent
/// messages with the same role, while the runtime appends user notices right
/// after user tool results. Merge them on the wire only, so persisted history
/// stays untouched and sessions written before this fix still resume.
fn normalize_anthropic(chat: Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::with_capacity(chat.len());
    for mut message in chat {
        if let Some(previous) = out.last_mut().filter(|p| p["role"] == message["role"]) {
            if let (Some(into), Value::Array(from)) = (
                previous["content"].as_array_mut(),
                message["content"].take(),
            ) {
                into.extend(from);
            }
            continue;
        }
        out.push(message);
    }
    for message in out.iter_mut().filter(|m| m["role"] == "user") {
        if let Some(content) = message["content"].as_array_mut() {
            // Stable: tool results first, other blocks keep their order.
            content.sort_by_key(|block| block["type"] != "tool_result");
        }
    }
    out
}
fn media_part(b: &Block, store: &Attachments, style: &str) -> Result<Value> {
    let Block::Media {
        kind,
        mime_type,
        name,
        ..
    } = b
    else {
        bail!("not media")
    };
    let data = store.encoded(b)?;
    if style == "anthropic" {
        return Ok(
            json!({"type":if kind=="image"{"image"}else{"document"},"source":{"type":"base64","media_type":mime_type,"data":data}}),
        );
    }
    let uri = format!("data:{mime_type};base64,{data}");
    Ok(match kind.as_str() {
        "image" => json!({"type":"image_url","image_url":{"url":uri}}),
        "pdf" => json!({"type":"file","file":{"filename":name,"file_data":uri}}),
        "audio" => {
            json!({"type":"input_audio","input_audio":{"data":data,"format":if mime_type=="audio/wav"{"wav"}else{"mp3"}}})
        }
        _ => json!({"type":"video_url","video_url":{"url":uri}}),
    })
}
fn parse_usage(v: &Value, style: &str) -> Usage {
    if !v.is_object() {
        return Usage {
            available: false,
            ..Usage::default()
        };
    }
    if style == "anthropic" {
        let read = v["cache_read_input_tokens"].as_u64().unwrap_or(0);
        let write = v["cache_creation_input_tokens"].as_u64().unwrap_or(0);
        Usage {
            input_tokens: v["input_tokens"].as_u64().unwrap_or(0) + read + write,
            output_tokens: v["output_tokens"].as_u64().unwrap_or(0),
            cache_read_tokens: read,
            cache_write_tokens: write,
            available: true,
        }
    } else {
        Usage {
            input_tokens: v["prompt_tokens"].as_u64().unwrap_or(0),
            output_tokens: v["completion_tokens"].as_u64().unwrap_or(0),
            cache_read_tokens: v["prompt_tokens_details"]["cached_tokens"]
                .as_u64()
                .unwrap_or(0),
            cache_write_tokens: v["prompt_tokens_details"]["cache_creation_tokens"]
                .as_u64()
                .unwrap_or(0),
            available: true,
        }
    }
}
fn parse_openai(v: &Value) -> Result<Response> {
    let choice = v["choices"]
        .as_array()
        .and_then(|v| v.first())
        .context("missing provider choice")?;
    let message = &choice["message"];
    if choice["finish_reason"] == "content_filter"
        || message["refusal"].as_str().is_some_and(|s| !s.is_empty())
    {
        bail!("provider rejected the response")
    }
    let mut blocks = vec![];
    if let Some(text) = message["content"].as_str().filter(|s| !s.is_empty()) {
        blocks.push(Block::text(text))
    }
    let truncated = choice["finish_reason"] == "length";
    if let Some(calls) = message["tool_calls"].as_array().filter(|_| !truncated) {
        for call in calls {
            let input: Value = serde_json::from_str(util::string(&call["function"], "arguments")?)?;
            if !input.is_object() {
                bail!("tool arguments must be an object")
            };
            blocks.push(Block::ToolUse {
                id: util::string(call, "id")?.into(),
                name: util::string(&call["function"], "name")?.into(),
                input,
            });
        }
    }
    if truncated && blocks.is_empty() {
        blocks.push(Block::text("模型输出被截断，未执行任何工具调用。"));
    }
    Ok(Response {
        blocks,
        stop_reason: match choice["finish_reason"].as_str() {
            Some("tool_calls") => "tool_use",
            Some("length") => "max_tokens",
            _ => "end_turn",
        }
        .into(),
        usage: parse_usage(&v["usage"], "openai"),
    })
}
fn parse_anthropic(v: &Value) -> Result<Response> {
    let mut blocks = vec![];
    let truncated = v["stop_reason"] == "max_tokens";
    for b in v["content"]
        .as_array()
        .context("missing provider content")?
    {
        match b["type"].as_str() {
            Some("text") => blocks.push(Block::text(util::string(b, "text")?)),
            Some("tool_use") if !truncated => blocks.push(Block::ToolUse {
                id: util::string(b, "id")?.into(),
                name: util::string(b, "name")?.into(),
                input: b["input"].clone(),
            }),
            _ => {}
        }
    }
    if truncated && blocks.is_empty() {
        blocks.push(Block::text("模型输出被截断，未执行任何工具调用。"));
    }
    Ok(Response {
        blocks,
        stop_reason: v["stop_reason"].as_str().unwrap_or("end_turn").into(),
        usage: parse_usage(&v["usage"], "anthropic"),
    })
}
fn validate_fake(v: &Value) -> Result<()> {
    fn fields(v: &Value, keys: &[&str]) -> Result<()> {
        for key in v
            .as_object()
            .context("fake script entry must be an object")?
            .keys()
        {
            if !keys.contains(&key.as_str()) {
                bail!("脚本包含未知字段: {key}")
            }
        }
        Ok(())
    }
    fields(
        v,
        &[
            "turns",
            "exhausted_text",
            "default_input_tokens",
            "default_output_tokens",
        ],
    )?;
    let empty = vec![];
    let turns = match v.get("turns") {
        Some(turns) => turns.as_array().context("turns must be an array")?,
        None => &empty,
    };
    for turn in turns {
        fields(
            turn,
            &[
                "text",
                "tool_calls",
                "input_tokens",
                "output_tokens",
                "cache_read_tokens",
                "cache_write_tokens",
                "stop_reason",
                "error",
                "panic",
            ],
        )?;
        for key in ["error", "panic"] {
            if !turn[key].is_null() && !turn[key].is_string() {
                bail!("{key} must be a string")
            }
        }
        if let Some(calls) = turn["tool_calls"].as_array() {
            for c in calls {
                fields(c, &["name", "arguments", "id"])?;
                util::string(c, "name")?;
                if !c["arguments"].is_null() && !c["arguments"].is_object() {
                    bail!("tool arguments must be an object")
                }
            }
        }
    }
    Ok(())
}
pub fn validate_response(response: &Response, messages: &[Message]) -> Result<()> {
    if response.blocks.is_empty() {
        bail!("provider returned an empty assistant response")
    }
    let mut known = HashSet::new();
    for message in messages {
        for b in &message.content {
            if let Block::ToolUse { id, .. } = b {
                known.insert(id.clone());
            }
        }
    }
    let mut count = 0;
    for b in &response.blocks {
        match b {
            Block::ToolUse { id, name, input } => {
                count += 1;
                if id.trim().is_empty()
                    || id.len() > 256
                    || name.trim().is_empty()
                    || !known.insert(id.clone())
                    || !input.is_object()
                {
                    bail!("provider returned a missing or reused tool call id/name")
                }
            }
            Block::Text { .. } => {}
            _ => bail!("provider emitted an invalid assistant block"),
        }
    }
    if count > 128 || serde_json::to_vec(&response.blocks)?.len() > 16_000_000 {
        bail!("provider response exceeds the capture limit")
    };
    Ok(())
}
