//! Debugging tools for agents in their Chrome tab, as a person would use
//! Chrome's DevTools: performance (Core Web Vitals, what blocks and what is
//! slow), audits (accessibility, SEO, best practices), emulation (device,
//! network, CPU, colour scheme, place, time zone), mocked responses, memory
//! snapshots compared over time, and errors with their stacks mapped
//! through source maps to the workspace's files.
//!
//! Emulation and mocks change the user's own tab, so they last only while
//! the agent works in it: they end when Chrome's debugger lets go of the
//! idle tab (a few minutes after the agent's last action at most), when
//! the user presses Stop, or with `reset`/`clear`. The agent is told when
//! they ended.

use std::collections::HashMap;

use base64::Engine as _;

use super::*;
use crate::sourcemap::{SourceMap, tidy_source};

/// Emulation and mocks keep the debugger this long after the last action.
pub(super) const OVERRIDES_KEEP: Duration = Duration::from_secs(120);
/// The largest heap snapshot read, in bytes.
const MAX_HEAP: usize = 400 * 1024 * 1024;
/// The largest source map read, in bytes.
const MAX_SOURCE_MAP: usize = 24 * 1024 * 1024;

/// A mocked response for requests whose address matches `pattern`.
#[derive(Debug, Clone)]
pub(super) struct Mock {
    /// `*` matches anything, `?` one character (as Chrome's patterns).
    pub pattern: String,
    pub method: Option<String>,
    pub status: u16,
    pub body: String,
    pub content_type: String,
    pub headers: Vec<(String, String)>,
    pub delay_ms: u64,
    /// `failed`, `timed_out`, `offline` or `aborted`: the request fails.
    pub fail: Option<String>,
    /// Requests answered so far.
    pub hits: u32,
}

/// What a heap snapshot holds, by kind of object: count and bytes.
pub(super) type HeapSummary = HashMap<String, (u64, u64)>;

impl AgentChrome {
    /// Core Web Vitals and what slows the page, after reloading it (unless
    /// `reload` is false: then as it is now).
    pub fn performance(
        &mut self,
        who: &Who,
        reload: bool,
        no_cache: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let page = self.page(who, false, false, cx);
        cx.spawn(async move |this, cx| {
            let page = page.await?;
            begin(&this, &page, cx).await;
            let cdp = page.cdp.clone();
            cdp.call("Performance.enable", json!({})).await?;
            if reload {
                if no_cache {
                    cdp.call("Network.setCacheDisabled", json!({ "cacheDisabled": true }))
                        .await?;
                }
                guarded(&this, &page, cx, async {
                    cdp.call("Page.reload", json!({ "ignoreCache": no_cache }))
                        .await
                        .map(|_| ())
                })
                .await?;
                wait_until_ready(&this, &page, cx).await;
                // The largest paint and late shifts come after the load.
                cx.background_executor().timer(Duration::from_secs(3)).await;
                if no_cache {
                    cdp.call(
                        "Network.setCacheDisabled",
                        json!({ "cacheDisabled": false }),
                    )
                    .await
                    .ok();
                }
            }
            let vitals = evaluate_main(&cdp, PERFORMANCE).await?;
            let metrics = cdp.call("Performance.getMetrics", json!({})).await?;
            Ok(json!({ "text": performance_text(&vitals, &metrics, reload) }))
        })
    }

    /// Accessibility, SEO and best-practice problems on the page.
    pub fn audit(
        &mut self,
        who: &Who,
        categories: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let page = self.page(who, false, false, cx);
        cx.spawn(async move |this, cx| {
            let page = page.await?;
            begin(&this, &page, cx).await;
            let wanted = if categories.is_empty() {
                vec![
                    "accessibility".to_string(),
                    "seo".to_string(),
                    "best-practices".to_string(),
                ]
            } else {
                categories
            };
            let expression = format!("{AUDIT}({})", json!(wanted));
            let found = in_world(&this, &page, &expression, cx).await?;
            let console_errors = this.update(cx, |this, _| {
                this.tab(&page.target).map_or(0, |tab| {
                    tab.logs
                        .console
                        .iter()
                        .filter(|line| line.level == Level::Error)
                        .count()
                })
            })?;
            Ok(json!({ "text": audit_text(&found, console_errors) }))
        })
    }

    /// Emulates a device, network, CPU and more in the agent's tab, or
    /// resets it (`reset`).
    pub fn emulate(
        &mut self,
        who: &Who,
        spec: Value,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let page = self.page(who, false, false, cx);
        cx.spawn(async move |this, cx| {
            let page = page.await?;
            begin(&this, &page, cx).await;
            let cdp = page.cdp.clone();
            let mut done = Vec::new();
            if spec["reset"] == true {
                for (method, params) in [
                    ("Emulation.clearDeviceMetricsOverride", json!({})),
                    ("Emulation.setTouchEmulationEnabled", json!({ "enabled": false })),
                    ("Network.emulateNetworkConditions", json!({ "offline": false, "latency": 0, "downloadThroughput": -1, "uploadThroughput": -1 })),
                    ("Emulation.setCPUThrottlingRate", json!({ "rate": 1 })),
                    ("Emulation.setEmulatedMedia", json!({ "media": "", "features": [] })),
                    ("Emulation.clearGeolocationOverride", json!({})),
                    ("Emulation.setTimezoneOverride", json!({ "timezoneId": "" })),
                    ("Emulation.setLocaleOverride", json!({})),
                ] {
                    cdp.call(method, params).await.ok();
                }
                this.update(cx, |this, _| {
                    if let Some(tab) = this.tab_mut(&page.target) {
                        tab.emulating.clear();
                    }
                })?;
                return Ok(json!({ "text": "The tab is back to normal: no emulation." }));
            }
            let device = spec["device"].as_str();
            let preset = match device {
                Some("phone") => Some((390, 844, 3.0, true)),
                Some("tablet") => Some((820, 1180, 2.0, true)),
                Some("laptop") => Some((1366, 768, 1.0, false)),
                Some("desktop") => Some((1920, 1080, 1.0, false)),
                Some(other) => bail!("unknown device `{other}`: phone, tablet, laptop or desktop (or width and height)"),
                None => None,
            };
            let size = match (spec["width"].as_u64(), spec["height"].as_u64()) {
                (Some(width), Some(height)) => Some((
                    width.clamp(200, 4000) as i64,
                    height.clamp(200, 4000) as i64,
                    spec["scale"].as_f64().unwrap_or(1.0).clamp(1.0, 4.0),
                    spec["mobile"].as_bool().unwrap_or(width < 800),
                )),
                _ => preset,
            };
            if let Some((width, height, scale, mobile)) = size {
                cdp.call(
                    "Emulation.setDeviceMetricsOverride",
                    json!({ "width": width, "height": height, "deviceScaleFactor": scale, "mobile": mobile }),
                )
                .await?;
                cdp.call("Emulation.setTouchEmulationEnabled", json!({ "enabled": mobile }))
                    .await?;
                done.push(format!(
                    "{} {width}×{height} at {scale}×",
                    device.unwrap_or(if mobile { "mobile" } else { "screen" })
                ));
            }
            if let Some(network) = spec["network"].as_str() {
                let (offline, latency, down, up) = match network {
                    "offline" => (true, 0, 0, 0),
                    "slow-3g" => (false, 400, 50_000, 50_000),
                    "fast-3g" => (false, 150, 200_000, 93_750),
                    "4g" => (false, 60, 1_125_000, 187_500),
                    "none" => (false, 0, -1, -1),
                    other => bail!("unknown network `{other}`: offline, slow-3g, fast-3g, 4g or none"),
                };
                cdp.call(
                    "Network.emulateNetworkConditions",
                    json!({ "offline": offline, "latency": latency, "downloadThroughput": down, "uploadThroughput": up }),
                )
                .await?;
                done.push(format!("{network} network"));
            }
            if let Some(rate) = spec["cpu"].as_f64() {
                let rate = rate.clamp(1.0, 20.0);
                cdp.call("Emulation.setCPUThrottlingRate", json!({ "rate": rate }))
                    .await?;
                done.push(format!("CPU {rate}× slower"));
            }
            let mut features = Vec::new();
            if let Some(scheme) = spec["color_scheme"].as_str() {
                features.push(json!({ "name": "prefers-color-scheme", "value": scheme }));
                done.push(format!("{scheme} colour scheme"));
            }
            if spec["reduced_motion"] == true {
                features.push(json!({ "name": "prefers-reduced-motion", "value": "reduce" }));
                done.push("reduced motion".into());
            }
            if !features.is_empty() {
                cdp.call("Emulation.setEmulatedMedia", json!({ "features": features }))
                    .await?;
            }
            if let (Some(latitude), Some(longitude)) = (
                spec["geolocation"]["latitude"].as_f64(),
                spec["geolocation"]["longitude"].as_f64(),
            ) {
                cdp.call(
                    "Emulation.setGeolocationOverride",
                    json!({ "latitude": latitude, "longitude": longitude, "accuracy": 50 }),
                )
                .await?;
                done.push(format!("location {latitude:.4}, {longitude:.4}"));
            }
            if let Some(zone) = spec["timezone"].as_str() {
                cdp.call("Emulation.setTimezoneOverride", json!({ "timezoneId": zone }))
                    .await?;
                done.push(format!("time zone {zone}"));
            }
            if let Some(locale) = spec["locale"].as_str() {
                cdp.call("Emulation.setLocaleOverride", json!({ "locale": locale }))
                    .await?;
                done.push(format!("locale {locale}"));
            }
            if done.is_empty() {
                bail!("say what to emulate: device (or width and height), network, cpu, color_scheme, reduced_motion, geolocation, timezone or locale; or reset");
            }
            this.update(cx, |this, _| {
                if let Some(tab) = this.tab_mut(&page.target) {
                    tab.emulating = done.clone();
                    tab.keep_until = Some(Instant::now() + OVERRIDES_KEEP);
                }
            })?;
            Ok(json!({ "text": format!(
                "Emulating {}. It lasts while you work in this tab and ends by itself a few minutes after your last action (or with reset). Reload (chrome_navigate) if the page should load under it; chrome_screenshot shows it.",
                done.join(", ")
            ) }))
        })
    }

    /// Mocked responses: `add` one, `list` them, or `clear` them all.
    pub fn mock(&mut self, who: &Who, spec: Value, cx: &mut Context<Self>) -> Task<Result<Value>> {
        let page = self.page(who, false, false, cx);
        cx.spawn(async move |this, cx| {
            let page = page.await?;
            begin(&this, &page, cx).await;
            let action = spec["action"].as_str().unwrap_or("add");
            let mocks = match action {
                "list" => {
                    let text = this.update(cx, |this, _| {
                        this.tab(&page.target).map(|tab| mocks_text(&tab.mocks)).unwrap_or_default()
                    })?;
                    return Ok(json!({ "text": text }));
                }
                "clear" => {
                    this.update(cx, |this, _| {
                        if let Some(tab) = this.tab_mut(&page.target) {
                            tab.mocks.clear();
                        }
                    })?;
                    page.cdp.call("Fetch.disable", json!({})).await.ok();
                    return Ok(json!({ "text": "No request is mocked now." }));
                }
                "add" => {
                    let pattern = spec["url"]
                        .as_str()
                        .filter(|url| !url.trim().is_empty())
                        .context("`url`: the addresses to answer, with * for any part (`*/api/orders*`)")?
                        .to_string();
                    let fail = spec["fail"].as_str().map(str::to_string);
                    if let Some(reason) = &fail
                        && !["failed", "timed_out", "offline", "aborted"].contains(&reason.as_str())
                    {
                        bail!("`fail`: failed, timed_out, offline or aborted");
                    }
                    let body = match &spec["body"] {
                        Value::String(text) => text.clone(),
                        Value::Null => String::new(),
                        other => other.to_string(),
                    };
                    let content_type = spec["content_type"].as_str().map(str::to_string).unwrap_or_else(|| {
                        if spec["body"].is_object() || spec["body"].is_array() {
                            "application/json".into()
                        } else {
                            "text/plain; charset=utf-8".into()
                        }
                    });
                    let headers = spec["headers"]
                        .as_object()
                        .map(|headers| {
                            headers
                                .iter()
                                .map(|(name, value)| (name.clone(), value.as_str().map_or_else(|| value.to_string(), str::to_string)))
                                .collect()
                        })
                        .unwrap_or_default();
                    let mock = Mock {
                        pattern,
                        method: spec["method"].as_str().map(|method| method.to_ascii_uppercase()),
                        status: spec["status"].as_u64().unwrap_or(200).clamp(100, 599) as u16,
                        body,
                        content_type,
                        headers,
                        delay_ms: spec["delay_ms"].as_u64().unwrap_or(0).min(60_000),
                        fail,
                        hits: 0,
                    };
                    this.update(cx, |this, _| {
                        let tab = this.tab_mut(&page.target)?;
                        tab.mocks.retain(|known| known.pattern != mock.pattern || known.method != mock.method);
                        tab.mocks.push(mock);
                        tab.keep_until = Some(Instant::now() + OVERRIDES_KEEP);
                        Some(tab.mocks.clone())
                    })?
                    .context("your tab closed")?
                }
                other => bail!("unknown action `{other}`: add, list or clear"),
            };
            let patterns: Vec<Value> = mocks
                .iter()
                .map(|mock| json!({ "urlPattern": mock.pattern, "requestStage": "Request" }))
                .collect();
            page.cdp
                .call("Fetch.enable", json!({ "patterns": patterns }))
                .await?;
            Ok(json!({ "text": format!(
                "{}\nMocks last while you work in this tab and end by themselves a few minutes after your last action (or with clear). Reload or act on the page to use them.",
                mocks_text(&mocks)
            ) }))
        })
    }

    /// A request the page made that a mock may answer (`Fetch.requestPaused`).
    pub(super) fn answer_paused(&mut self, index: usize, params: &Value, cx: &mut Context<Self>) {
        let tab = &mut self.tabs[index];
        let Some(id) = params["requestId"].as_str().map(str::to_string) else {
            return;
        };
        let url = params["request"]["url"].as_str().unwrap_or_default();
        let method = params["request"]["method"].as_str().unwrap_or("GET");
        let cdp = tab.cdp.clone();
        let found = tab.mocks.iter_mut().find(|mock| {
            matches_pattern(&mock.pattern, url)
                && mock.method.as_deref().is_none_or(|wanted| wanted == method)
        });
        let Some(mock) = found else {
            cx.background_spawn(async move {
                cdp.call("Fetch.continueRequest", json!({ "requestId": id }))
                    .await
                    .ok();
            })
            .detach();
            return;
        };
        mock.hits += 1;
        let mock = mock.clone();
        let delay = Duration::from_millis(mock.delay_ms);
        cx.spawn(async move |_, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let call = match &mock.fail {
                Some(reason) => {
                    let reason = match reason.as_str() {
                        "timed_out" => "TimedOut",
                        "offline" => "InternetDisconnected",
                        "aborted" => "Aborted",
                        _ => "Failed",
                    };
                    cdp.call("Fetch.failRequest", json!({ "requestId": id, "errorReason": reason }))
                        .await
                }
                None => {
                    let mut headers = vec![json!({ "name": "Content-Type", "value": mock.content_type })];
                    headers.extend(mock.headers.iter().map(|(name, value)| json!({ "name": name, "value": value })));
                    let body = base64::engine::general_purpose::STANDARD.encode(mock.body.as_bytes());
                    cdp.call(
                        "Fetch.fulfillRequest",
                        json!({ "requestId": id, "responseCode": mock.status, "responseHeaders": headers, "body": body }),
                    )
                    .await
                }
            };
            call.ok();
        })
        .detach();
    }

    /// A heap snapshot of the page, by kind of object; with `compare`, what
    /// grew since the previous one (for leaks).
    pub fn memory(
        &mut self,
        who: &Who,
        compare: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let page = self.page(who, false, false, cx);
        cx.spawn(async move |this, cx| {
            let page = page.await?;
            begin(&this, &page, cx).await;
            let cdp = page.cdp.clone();
            cdp.call("HeapProfiler.enable", json!({})).await?;
            cdp.call("HeapProfiler.collectGarbage", json!({})).await.ok();
            this.update(cx, |this, _| {
                if let Some(tab) = this.tab_mut(&page.target) {
                    tab.heap = Some(String::new());
                    tab.heap_too_large = false;
                }
            })?;
            let taken = cdp
                .call("HeapProfiler.takeHeapSnapshot", json!({ "reportProgress": false }))
                .await;
            let (text, previous) = this.update(cx, |this, _| {
                this.tab_mut(&page.target)
                    .map(|tab| (tab.heap.take().unwrap_or_default(), tab.heap_summary.clone()))
                    .unwrap_or_default()
            })?;
            cdp.call("HeapProfiler.disable", json!({})).await.ok();
            taken?;
            if text.is_empty() {
                bail!("the heap snapshot was too large to read here ({} MB at most); use Chrome DevTools' Memory panel", MAX_HEAP / 1024 / 1024);
            }
            let summary = cx
                .background_spawn(async move { heap_summary(&text) })
                .await?;
            let report = memory_text(&summary, previous.as_ref().filter(|_| compare));
            this.update(cx, |this, _| {
                if let Some(tab) = this.tab_mut(&page.target) {
                    tab.heap_summary = Some(summary);
                }
            })?;
            Ok(json!({ "text": report }))
        })
    }

    /// A heap snapshot's next chunk (`HeapProfiler.addHeapSnapshotChunk`).
    pub(super) fn heap_chunk(&mut self, index: usize, params: &Value) {
        let tab = &mut self.tabs[index];
        if let Some(heap) = tab.heap.as_mut() {
            let chunk = params["chunk"].as_str().unwrap_or_default();
            if heap.len() + chunk.len() > MAX_HEAP {
                tab.heap = Some(String::new());
                tab.heap_too_large = true;
            } else if !tab.heap_too_large {
                heap.push_str(chunk);
            }
        }
    }

    /// The page's errors, newest last, with their stacks mapped through
    /// source maps to the original files (the workspace's, when there).
    pub fn errors(
        &mut self,
        who: &Who,
        project_id: Option<i64>,
        limit: usize,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let roots = self.allowed_folders(project_id);
        let lines: Vec<(String, Option<String>, Option<String>)> = match self.current_tab(who) {
            Ok(tab) => tab
                .logs
                .console
                .iter()
                .filter(|line| line.level == Level::Error)
                .rev()
                .take(limit)
                .map(|line| (line.text.clone(), line.source.clone(), line.stack.clone()))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
            Err(err) => return Task::ready(Err(err)),
        };
        if lines.is_empty() {
            return Task::ready(Ok(
                json!({ "text": "No errors in your tab's console since you opened it." }),
            ));
        }
        let page = self.page(who, false, false, cx);
        cx.spawn(async move |_, cx| {
            let page = page.await?;
            // The scripts the stacks name, and their maps (read by the page,
            // as its own requests would be).
            let mut scripts: Vec<String> = Vec::new();
            for (_, source, stack) in &lines {
                for place in source
                    .iter()
                    .chain(stack.iter())
                    .flat_map(|text| places(text))
                {
                    if !scripts.contains(&place.0) && scripts.len() < 12 {
                        scripts.push(place.0);
                    }
                }
            }
            let mut maps: HashMap<String, SourceMap> = HashMap::new();
            for script in scripts {
                if let Ok(Some(map)) = source_map_of(&page.cdp, &script).await {
                    let parsed = cx
                        .background_spawn(async move { SourceMap::parse(&map) })
                        .await;
                    if let Some(parsed) = parsed {
                        maps.insert(script, parsed);
                    }
                }
            }
            let mapped = |text: &str| map_places(text, &maps, &roots);
            let mut out = Vec::new();
            for (text, source, stack) in &lines {
                let mut entry = format!("[error] {}", mapped(text));
                if let Some(source) = source {
                    entry.push_str(&format!("  ({})", mapped(source)));
                }
                if let Some(stack) = stack {
                    for frame in stack.lines().take(10) {
                        entry.push_str(&format!("\n    {}", mapped(frame)));
                    }
                }
                out.push(entry);
            }
            let note = if maps.is_empty() {
                "\n(No source maps were found: places are in the built scripts.)"
            } else {
                ""
            };
            Ok(json!({ "text": format!("{}{note}", out.join("\n")) }))
        })
    }
}

/// Whether `url` matches a Chrome-style pattern (`*` any run, `?` one).
pub(super) fn matches_pattern(pattern: &str, url: &str) -> bool {
    fn go(pattern: &[u8], text: &[u8]) -> bool {
        match pattern.split_first() {
            None => text.is_empty(),
            Some((b'*', rest)) => (0..=text.len()).any(|skip| go(rest, &text[skip..])),
            Some((b'?', rest)) => !text.is_empty() && go(rest, &text[1..]),
            Some((byte, rest)) => text.first() == Some(byte) && go(rest, &text[1..]),
        }
    }
    // Collapse runs of `*`, which would only slow the match.
    let mut pattern = pattern.to_string();
    while pattern.contains("**") {
        pattern = pattern.replace("**", "*");
    }
    go(pattern.as_bytes(), url.as_bytes())
}

fn mocks_text(mocks: &[Mock]) -> String {
    if mocks.is_empty() {
        return "No request is mocked.".into();
    }
    let lines: Vec<String> = mocks
        .iter()
        .map(|mock| {
            let answer = match &mock.fail {
                Some(reason) => format!("fails ({reason})"),
                None => format!(
                    "{} {} ({} bytes)",
                    mock.status,
                    mock.content_type,
                    mock.body.len()
                ),
            };
            let delay = if mock.delay_ms > 0 {
                format!(" after {} ms", mock.delay_ms)
            } else {
                String::new()
            };
            format!(
                "- {} {} → {answer}{delay}; answered {} time(s)",
                mock.method.as_deref().unwrap_or("any"),
                mock.pattern,
                mock.hits
            )
        })
        .collect();
    format!("Mocked requests:\n{}", lines.join("\n"))
}

/// The `url:line:column` places a text names (http, https and file).
fn places(text: &str) -> Vec<(String, u32, u32)> {
    let mut out = Vec::new();
    for word in text.split(|c: char| c.is_whitespace() || c == '(' || c == ')') {
        if !(word.starts_with("http://")
            || word.starts_with("https://")
            || word.starts_with("file://"))
        {
            continue;
        }
        let mut parts = word.rsplitn(3, ':');
        let (Some(column), Some(line), Some(url)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if let (Ok(line), Ok(column)) = (line.parse(), column.parse()) {
            out.push((url.to_string(), line, column));
        }
    }
    out
}

/// `text` with each place in a mapped script replaced by its original
/// (the workspace's file when one of `roots` has it).
fn map_places(text: &str, maps: &HashMap<String, SourceMap>, roots: &[PathBuf]) -> String {
    let mut out = text.to_string();
    for (url, line, column) in places(text) {
        let Some(original) = maps.get(&url).and_then(|map| map.lookup(line, column)) else {
            continue;
        };
        let source = tidy_source(&original.source);
        let in_workspace = roots.iter().any(|root| root.join(&source).is_file());
        let replacement = format!(
            "{source}:{}:{}{}",
            original.line,
            original.column,
            if in_workspace {
                ""
            } else {
                " (not in the workspace)"
            }
        );
        out = out.replace(&format!("{url}:{line}:{column}"), &replacement);
    }
    out
}

/// The source map of the script at `url`, read by the page itself (its
/// own address, cookies and permissions), when it names one.
async fn source_map_of(cdp: &Cdp, url: &str) -> Result<Option<String>> {
    let expression = format!(
        r#"(async () => {{
  const script = {url};
  const text = await (await fetch(script, {{ credentials: 'include' }})).text();
  const found = /[#@] sourceMappingURL=([^\s'"]+)\s*$/.exec(text.slice(-4000));
  if (!found) return null;
  const where = new URL(found[1], script).href;
  if (where.startsWith('data:')) {{
    const comma = where.indexOf(',');
    const data = where.slice(comma + 1);
    return where.slice(0, comma).endsWith(';base64') ? atob(data) : decodeURIComponent(data);
  }}
  const map = await fetch(where, {{ credentials: 'include' }});
  if (!map.ok) return null;
  const body = await map.text();
  return body.length > {MAX_SOURCE_MAP} ? null : body;
}})()"#,
        url = json!(url),
    );
    let value = evaluate_main(cdp, &expression).await?;
    Ok(value.as_str().map(str::to_string))
}

/// Counts and bytes by kind of object in a heap snapshot's JSON.
fn heap_summary(text: &str) -> Result<HeapSummary> {
    let snapshot: Value = serde_json::from_str(text).context("the heap snapshot did not read")?;
    let meta = &snapshot["snapshot"]["meta"];
    let fields: Vec<&str> = meta["node_fields"]
        .as_array()
        .context("no node fields")?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let types: Vec<&str> = meta["node_types"][0]
        .as_array()
        .context("no node types")?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let at = |name: &str| fields.iter().position(|field| *field == name);
    let (type_at, name_at, size_at) = (
        at("type").context("no type field")?,
        at("name").context("no name field")?,
        at("self_size").context("no size field")?,
    );
    let detached_at = at("detachedness");
    let strings: Vec<&str> = snapshot["strings"]
        .as_array()
        .context("no strings")?
        .iter()
        .map(|value| value.as_str().unwrap_or_default())
        .collect();
    let nodes = snapshot["nodes"].as_array().context("no nodes")?;
    let width = fields.len();
    let mut summary: HeapSummary = HashMap::new();
    for node in nodes.chunks(width) {
        let kind = types
            .get(node[type_at].as_u64().unwrap_or(0) as usize)
            .copied()
            .unwrap_or("?");
        let size = node[size_at].as_u64().unwrap_or(0);
        let name = strings
            .get(node[name_at].as_u64().unwrap_or(0) as usize)
            .copied()
            .unwrap_or("");
        let key = match kind {
            "object" | "native" => {
                let detached = detached_at.is_some_and(|at| node[at].as_u64() == Some(2));
                let name: String = name
                    .split(' ')
                    .next()
                    .unwrap_or(name)
                    .chars()
                    .take(60)
                    .collect();
                if detached {
                    format!("Detached {name}")
                } else {
                    name
                }
            }
            "closure" => "(closures)".into(),
            "string" | "concatenated string" | "sliced string" => "(strings)".into(),
            "array" => "(internal arrays)".into(),
            "code" => "(compiled code)".into(),
            other => format!("({other})"),
        };
        let entry = summary.entry(key).or_default();
        entry.0 += 1;
        entry.1 += size;
    }
    Ok(summary)
}

fn memory_text(summary: &HeapSummary, previous: Option<&HeapSummary>) -> String {
    let total: u64 = summary.values().map(|(_, bytes)| bytes).sum();
    let mut lines = vec![format!(
        "Heap after garbage collection: {}",
        human_size(total)
    )];
    let detached: u64 = summary
        .iter()
        .filter(|(name, _)| name.starts_with("Detached "))
        .map(|(_, (count, _))| count)
        .sum();
    if detached > 0 {
        lines.push(format!(
            "{detached} detached DOM nodes (removed from the page but still referenced: a common leak)."
        ));
    }
    let mut biggest: Vec<_> = summary.iter().collect();
    biggest.sort_by_key(|(_, (_, bytes))| std::cmp::Reverse(*bytes));
    lines.push("\nLargest by kind (count, bytes):".into());
    for (name, (count, bytes)) in biggest.into_iter().take(20) {
        lines.push(format!("- {name}: {count}, {}", human_size(*bytes)));
    }
    if let Some(previous) = previous {
        let mut grew: Vec<(String, i64, i64)> = summary
            .iter()
            .map(|(name, (count, bytes))| {
                let (before_count, before_bytes) = previous.get(name).copied().unwrap_or_default();
                (
                    name.clone(),
                    *count as i64 - before_count as i64,
                    *bytes as i64 - before_bytes as i64,
                )
            })
            .filter(|(_, count, bytes)| *count > 0 || *bytes > 0)
            .collect();
        grew.sort_by_key(|grown| std::cmp::Reverse(grown.2));
        let before: u64 = previous.values().map(|(_, bytes)| bytes).sum();
        lines.push(format!(
            "\nSince the previous snapshot: {} → {} ({:+} bytes). Grew most (count, bytes):",
            human_size(before),
            human_size(total),
            total as i64 - before as i64
        ));
        if grew.is_empty() {
            lines.push("- nothing grew".into());
        }
        for (name, count, bytes) in grew.into_iter().take(15) {
            lines.push(format!("- {name}: {count:+}, {bytes:+} bytes"));
        }
    } else {
        lines.push("\nDo what might leak (open and close a dialog a few times), then call chrome_memory with compare: true to see what grew.".into());
    }
    lines.join("\n")
}

fn performance_text(vitals: &Value, metrics: &Value, reloaded: bool) -> String {
    let ms = |value: &Value| value.as_f64().map(|ms| ms.round());
    let rate = |value: Option<f64>, good: f64, poor: f64| match value {
        Some(value) if value <= good => "good",
        Some(value) if value <= poor => "needs improvement",
        Some(_) => "poor",
        None => "not measured",
    };
    let mut lines = vec![format!(
        "Performance of {}{}",
        vitals["url"].as_str().unwrap_or_default(),
        if reloaded {
            " (reloaded to measure)"
        } else {
            " (as it is now)"
        }
    )];
    let lcp = ms(&vitals["lcp"]["ms"]);
    let cls = vitals["cls"].as_f64();
    let inp = ms(&vitals["inp"]);
    let fcp = ms(&vitals["fcp"]);
    let ttfb = ms(&vitals["ttfb"]);
    lines.push("\nCore Web Vitals:".into());
    lines.push(format!(
        "- Largest Contentful Paint: {} ({}){}",
        lcp.map_or("—".into(), |ms| format!("{ms} ms")),
        rate(lcp, 2500.0, 4000.0),
        vitals["lcp"]["element"]
            .as_str()
            .map(|element| format!(", the element: {element}"))
            .unwrap_or_default()
    ));
    lines.push(format!(
        "- Cumulative Layout Shift: {} ({})",
        cls.map_or("—".into(), |cls| format!("{cls}")),
        rate(cls, 0.1, 0.25)
    ));
    lines.push(format!(
        "- Interaction to Next Paint: {} ({})",
        inp.map_or("— (no interaction yet)".into(), |ms| format!("{ms} ms")),
        if inp.is_some() {
            rate(inp, 200.0, 500.0)
        } else {
            "click or type, then ask again"
        }
    ));
    lines.push(format!(
        "- First Contentful Paint: {} ({}); time to first byte: {} ({})",
        fcp.map_or("—".into(), |ms| format!("{ms} ms")),
        rate(fcp, 1800.0, 3000.0),
        ttfb.map_or("—".into(), |ms| format!("{ms} ms")),
        rate(ttfb, 800.0, 1800.0)
    ));
    if let Some(load) = ms(&vitals["load"]) {
        lines.push(format!(
            "- DOM content loaded {} ms, load {} ms",
            ms(&vitals["domContentLoaded"]).unwrap_or(0.0),
            load
        ));
    }
    let list = |key: &str| vitals[key].as_array().cloned().unwrap_or_default();
    let shifts = list("shiftSources");
    if !shifts.is_empty() {
        lines.push("\nWhat shifted:".into());
        for shift in shifts {
            lines.push(format!(
                "- {} ({})",
                shift["selector"].as_str().unwrap_or("?"),
                shift["value"]
            ));
        }
    }
    let frames = list("longFrames");
    if !frames.is_empty() {
        lines.push("\nLong animation frames (main thread busy), with the scripts in them:".into());
        for frame in frames {
            lines.push(format!(
                "- {} ms (blocking {} ms)",
                frame["ms"], frame["blocking"]
            ));
            for script in frame["scripts"].as_array().into_iter().flatten() {
                lines.push(format!(
                    "    {} ms {} ({})",
                    script["ms"],
                    script["source"].as_str().unwrap_or("?"),
                    script["invoker"].as_str().unwrap_or("")
                ));
            }
        }
    }
    let blocking = list("blocking");
    if !blocking.is_empty() {
        lines.push("\nRender-blocking resources:".into());
        lines.extend(
            blocking
                .iter()
                .filter_map(Value::as_str)
                .map(|url| format!("- {url}")),
        );
    }
    lines.push(format!(
        "\n{} requests, {} KB transferred, {} DOM elements.",
        vitals["requests"], vitals["totalKb"], vitals["domNodes"]
    ));
    for (title, key) in [("Slowest requests", "slow"), ("Heaviest requests", "heavy")] {
        let items = list(key);
        if items.is_empty() {
            continue;
        }
        lines.push(format!("\n{title}:"));
        for item in items {
            lines.push(format!(
                "- {} ms, {} KB, {}: {}",
                item["ms"],
                item["kb"],
                item["type"].as_str().unwrap_or("?"),
                item["url"].as_str().unwrap_or("?")
            ));
        }
    }
    let metric = |name: &str| {
        metrics["metrics"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|metric| metric["name"] == name)
            .and_then(|metric| metric["value"].as_f64())
    };
    if let Some(heap) = metric("JSHeapUsedSize") {
        lines.push(format!(
            "\nMain thread so far: script {:.0} ms, style {:.0} ms, layout {:.0} ms, {} layouts; JS heap {}.",
            metric("ScriptDuration").unwrap_or(0.0) * 1000.0,
            metric("RecalcStyleDuration").unwrap_or(0.0) * 1000.0,
            metric("LayoutDuration").unwrap_or(0.0) * 1000.0,
            metric("LayoutCount").unwrap_or(0.0),
            human_size(heap as u64)
        ));
    }
    lines.push("\nMeasured in the user's own Chrome (their extensions and machine): compare runs rather than reading one number alone.".into());
    lines.join("\n")
}

fn audit_text(found: &Value, console_errors: usize) -> String {
    let mut lines = Vec::new();
    for (key, title) in [
        ("accessibility", "Accessibility"),
        ("seo", "SEO"),
        ("best-practices", "Best practices"),
    ] {
        let Some(issues) = found[key].as_array() else {
            continue;
        };
        let mut issues: Vec<String> = issues
            .iter()
            .map(|issue| {
                format!(
                    "- {}{}{}",
                    issue["rule"].as_str().unwrap_or("?"),
                    issue["selector"]
                        .as_str()
                        .map(|selector| format!(" — {selector}"))
                        .unwrap_or_default(),
                    issue["detail"]
                        .as_str()
                        .map(|detail| format!(": {detail}"))
                        .unwrap_or_default(),
                )
            })
            .collect();
        if key == "best-practices" && console_errors > 0 {
            issues.push(format!(
                "- {console_errors} errors in the console (chrome_errors shows them)"
            ));
        }
        lines.push(format!(
            "{title}: {}",
            if issues.is_empty() {
                "no problems found".to_string()
            } else {
                format!("{} problem(s)", issues.len())
            }
        ));
        lines.extend(issues);
        lines.push(String::new());
    }
    lines.push("These checks read the page as it is now (a part of what Lighthouse checks); fix what they find, then audit again.".into());
    lines.join("\n")
}

/// Core Web Vitals and what slowed the page, read from the page's own
/// performance timeline (reads only).
const PERFORMANCE: &str = r#"(async () => {
  const selectorOf = (el) => {
    if (!el || el.nodeType !== 1) return null;
    if (el.id) return '#' + el.id;
    const parts = [];
    for (let node = el; node && node.nodeType === 1 && parts.length < 4; node = node.parentElement) {
      if (node.id) { parts.unshift('#' + node.id); break; }
      const cls = [...node.classList].slice(0, 2).map((c) => '.' + c).join('');
      parts.unshift(node.localName + cls);
    }
    return parts.join(' > ');
  };
  const observe = (type) => new Promise((resolve) => {
    try {
      const entries = [];
      const observer = new PerformanceObserver((list) => entries.push(...list.getEntries()));
      observer.observe({ type, buffered: true });
      setTimeout(() => { entries.push(...observer.takeRecords()); observer.disconnect(); resolve(entries); }, 100);
    } catch (error) { resolve([]); }
  });
  const [lcps, shifts, frames, events] = await Promise.all(
    ['largest-contentful-paint', 'layout-shift', 'long-animation-frame', 'event'].map(observe));
  const nav = performance.getEntriesByType('navigation')[0];
  const paint = Object.fromEntries(performance.getEntriesByType('paint').map((p) => [p.name, p.startTime]));
  const lcp = lcps[lcps.length - 1];
  let cls = 0, windowValue = 0, last = 0, first = 0;
  const sources = [];
  for (const shift of shifts) {
    if (shift.hadRecentInput) continue;
    if (shift.startTime - last > 1000 || shift.startTime - first > 5000) { windowValue = 0; first = shift.startTime; }
    windowValue += shift.value; last = shift.startTime; cls = Math.max(cls, windowValue);
    for (const source of shift.sources || []) {
      if (source.node) sources.push({ selector: selectorOf(source.node), value: Math.round(shift.value * 1000) / 1000 });
    }
  }
  const inp = events.filter((e) => e.interactionId).reduce((most, e) => Math.max(most, e.duration), 0);
  const resources = performance.getEntriesByType('resource');
  const brief = (r) => ({ url: r.name.slice(0, 200), ms: Math.round(r.duration), kb: Math.round((r.transferSize || 0) / 1024), type: r.initiatorType });
  return {
    url: location.href,
    ttfb: nav ? nav.responseStart : null,
    fcp: paint['first-contentful-paint'] ?? null,
    lcp: lcp ? { ms: lcp.startTime, element: lcp.element ? selectorOf(lcp.element) : (lcp.url || null) } : null,
    cls: Math.round(cls * 1000) / 1000,
    shiftSources: sources.slice(0, 5),
    inp: inp || null,
    domContentLoaded: nav ? nav.domContentLoadedEventEnd : null,
    load: nav ? nav.loadEventEnd : null,
    requests: resources.length,
    totalKb: Math.round((resources.reduce((sum, r) => sum + (r.transferSize || 0), 0) + (nav ? nav.transferSize || 0 : 0)) / 1024),
    slow: resources.slice().sort((a, b) => b.duration - a.duration).slice(0, 6).map(brief),
    heavy: resources.slice().sort((a, b) => (b.transferSize || 0) - (a.transferSize || 0)).slice(0, 6).map(brief),
    blocking: resources.filter((r) => r.renderBlockingStatus === 'blocking').map((r) => r.name.slice(0, 200)).slice(0, 10),
    longFrames: frames.sort((a, b) => b.duration - a.duration).slice(0, 5).map((f) => ({
      ms: Math.round(f.duration), blocking: Math.round(f.blockingDuration || 0),
      scripts: (f.scripts || []).sort((a, b) => b.duration - a.duration).slice(0, 3).map((s) => ({
        ms: Math.round(s.duration), source: (s.sourceURL || '?') + (s.sourceFunctionName ? ' ' + s.sourceFunctionName : ''), invoker: s.invoker || '' })),
    })),
    domNodes: document.getElementsByTagName('*').length,
  };
})()"#;

/// Accessibility, SEO and best-practice checks of the page as it is (reads
/// only). Called with the categories wanted.
const AUDIT: &str = r#"((wanted) => {
  const out = {};
  const MAX = 30;
  const selectorOf = (el) => {
    if (el.id) return '#' + CSS.escape(el.id);
    const parts = [];
    for (let node = el; node && node.nodeType === 1 && parts.length < 4; node = node.parentElement) {
      if (node.id) { parts.unshift('#' + CSS.escape(node.id)); break; }
      let part = node.localName;
      const parent = node.parentElement;
      if (parent) {
        const same = [...parent.children].filter((c) => c.localName === node.localName);
        if (same.length > 1) part += ':nth-of-type(' + (same.indexOf(node) + 1) + ')';
      }
      parts.unshift(part);
    }
    return parts.join(' > ');
  };
  const visible = (el) => {
    const r = el.getBoundingClientRect();
    const s = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none';
  };
  const nameOf = (el) => {
    const labelled = el.getAttribute('aria-labelledby');
    if (labelled) return labelled.split(/\s+/).map((id) => document.getElementById(id)?.textContent || '').join(' ').trim();
    return (el.getAttribute('aria-label') || el.labels?.[0]?.textContent || el.getAttribute('title')
      || el.textContent || el.getAttribute('alt') || [...el.querySelectorAll('img[alt]')].map((i) => i.alt).join(' ')
      || el.getAttribute('placeholder') || el.value || '').trim();
  };
  const issues = (list) => list.slice(0, MAX);
  const add = (list, rule, el, detail) => list.push({ rule, selector: el ? selectorOf(el) : null, detail: detail || null });

  // Contrast: text colour against the first opaque background behind it.
  const parse = (c) => { const m = c.match(/[\d.]+/g); return m ? m.map(Number) : [0, 0, 0, 1]; };
  const lum = ([r, g, b]) => { const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }; return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b); };
  const background = (el) => {
    for (let node = el; node && node.nodeType === 1; node = node.parentElement) {
      const s = getComputedStyle(node);
      if (s.backgroundImage && s.backgroundImage !== 'none') return null;
      const c = parse(s.backgroundColor);
      if ((c[3] ?? 1) >= 0.95) return c;
    }
    return [255, 255, 255, 1];
  };

  if (wanted.includes('accessibility')) {
    const a = [];
    if (!document.documentElement.lang) add(a, 'The page has no lang attribute', null, '<html lang="…">');
    for (const img of document.querySelectorAll('img')) {
      if (!img.hasAttribute('alt') && visible(img)) add(a, 'Image without alt text', img, img.currentSrc.slice(0, 120));
    }
    for (const el of document.querySelectorAll('button, a[href], [role=button], [role=link]')) {
      if (visible(el) && !nameOf(el)) add(a, 'Button or link without a name', el, el.outerHTML.slice(0, 120));
    }
    for (const el of document.querySelectorAll('input:not([type=hidden]):not([type=submit]):not([type=button]), select, textarea')) {
      if (visible(el) && !el.labels?.length && !el.getAttribute('aria-label') && !el.getAttribute('aria-labelledby') && !el.getAttribute('title')) {
        add(a, 'Form field without a label', el, el.getAttribute('placeholder') ? 'a placeholder is not a label' : null);
      }
    }
    const ids = new Map();
    for (const el of document.querySelectorAll('[id]')) ids.set(el.id, (ids.get(el.id) || 0) + 1);
    for (const [id, count] of ids) if (count > 1) add(a, 'Duplicate id', document.getElementById(id), `#${id} is used ${count} times`);
    let level = 0;
    for (const h of document.querySelectorAll('h1, h2, h3, h4, h5, h6')) {
      const n = Number(h.localName[1]);
      if (level && n > level + 1) add(a, 'Heading level skipped', h, `h${level} then h${n}`);
      level = n;
    }
    for (const el of document.querySelectorAll('[tabindex]')) {
      if (Number(el.getAttribute('tabindex')) > 0) add(a, 'Positive tabindex (changes the keyboard order)', el);
    }
    for (const el of document.querySelectorAll('[aria-hidden=true] a[href], [aria-hidden=true] button, [aria-hidden=true] input')) {
      add(a, 'Focusable element inside aria-hidden', el);
    }
    for (const frame of document.querySelectorAll('iframe')) if (!frame.title) add(a, 'Frame without a title', frame);
    const viewport = document.querySelector('meta[name=viewport]')?.content || '';
    if (/user-scalable\s*=\s*no|maximum-scale\s*=\s*1(\.0)?\b/.test(viewport)) add(a, 'Zooming is turned off', null, viewport);
    let contrast = 0;
    for (const el of document.querySelectorAll('p, span, a, button, label, li, td, th, h1, h2, h3, h4, h5, h6')) {
      if (contrast >= 10 || !visible(el) || !el.childNodes.length || ![...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim())) continue;
      const s = getComputedStyle(el);
      const fg = parse(s.color);
      const bg = background(el);
      if (!bg) continue;
      const [l1, l2] = [lum(fg), lum(bg)].sort((x, y) => y - x);
      const ratio = (l1 + 0.05) / (l2 + 0.05);
      const large = parseFloat(s.fontSize) >= 24 || (parseFloat(s.fontSize) >= 18.66 && Number(s.fontWeight) >= 700);
      if (ratio < (large ? 3 : 4.5)) { contrast += 1; add(a, 'Low text contrast', el, `${ratio.toFixed(2)}:1, needs ${large ? 3 : 4.5}:1`); }
    }
    for (const el of document.querySelectorAll('button, a[href], [role=button], input[type=checkbox], input[type=radio]')) {
      const r = el.getBoundingClientRect();
      if (visible(el) && r.width < 24 && r.height < 24) { add(a, 'Small touch target (under 24×24 px)', el, `${Math.round(r.width)}×${Math.round(r.height)}`); }
    }
    out.accessibility = issues(a);
  }
  if (wanted.includes('seo')) {
    const s = [];
    const title = document.title.trim();
    if (!title) add(s, 'No title'); else if (title.length > 65) add(s, 'Long title', null, `${title.length} characters`);
    const description = document.querySelector('meta[name=description]')?.content?.trim();
    if (!description) add(s, 'No meta description'); else if (description.length > 160) add(s, 'Long meta description', null, `${description.length} characters`);
    const h1s = document.querySelectorAll('h1').length;
    if (h1s !== 1) add(s, h1s ? 'More than one h1' : 'No h1', null, `${h1s} found`);
    if (!document.querySelector('link[rel=canonical]')) add(s, 'No canonical link');
    const robots = document.querySelector('meta[name=robots]')?.content || '';
    if (/noindex/i.test(robots)) add(s, 'The page asks not to be indexed', null, robots);
    if (!document.querySelector('meta[name=viewport]')) add(s, 'No viewport meta tag (not mobile friendly)');
    for (const link of document.querySelectorAll('a[href]')) {
      if (visible(link) && /^(click here|here|more|read more|link)$/i.test(link.textContent.trim())) add(s, 'Link text says nothing', link, link.textContent.trim());
    }
    if (!document.documentElement.lang) add(s, 'No lang attribute');
    out.seo = issues(s);
  }
  if (wanted.includes('best-practices')) {
    const b = [];
    if (location.protocol === 'http:' && !/^(localhost|127\.|\[::1\])/.test(location.hostname)) add(b, 'The page is not served over HTTPS');
    if (location.protocol === 'https:') {
      for (const r of performance.getEntriesByType('resource')) if (r.name.startsWith('http:')) add(b, 'Mixed content (an http resource on an https page)', null, r.name.slice(0, 160));
    }
    for (const img of document.querySelectorAll('img')) {
      if (!visible(img) || !img.naturalWidth) continue;
      const r = img.getBoundingClientRect();
      if (img.naturalWidth > r.width * devicePixelRatio * 2 && img.naturalWidth > 600) {
        add(b, 'Image much larger than shown', img, `${img.naturalWidth}px wide, shown at ${Math.round(r.width)}px`);
      }
      const sized = img.getAttribute('width') || img.getAttribute('height') || (img.style.width && img.style.height)
        || getComputedStyle(img).aspectRatio.includes('/');
      if (!sized) {
        add(b, 'Image without width and height (shifts the layout as it loads)', img);
      }
    }
    if (document.compatMode !== 'CSS1Compat') add(b, 'The page is in quirks mode (no <!doctype html>)');
    if (location.protocol === 'http:' && document.querySelector('input[type=password]')) add(b, 'Password field on a page without HTTPS');
    out['best-practices'] = issues(b);
  }
  return out;
})"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_match_as_chrome_does() {
        assert!(matches_pattern(
            "*/api/orders*",
            "https://x.dev/api/orders?page=2"
        ));
        assert!(matches_pattern("*://x.dev/*", "https://x.dev/a"));
        assert!(!matches_pattern("*/api/users", "https://x.dev/api/orders"));
        assert!(matches_pattern("https://x.dev/v?/a", "https://x.dev/v2/a"));
        assert!(matches_pattern("***", ""));
    }

    #[test]
    fn places_are_found_and_mapped() {
        let found = places("TypeError: x at render (https://app.dev/assets/index-1a.js:1:2050)");
        assert_eq!(
            found,
            vec![("https://app.dev/assets/index-1a.js".into(), 1, 2050)]
        );
        assert!(places("no places here: localhost:3000").is_empty());
    }

    #[test]
    fn heap_snapshots_are_summed_by_kind() {
        let snapshot = r#"{"snapshot":{"meta":{"node_fields":["type","name","id","self_size","edge_count","trace_node_id","detachedness"],"node_types":[["hidden","array","string","object","code","closure","regexp","number","native"]]}},
          "nodes":[3,0,1,100,0,0,0, 3,0,2,50,0,0,0, 8,1,3,30,0,0,2, 5,2,4,10,0,0,0],
          "strings":["Thing","HTMLDivElement","fn"]}"#;
        let summary = heap_summary(snapshot).unwrap();
        assert_eq!(summary["Thing"], (2, 150));
        assert_eq!(summary["Detached HTMLDivElement"], (1, 30));
        assert_eq!(summary["(closures)"], (1, 10));
        let mut later = summary.clone();
        later.insert("Thing".into(), (5, 400));
        let text = memory_text(&later, Some(&summary));
        assert!(text.contains("Thing: +3, +250 bytes"));
        assert!(text.contains("1 detached DOM nodes"));
    }
}
