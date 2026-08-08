use std::env;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, ValueEnum};
use csv::ReaderBuilder;
use git2::{Repository, Signature};
use nostr_sdk::prelude::*;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};
use tokio::runtime::Builder;

const DEFAULT_SHEET_CSV_URL: &str = "https://docs.google.com/spreadsheets/d/1-eGxq2mMoEGwgSpNVL5j2sa6ToojZUZ-Zun8h2oBAR4/export?format=csv&gid=0";
const SNAPSHOT_DIR: &str = "sheet-snapshots";
const SNAPSHOT_CSV: &str = "sheet-snapshots/google-sheet.csv";
const SNAPSHOT_META: &str = "sheet-snapshots/google-sheet.meta";
const DEFAULT_BIND_ADDR: &str = "127.0.0.1:0";
const DEFAULT_RELAYS: [&str; 3] = [
    "wss://relay.damus.io",
    "wss://relay.nostr.band",
    "wss://nostr.wine",
];

#[derive(Parser, Debug)]
#[command(version, about = "Sync and serve a Google Sheet snapshot")]
struct Args {
    #[arg(long, value_name = "URL")]
    sheet_url: Option<String>,

    #[arg(long, value_name = "ADDR")]
    bind_addr: Option<String>,

    #[arg(long, value_name = "NSEC_OR_HEX", help = "Nostr private key used to publish sync notes")]
    privkey: Option<String>,

    #[arg(value_enum, default_value_t = Mode::Serve)]
    mode: Mode,
}

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq)]
enum Mode {
    Serve,
    Sync,
}

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let args = Args::parse();
    let sheet_url = resolve_sheet_url(args.sheet_url);

    match args.mode {
        Mode::Sync => sync_once(&sheet_url, args.privkey.as_deref())?,
        Mode::Serve => serve(&sheet_url, args.bind_addr.as_deref(), args.privkey.as_deref())?,
    }

    Ok(())
}

fn resolve_sheet_url(value: Option<String>) -> String {
    value
        .or_else(|| env::var("SHEET_CSV_URL").ok())
        .unwrap_or_else(|| DEFAULT_SHEET_CSV_URL.to_string())
}

fn resolve_bind_addr(value: Option<&str>) -> String {
    value
        .map(ToOwned::to_owned)
        .or_else(|| env::var("BIND_ADDR").ok())
        .unwrap_or_else(|| DEFAULT_BIND_ADDR.to_string())
}

fn serve(sheet_url: &str, bind_addr: Option<&str>, privkey: Option<&str>) -> Result<(), Box<dyn Error + Send + Sync>> {
    let addr = resolve_bind_addr(bind_addr);
    let server = Server::http(&addr)?;

    match server.server_addr().to_ip() {
        Some(listen_addr) => println!("Serving spreadsheet viewer at http://{listen_addr}"),
        None => println!("Serving spreadsheet viewer on {addr}"),
    }
    println!("Fetching data from {sheet_url}");
    println!("POST /sync to snapshot the sheet into git");

    for request in server.incoming_requests() {
        let response = route_request(request.method(), request.url(), &request, sheet_url, privkey);
        let _ = request.respond(response);
    }

    Ok(())
}

fn route_request(
    method: &Method,
    path: &str,
    request: &Request,
    sheet_url: &str,
    privkey: Option<&str>,
) -> Response<std::io::Cursor<Vec<u8>>> {
    match (method, path) {
        (&Method::Get, "/") => match render_sheet_html(sheet_url) {
            Ok(html) => html_response(html),
            Err(err) => html_response(render_error_page(&err.to_string())),
        },
        (&Method::Post, "/sync") => {
            if !sync_request_authorized(request) {
                return html_response(render_unauthorized_page())
                    .with_status_code(StatusCode(403));
            }
            match sync_snapshot(sheet_url, privkey) {
                Ok(result) => html_response(render_sync_page(&result)),
                Err(err) => html_response(render_error_page(&err.to_string())),
            }
        }
        _ => Response::from_string("Not found").with_status_code(StatusCode(404)),
    }
}

fn sync_request_authorized(request: &Request) -> bool {
    if request
        .remote_addr()
        .map(|addr| addr.ip().is_loopback())
        .unwrap_or(false)
    {
        return true;
    }

    let token = match env::var("SYNC_TOKEN") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => return false,
    };

    request.headers().iter().any(|header| {
        if header.field.equiv("X-Sync-Token") {
            return token_matches(header.value.as_str().trim(), &token);
        }

        if header.field.equiv("Authorization") {
            return header
                .value
                .as_str()
                .trim()
                .strip_prefix("Bearer ")
                .map(|value| token_matches(value.trim(), &token))
                .unwrap_or(false);
        }

        false
    })
}

fn token_matches(candidate: &str, expected: &str) -> bool {
    bool::from(candidate.as_bytes().ct_eq(expected.as_bytes()))
}

fn sync_once(sheet_url: &str, privkey: Option<&str>) -> Result<(), Box<dyn Error + Send + Sync>> {
    let result = sync_snapshot(sheet_url, privkey)?;
    println!("Fetched {} rows from {}", result.snapshot.rows, sheet_url);
    println!("Snapshot hash: {}", result.snapshot.sha256);
    println!("Commit: {}", result.commit_id);

    if let Some(event_id) = result.nostr_event_id.as_deref() {
        println!("Nostr event: {}", event_id);
    }

    if let Some(event_id) = result.nip34_event_id.as_deref() {
        println!("NIP-34 event: {}", event_id);
    }

    if result.changed {
        println!("Committed snapshot to git.");
    } else {
        println!("No content change; repository already matched the sheet.");
    }

    Ok(())
}

struct Snapshot {
    csv: String,
    meta: String,
    rows: usize,
    sha256: String,
}

struct SyncResult {
    snapshot: Snapshot,
    commit_id: String,
    changed: bool,
    nostr_event_id: Option<String>,
    nip34_event_id: Option<String>,
}

fn sync_snapshot(url: &str, privkey: Option<&str>) -> Result<SyncResult, Box<dyn Error + Send + Sync>> {
    let snapshot = fetch_snapshot(url)?;
    let synced_at_unix_ns = synced_at_unix_ns();
    let repo = Repository::discover(".")?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| "repository must not be bare".to_string())?;

    fs::create_dir_all(workdir.join(SNAPSHOT_DIR))?;
    fs::write(workdir.join(SNAPSHOT_CSV), &snapshot.csv)?;
    fs::write(
        workdir.join(SNAPSHOT_META),
        format!("{}synced_at_unix_ns={}\n", snapshot.meta, synced_at_unix_ns),
    )?;

    let commit_message = format!(
        "sync: snapshot Google Sheet document\n\nsha256: {}\nrows: {}\nsource: {}\nfile: {}\nmeta: {}\nsynced_at_unix_ns: {}",
        snapshot.sha256, snapshot.rows, url, SNAPSHOT_CSV, SNAPSHOT_META, synced_at_unix_ns
    );

    let commit = commit_snapshot(&repo, &commit_message)?;
    let (nostr_event_id, nip34_event_id) = match privkey {
        Some(privkey) => (
            Some(publish_nostr_note(privkey, &snapshot, &commit.commit_id, url)?),
            Some(publish_nip34_repo_announcement(privkey, &repo)?),
        ),
        None => (None, None),
    };

    Ok(SyncResult {
        snapshot,
        commit_id: commit.commit_id,
        changed: commit.changed,
        nostr_event_id,
        nip34_event_id,
    })
}

fn fetch_snapshot(url: &str) -> Result<Snapshot, Box<dyn Error + Send + Sync>> {
    let body = reqwest::blocking::get(url)?.error_for_status()?.text()?;
    let mut reader = ReaderBuilder::new().has_headers(false).from_reader(body.as_bytes());

    let mut rows = 0usize;
    for record in reader.records() {
        record?;
        rows += 1;
    }

    let sha256 = format!("{:x}", Sha256::digest(body.as_bytes()));
    let meta = format!("source_url={url}\nsha256={sha256}\nrows={rows}\n");

    Ok(Snapshot {
        csv: body,
        meta,
        rows,
        sha256,
    })
}

fn publish_nip34_repo_announcement(
    privkey: &str,
    repo: &Repository,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let keys = Keys::parse(privkey)?;
    let announcement = GitRepositoryAnnouncement {
        id: "startsmall".to_string(),
        name: Some("StartSmall".to_string()),
        description: Some("Google Sheet snapshot viewer and git sync".to_string()),
        web: resolve_web_urls(repo),
        clone: resolve_clone_urls(repo),
        relays: resolve_relay_urls(),
        euc: None,
        maintainers: Vec::new(),
    };
    let event = announcement.into_event_builder().finalize(&keys)?;
    let relays = resolve_relays();

    let runtime = Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(async move {
        let client = Client::new();
        for relay in relays {
            client.add_relay(relay).await?;
        }
        client.connect().await;
        client.send_event(&event).await?;
        Ok::<String, Box<dyn Error + Send + Sync>>(event.id.to_string())
    })
}

fn resolve_web_urls(repo: &Repository) -> Vec<Url> {
    let mut urls = Vec::new();

    if let Some(workdir) = repo.workdir() {
        let cname = workdir.join("CNAME");
        if let Ok(domain) = fs::read_to_string(cname) {
            let domain = domain.trim();
            if !domain.is_empty() {
                if let Ok(url) = Url::parse(&format!("https://{domain}")) {
                    urls.push(url);
                }
            }
        }
    }

    urls
}

fn resolve_clone_urls(repo: &Repository) -> Vec<Url> {
    let mut urls = Vec::new();

    if let Ok(remote) = repo.find_remote("origin") {
        if let Some(url) = remote.url().and_then(normalize_clone_url) {
            if let Ok(parsed) = Url::parse(&url) {
                urls.push(parsed);
            }
        }
    }

    urls
}

fn resolve_relay_urls() -> Vec<RelayUrl> {
    resolve_relays()
        .into_iter()
        .filter_map(|relay| RelayUrl::parse(&relay).ok())
        .collect()
}

fn normalize_clone_url(remote: &str) -> Option<String> {
    if remote.starts_with("http://") || remote.starts_with("https://") {
        return Some(remote.trim_end_matches(".git").to_string());
    }

    if let Some(rest) = remote.strip_prefix("git@github.com:") {
        return Some(format!("https://github.com/{}", rest.trim_end_matches(".git")));
    }

    if let Some(rest) = remote.strip_prefix("ssh://git@github.com/") {
        return Some(format!("https://github.com/{}", rest.trim_end_matches(".git")));
    }

    None
}

struct CommitResult {
    commit_id: String,
    changed: bool,
}

fn commit_snapshot(repo: &Repository, message: &str) -> Result<CommitResult, Box<dyn Error + Send + Sync>> {
    let mut index = repo.index()?;
    index.add_path(Path::new(SNAPSHOT_CSV))?;
    index.add_path(Path::new(SNAPSHOT_META))?;
    index.write()?;

    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;
    let signature = signature(repo)?;

    if let Ok(head) = repo.head() {
        let parent = head.peel_to_commit()?;
        let commit_id = repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &[&parent],
        )?;
        Ok(CommitResult {
            commit_id: commit_id.to_string(),
            changed: true,
        })
    } else {
        let commit_id = repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &[],
        )?;
        Ok(CommitResult {
            commit_id: commit_id.to_string(),
            changed: true,
        })
    }
}

fn synced_at_unix_ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn signature(repo: &Repository) -> Result<Signature<'_>, Box<dyn Error + Send + Sync>> {
    if let Ok(sig) = repo.signature() {
        return Ok(sig);
    }

    Ok(Signature::now("Start Small Bot", "start-small-bot@example.com")?)
}

fn publish_nostr_note(
    privkey: &str,
    snapshot: &Snapshot,
    commit_id: &str,
    source_url: &str,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let keys = Keys::parse(privkey)?;
    let content = format!(
        "StartSmall snapshot synced\ncommit: {commit_id}\nsha256: {}\nrows: {}\nsource: {source_url}",
        snapshot.sha256, snapshot.rows
    );
    let relays = resolve_relays();

    let runtime = Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(async move {
        let client = Client::new();
        for relay in relays {
            client.add_relay(relay).await?;
        }
        client.connect().await;

        let event = EventBuilder::new(Kind::TextNote, content).finalize(&keys)?;
        client.send_event(&event).await?;
        Ok::<String, Box<dyn Error + Send + Sync>>(event.id.to_string())
    })
}

fn resolve_relays() -> Vec<String> {
    match env::var("NOSTR_RELAYS") {
        Ok(value) => value
            .split(',')
            .map(str::trim)
            .filter(|relay| !relay.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        Err(_) => DEFAULT_RELAYS.iter().map(|relay| (*relay).to_string()).collect(),
    }
}

fn render_sheet_html(url: &str) -> Result<String, Box<dyn Error + Send + Sync>> {
    let body = reqwest::blocking::get(url)?.error_for_status()?.text()?;
    let mut reader = ReaderBuilder::new().has_headers(false).from_reader(body.as_bytes());

    let mut rows = Vec::new();
    for record in reader.records() {
        rows.push(record?);
    }

    Ok(render_sheet_page(&rows))
}

fn render_sheet_page(rows: &[csv::StringRecord]) -> String {
    let parsed = parse_sheet(rows);
    let mut html = String::from(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>Spreadsheet Viewer</title>\
         <style>\
         body{font-family:system-ui,-apple-system,Segoe UI,Roboto,sans-serif;margin:24px;line-height:1.4}\
         table{border-collapse:collapse;width:100%;font-size:14px}\
         th,td{border:1px solid #d0d7de;padding:8px;vertical-align:top;text-align:left}\
         th{background:#f6f8fa;position:sticky;top:0}\
         .rownum{background:#f6f8fa;width:64px;white-space:nowrap;font-variant-numeric:tabular-nums}\
         .wrap{overflow-x:auto}\
         .meta{margin:0 0 16px 0}\
         .field{color:#57606a;font-size:12px;margin-left:6px}\
         .cell-link{text-decoration:none;color:#0969da}\
         .actions{margin:0 0 16px 0}\
         button{padding:8px 12px;border:1px solid #d0d7de;background:#f6f8fa;border-radius:6px;cursor:pointer}\
         </style></head><body><h1>Spreadsheet Viewer</h1><div class=\"actions\"><form method=\"post\" action=\"/sync\"><button type=\"submit\">Sync snapshot to git</button></form></div><div class=\"wrap\"><table>",
    );

    if !parsed.metadata_rows.is_empty() {
        html.push_str("</table></div><div class=\"meta\"><table>");
        for row in parsed.metadata_rows {
            html.push_str("<tr>");
            for cell in row {
                html.push_str("<td>");
                html.push_str(&escape_html(&cell));
                html.push_str("</td>");
            }
            html.push_str("</tr>");
        }
        html.push_str("</table></div><div class=\"wrap\"><table>");
    }

    if !parsed.headers.is_empty() {
        html.push_str("<tr>");
        html.push_str("<th class=\"rownum\">#</th>");
        for header in &parsed.headers {
            let (label, field) = annotate_header(header);
            html.push_str("<th>");
            html.push_str(&escape_html(&label));
            if let Some(field) = field {
                html.push_str("<span class=\"field\">");
                html.push_str(&escape_html(field));
                html.push_str("</span>");
            }
            html.push_str("</th>");
        }
        html.push_str("</tr>");
    }

    for (index, row) in parsed.data_rows.iter().enumerate() {
        html.push_str("<tr>");
        html.push_str(&format!("<th class=\"rownum\">{}</th>", index + 1));
        for (col_index, cell) in row.iter().enumerate() {
            html.push_str("<td>");
            let header = parsed.headers.get(col_index).map(String::as_str);
            html.push_str(&render_cell(cell, header));
            html.push_str("</td>");
        }
        html.push_str("</tr>");
    }

    html.push_str("</table></div></body></html>");
    html
}

struct ParsedSheet {
    metadata_rows: Vec<Vec<String>>,
    headers: Vec<String>,
    data_rows: Vec<Vec<String>>,
}

fn parse_sheet(rows: &[csv::StringRecord]) -> ParsedSheet {
    let mut metadata_rows = Vec::new();
    let mut headers = Vec::new();
    let mut data_rows = Vec::new();
    let mut header_found = false;

    for row in rows {
        let cells: Vec<String> = row.iter().map(|cell| cell.trim().to_string()).collect();
        if !header_found && is_header_row(&cells) {
            headers = cells;
            header_found = true;
            continue;
        }

        if header_found {
            if cells.iter().any(|cell| !cell.is_empty()) {
                data_rows.push(cells);
            }
        } else if cells.iter().any(|cell| !cell.is_empty()) {
            metadata_rows.push(cells);
        }
    }

    ParsedSheet {
        metadata_rows,
        headers,
        data_rows,
    }
}

fn is_header_row(cells: &[String]) -> bool {
    let keywords = ["date", "amount", "grantee", "twitter", "x", "link", "why", "domain", "url"];
    let mut score = 0usize;

    for cell in cells {
        let lower = cell.to_lowercase();
        if keywords.iter().any(|keyword| lower.contains(keyword)) {
            score += 1;
        }
    }

    score >= 2
}

fn annotate_header(header: &str) -> (String, Option<&'static str>) {
    let lower = header.to_lowercase();
    if lower.contains("twitter") || lower == "x" || lower.contains("x (twitter)") {
        (header.to_string(), Some("twitter"))
    } else if lower.contains("domain") {
        (header.to_string(), Some("domain"))
    } else if lower.contains("link") || lower.contains("url") || lower.contains("website") {
        (header.to_string(), Some("url"))
    } else {
        (header.to_string(), None)
    }
}

fn render_cell(value: &str, header: Option<&str>) -> String {
    if let Some((label, href)) = detect_link(value, header) {
        return format!(
            "<a class=\"cell-link\" href=\"{}\" target=\"_blank\" rel=\"noreferrer\">{}</a>",
            escape_html(&href),
            escape_html(&label)
        );
    }

    escape_html(value)
}

fn detect_link(value: &str, header: Option<&str>) -> Option<(String, String)> {
    let raw = value.trim();
    if raw.is_empty() {
        return None;
    }

    if let Some(url) = normalized_url(raw) {
        let label = if header.map_or(false, |h| is_twitter_header(h)) {
            twitter_label(raw)
        } else {
            raw.to_string()
        };
        return Some((label, url));
    }

    if header.map_or(false, |h| is_twitter_header(h)) {
        let handle = twitter_handle(raw)?;
        return Some((format!("@{handle}"), format!("https://x.com/{handle}")));
    }

    if header.map_or(false, |h| is_domain_header(h)) {
        let domain = bare_domain(raw)?;
        return Some((domain.to_string(), format!("https://{domain}")));
    }

    if let Some(domain) = bare_domain(raw) {
        return Some((domain.to_string(), format!("https://{domain}")));
    }

    None
}

fn normalized_url(value: &str) -> Option<String> {
    if value.starts_with("http://") || value.starts_with("https://") {
        return Some(value.to_string());
    }

    if value.contains("twitter.com/") || value.contains("x.com/") {
        let prefixed = if value.starts_with('@') {
            format!("https://x.com/{}", value.trim_start_matches('@'))
        } else {
            format!("https://{}", value.trim_start_matches('/'))
        };
        return Some(prefixed);
    }

    None
}

fn is_twitter_header(header: &str) -> bool {
    let lower = header.to_lowercase();
    lower.contains("twitter") || lower == "x" || lower.contains("x (twitter)")
}

fn is_domain_header(header: &str) -> bool {
    header.to_lowercase().contains("domain")
}

fn twitter_handle(value: &str) -> Option<&str> {
    let trimmed = value.trim().trim_start_matches('@');
    let trimmed = trimmed
        .strip_prefix("https://x.com/")
        .or_else(|| trimmed.strip_prefix("https://www.x.com/"))
        .or_else(|| trimmed.strip_prefix("https://twitter.com/"))
        .or_else(|| trimmed.strip_prefix("https://www.twitter.com/"))
        .unwrap_or(trimmed);

    let handle = trimmed.split('/').next()?.trim();
    if handle.is_empty() {
        None
    } else {
        Some(handle)
    }
}

fn twitter_label(value: &str) -> String {
    twitter_handle(value)
        .map(|handle| format!("@{handle}"))
        .unwrap_or_else(|| value.to_string())
}

fn bare_domain(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    let host = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);

    let host = host.trim_start_matches("www.");
    if host.contains('.') && !host.contains(' ') {
        Some(host.split('/').next().unwrap_or(host))
    } else {
        None
    }
}

fn render_sync_page(result: &SyncResult) -> String {
    let nostr = result
        .nostr_event_id
        .as_deref()
        .map(|id| format!("<p>Nostr event: <code>{}</code></p>", escape_html(id)))
        .unwrap_or_default();
    let nip34 = result
        .nip34_event_id
        .as_deref()
        .map(|id| format!("<p>NIP-34 event: <code>{}</code></p>", escape_html(id)))
        .unwrap_or_default();

    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Synced</title></head>\
         <body><h1>Snapshot synced</h1><p>Commit: <code>{}</code></p>\
         <p>SHA-256: <code>{}</code></p>\
         <p>Rows: {}</p>{nostr}{nip34}\
         <p><a href=\"/\">Back</a></p></body></html>",
        escape_html(&result.commit_id),
        escape_html(&result.snapshot.sha256),
        result.snapshot.rows
    )
}

fn render_error_page(message: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Error</title></head>\
         <body><h1>Failed to load spreadsheet</h1><pre>{}</pre></body></html>",
        escape_html(message)
    )
}

fn render_unauthorized_page() -> String {
    "<!doctype html><html><head><meta charset=\"utf-8\"><title>Forbidden</title></head>\
     <body><h1>Sync forbidden</h1><p>Use localhost or provide a valid SYNC_TOKEN.</p></body></html>"
        .to_string()
}

fn html_response(body: String) -> Response<std::io::Cursor<Vec<u8>>> {
    let mut response = Response::from_string(body);
    if let Ok(header) = Header::from_bytes(b"Content-Type".as_slice(), b"text/html; charset=utf-8".as_slice()) {
        response = response.with_header(header);
    }
    response
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
