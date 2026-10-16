//! Live viewers: a loopback HTTP server that serves a run's saved viewer and
//! streams perspective-filtered frames to it as the run commits operations.
//!
//! Each issued URL carries a random token bound to one perspective, so a page can
//! read only the frames its token grants. Frames come from the environment's own
//! `frame(perspective)`; the host adds the routed table events that perspective may
//! see and, for a seat, that seat's own operations. Frames are computed after each
//! commit, so a burst of commits can surface as one step.
use super::EnvironmentHost;
use anyhow::{bail, Context, Result};
use pa_types::environments::ViewPerspective;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Mutex};

const RUNTIME: &str =
    include_str!("../../../../skills/environments/assets/viewers/viewer_runtime.js");
const MAX_REQUEST_HEAD: usize = 16 * 1024;
const KEEPALIVE: Duration = Duration::from_secs(15);
const CSP: &str = "default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; \
img-src 'self' data: blob:; font-src 'self' data:; media-src 'self' data: blob:; connect-src 'self'; \
frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

/// The running live-view server of one host, started on the first request.
pub(crate) struct LiveViews {
    port: u16,
    viewer: PathBuf,
    viewer_meta: Value,
    grants: std::sync::Mutex<BTreeMap<String, ViewPerspective>>,
    streams: Mutex<BTreeMap<ViewPerspective, FrameStream>>,
    generation: watch::Sender<u64>,
}

/// One perspective's encoded steps: a keyframe, then patches against the step before.
#[derive(Default)]
struct FrameStream {
    events: Vec<String>,
    last: Option<Value>,
    sequence: u64,
    table_seq: u64,
}

impl EnvironmentHost {
    /// Issue a live-view URL for `perspective` (default: the human seat, else public).
    /// Only the run's own human seat can be viewed from a seat perspective.
    pub(super) async fn live_view(self: &Arc<Self>, parameters: &Value) -> Result<Value> {
        let result = self.issue_live_view(parameters).await;
        crate::environment_telemetry::track(
            self.telemetry.as_ref(),
            "environment_live_view",
            if result.is_ok() {
                "completed"
            } else {
                "failed"
            },
            None,
        );
        result
    }

    async fn issue_live_view(self: &Arc<Self>, parameters: &Value) -> Result<Value> {
        let perspective = match parameters
            .get("perspective")
            .filter(|value| !value.is_null())
        {
            Some(value) => serde_json::from_value(value.clone())?,
            None => self
                .human()
                .map_or(ViewPerspective::Public, |seat| ViewPerspective::Seat {
                    seat,
                }),
        };
        if let ViewPerspective::Seat { seat } = &perspective {
            if self.human().as_ref() != Some(seat) {
                bail!("only the run's human seat can be viewed from a seat's perspective");
            }
        }
        let (_, frame) = self
            .instance
            .frame_versioned(&perspective)
            .await
            .context("live view unavailable")?;
        if frame.is_none() {
            bail!("this environment defines no frame(perspective), so it has no live view");
        }
        let live = {
            let mut slot = self.live.lock().await;
            if let Some(live) = slot.as_ref() {
                Arc::clone(live)
            } else {
                let live = self.start_live_views(parameters["viewer"].as_str()).await?;
                *slot = Some(Arc::clone(&live));
                live
            }
        };
        live.track(self, &perspective).await?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        live.grants
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(token.clone(), perspective.clone());
        Ok(json!({
            "url": format!("http://127.0.0.1:{}/v/{token}/", live.port),
            "perspective": perspective,
            "viewer": live.viewer_meta["id"],
        }))
    }

    async fn start_live_views(self: &Arc<Self>, requested: Option<&str>) -> Result<Arc<LiveViews>> {
        let (viewer, viewer_meta) = find_viewer(
            self.instance.path(),
            &self.instance.instance.definition_id,
            &self.instance.instance.version,
            requested,
        )?;
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let live = Arc::new(LiveViews {
            port: listener.local_addr()?.port(),
            viewer,
            viewer_meta,
            grants: std::sync::Mutex::new(BTreeMap::new()),
            streams: Mutex::new(BTreeMap::new()),
            generation: watch::channel(0).0,
        });
        let accept = Arc::clone(&live);
        let host = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = host.shutdown.cancelled() => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { continue };
                        let live = Arc::clone(&accept);
                        let host = Arc::clone(&host);
                        tokio::spawn(async move {
                            if let Err(error) = live.serve(&host, stream).await {
                                // A page closing its connection is routine; anything else is reported.
                                let disconnected = error.downcast_ref::<std::io::Error>().is_some_and(|error| {
                                    matches!(error.kind(), std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted)
                                });
                                if !disconnected {
                                    eprintln!("environment live view request failed: {error:#}");
                                }
                            }
                        });
                    }
                }
            }
        });
        let pump = Arc::clone(&live);
        let host = Arc::clone(self);
        let mut commits = self.instance.commits();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = host.shutdown.cancelled() => break,
                    changed = commits.changed() => {
                        if changed.is_err() { break; }
                        pump.refresh(&host).await;
                    }
                }
            }
        });
        Ok(live)
    }
}

impl LiveViews {
    /// Start streaming `perspective` if no page has asked for it yet.
    async fn track(&self, host: &EnvironmentHost, perspective: &ViewPerspective) -> Result<()> {
        let mut streams = self.streams.lock().await;
        let stream = streams.entry(perspective.clone()).or_default();
        if stream.events.is_empty() {
            push_step(host, perspective, stream).await?;
            self.generation.send_modify(|generation| *generation += 1);
        }
        Ok(())
    }

    /// Append the current frame of every streamed perspective after a commit.
    /// A failed frame reaches the page as an error event instead of a step.
    async fn refresh(&self, host: &EnvironmentHost) {
        let mut streams = self.streams.lock().await;
        for (perspective, stream) in streams.iter_mut() {
            if let Err(error) = push_step(host, perspective, stream).await {
                stream
                    .events
                    .push(json!({"error":format!("{error:#}")}).to_string());
            }
        }
        drop(streams);
        self.generation.send_modify(|generation| *generation += 1);
    }

    async fn serve(&self, host: &EnvironmentHost, mut stream: TcpStream) -> Result<()> {
        let head = tokio::time::timeout(Duration::from_secs(10), read_head(&mut stream)).await??;
        let mut lines = head.lines();
        let request = lines.next().unwrap_or_default();
        let mut parts = request.split(' ');
        let (method, target) = (
            parts.next().unwrap_or_default(),
            parts.next().unwrap_or_default(),
        );
        let headers: BTreeMap<String, String> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        // Refuse other host names so a rebound DNS name cannot reach the loopback port.
        let host_header = headers.get("host").map(String::as_str).unwrap_or_default();
        if host_header != format!("127.0.0.1:{}", self.port)
            && host_header != format!("localhost:{}", self.port)
        {
            return respond(&mut stream, "403 Forbidden", "text/plain", b"forbidden").await;
        }
        if method != "GET" {
            return respond(
                &mut stream,
                "405 Method Not Allowed",
                "text/plain",
                b"GET only",
            )
            .await;
        }
        let path = target.split('?').next().unwrap_or_default();
        let Some((token, rest)) = path
            .strip_prefix("/v/")
            .and_then(|rest| rest.split_once('/'))
        else {
            return respond(&mut stream, "404 Not Found", "text/plain", b"not found").await;
        };
        let perspective = self
            .grants
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(token)
            .cloned();
        let Some(perspective) = perspective else {
            return respond(&mut stream, "404 Not Found", "text/plain", b"not found").await;
        };
        match rest {
            "" | "index.html" => {
                let page = self.page(host, &perspective)?;
                respond(
                    &mut stream,
                    "200 OK",
                    "text/html; charset=utf-8",
                    page.as_bytes(),
                )
                .await
            }
            "events" => {
                let resume = headers
                    .get("last-event-id")
                    .and_then(|id| id.parse::<usize>().ok())
                    .map_or(0, |id| id + 1);
                self.events(host, &perspective, resume, stream).await
            }
            file => match self.asset(file) {
                Some((kind, bytes)) => respond(&mut stream, "200 OK", kind, &bytes).await,
                None => respond(&mut stream, "404 Not Found", "text/plain", b"not found").await,
            },
        }
    }

    fn page(&self, host: &EnvironmentHost, perspective: &ViewPerspective) -> Result<String> {
        let instance = &host.instance.instance;
        let seats: Vec<Value> = host
            .instance
            .roster()
            .seats
            .iter()
            .map(|seat| json!({"id":seat.id,"role":seat.role,"occupant":seat.occupant,"model":seat.model}))
            .collect();
        let config = json!({
            "format": 1,
            "events": "events",
            "meta": {
                "definition_id": instance.definition_id,
                "name": host.instance.definition.manifest.name,
                "version": instance.version,
                "instance_id": instance.id,
                "created_at": instance.created_at,
                "seats": seats,
                "config": host.instance.setup_config()?,
                "perspective": perspective,
                "mode": "live",
                "viewer": {
                    "id": self.viewer_meta["id"],
                    "title": self.viewer_meta["title"],
                    "revision": self.viewer_meta["revision"],
                    "shell": self.viewer_meta["shell"],
                },
            },
        });
        let injected = format!(
            "<meta charset=\"utf-8\"><script type=\"application/json\" id=\"env-live-config\">{}</script><script>{RUNTIME}</script>",
            config.to_string().replace("</", "<\\/")
        );
        let page = std::fs::read_to_string(self.viewer.join("index.html"))?;
        let lower = page.to_ascii_lowercase();
        let head_end = lower
            .find("<head")
            .and_then(|start| lower[start..].find('>').map(|end| start + end + 1));
        Ok(match head_end {
            Some(at) => format!("{}{injected}{}", &page[..at], &page[at..]),
            None => format!("{injected}{page}"),
        })
    }

    /// A viewer file, resolved inside the viewer folder only.
    fn asset(&self, relative: &str) -> Option<(&'static str, Vec<u8>)> {
        if relative.contains("..") || relative.contains('%') || relative.contains('\\') {
            return None;
        }
        let root = self.viewer.canonicalize().ok()?;
        let path = root.join(relative).canonicalize().ok()?;
        if !path.starts_with(&root) || !path.is_file() || path.file_name()? == "viewer.json" {
            return None;
        }
        let kind = match path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
        {
            "html" => "text/html; charset=utf-8",
            "js" | "mjs" => "text/javascript; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            "json" => "application/json",
            "svg" => "image/svg+xml",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "woff2" => "font/woff2",
            "woff" => "font/woff",
            "ttf" => "font/ttf",
            "ico" => "image/x-icon",
            _ => "application/octet-stream",
        };
        Some((kind, std::fs::read(path).ok()?))
    }

    /// Server-sent events: every step from `resume` on, then each new one.
    async fn events(
        &self,
        host: &EnvironmentHost,
        perspective: &ViewPerspective,
        resume: usize,
        mut stream: TcpStream,
    ) -> Result<()> {
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\n\
                  Referrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nConnection: keep-alive\r\n\r\n",
            )
            .await?;
        let mut generation = self.generation.subscribe();
        let mut sent = resume;
        loop {
            let pending: Vec<(usize, String)> = {
                let streams = self.streams.lock().await;
                streams
                    .get(perspective)
                    .map(|frames| {
                        frames
                            .events
                            .iter()
                            .cloned()
                            .enumerate()
                            .skip(sent)
                            .collect()
                    })
                    .unwrap_or_default()
            };
            for (index, event) in pending {
                stream
                    .write_all(format!("id: {index}\ndata: {event}\n\n").as_bytes())
                    .await?;
                sent = index + 1;
            }
            tokio::select! {
                () = host.shutdown.cancelled() => return Ok(()),
                changed = generation.changed() => changed?,
                () = tokio::time::sleep(KEEPALIVE) => stream.write_all(b": keepalive\n\n").await?,
            }
        }
    }
}

/// Encode the current frame of `perspective` as the next step, if the run moved.
async fn push_step(
    host: &EnvironmentHost,
    perspective: &ViewPerspective,
    stream: &mut FrameStream,
) -> Result<()> {
    let (version, frame) = host.instance.frame_versioned(perspective).await?;
    let frame = frame.context("the environment stopped returning frames")?;
    let sequence = version.saturating_sub(1);
    if stream.last.is_some() && sequence <= stream.sequence {
        return Ok(());
    }
    let checkpoint = host.instance.checkpoint().await;
    let viewer_seat = match perspective {
        ViewPerspective::Public => None,
        ViewPerspective::Seat { seat } => Some(seat),
    };
    // A seat sees its own operation in full; everything else stays in the frame
    // and the routed events the environment chose to publish.
    let op = checkpoint
        .receipts
        .values()
        .find(|receipt| receipt.sequence == Some(sequence))
        .filter(|receipt| {
            viewer_seat.is_some()
                && receipt.request.get("participant")
                    == viewer_seat.map(|seat| json!(seat)).as_ref()
        })
        .map(|receipt| {
            let params = &receipt.request["params"];
            json!({"method":receipt.request["method"],"seat":receipt.request["participant"],
                   "action":params["action"],"arguments":params["arguments"],"text":params["text"],
                   "success":receipt.success,"result":receipt.result})
        });
    let deliveries = host.table.since(viewer_seat, stream.table_seq)?;
    if let Some(last) = deliveries.last() {
        stream.table_seq = last.seq;
    }
    let mut step = json!({
        "sequence": sequence,
        "recorded": true,
        "time": crate::util::now_ms(),
        "op": op,
        "deliveries": deliveries,
        "finished": checkpoint.finished.map(|finished| finished.summary),
    });
    match stream.last.as_ref() {
        Some(last) => {
            let patch = pa_environments::json_patch(last, &frame)?;
            if serde_json::to_vec(&patch)?.len() < serde_json::to_vec(&frame)?.len() {
                step["patch"] = patch;
            } else {
                step["state"] = frame.clone();
            }
        }
        None => step["state"] = frame.clone(),
    }
    stream.events.push(step.to_string());
    stream.last = Some(frame);
    stream.sequence = sequence;
    Ok(())
}

/// The saved viewer for this definition that lists `version`, or the requested one.
fn find_viewer(
    instance: &Path,
    definition: &str,
    version: &str,
    requested: Option<&str>,
) -> Result<(PathBuf, Value)> {
    let root = instance
        .parent()
        .and_then(Path::parent)
        .context("instance directory has no environments root")?
        .join("viewers")
        .join(definition);
    let mut matches = Vec::new();
    for entry in
        std::fs::read_dir(&root).map_or_else(|_| Vec::new(), |entries| entries.flatten().collect())
    {
        let meta_path = entry.path().join("viewer.json");
        let Ok(bytes) = std::fs::read(&meta_path) else {
            continue;
        };
        let meta: Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("decode {}", meta_path.display()))?;
        let listed = meta["compatible_versions"]
            .as_array()
            .is_some_and(|versions| versions.iter().any(|v| v == version));
        if listed && requested.is_none_or(|id| meta["id"] == id) {
            matches.push((entry.path(), meta));
        }
    }
    matches.sort_by(|a, b| a.0.cmp(&b.0));
    match matches.len() {
        0 => bail!("no saved viewer for {definition} lists this version; ask the builder to create or update one"),
        1 => Ok(matches.remove(0)),
        _ => bail!("several viewers match; choose one with the viewer parameter"),
    }
}

async fn read_head(stream: &mut TcpStream) -> Result<String> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 2048];
    while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut chunk).await?;
        if read == 0 || buffer.len() + read > MAX_REQUEST_HEAD {
            bail!("incomplete or oversized request head");
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

async fn respond(stream: &mut TcpStream, status: &str, kind: &str, body: &[u8]) -> Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         Content-Security-Policy: {CSP}\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.shutdown().await?;
    Ok(())
}
