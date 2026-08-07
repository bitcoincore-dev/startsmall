use std::env;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::thread;

use csv::ReaderBuilder;
use git2::{Repository, Signature};
use sha2::{Digest, Sha256};
use tiny_http::{Header, Method, Response, Server, StatusCode};

const DEFAULT_SHEET_CSV_URL: &str = "https://docs.google.com/spreadsheets/d/1-eGxq2mMoEGwgSpNVL5j2sa6ToojZUZ-Zun8h2oBAR4/export?format=csv&gid=0";
const SNAPSHOT_DIR: &str = "sheet-snapshots";
const SNAPSHOT_CSV: &str = "sheet-snapshots/google-sheet.csv";
const SNAPSHOT_META: &str = "sheet-snapshots/google-sheet.meta";

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let sheet_url = env::var("SHEET_CSV_URL").unwrap_or_else(|_| DEFAULT_SHEET_CSV_URL.to_string());

    match env::args().nth(1).as_deref() {
        Some("sync") => sync_once(&sheet_url)?,
        Some("serve") | None => serve(&sheet_url)?,
        Some(mode) => return Err(format!("unknown mode: {mode}").into()),
    }

    Ok(())
}

fn serve(sheet_url: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let addr = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:0".to_string());
    let server = Server::http(&addr)?;

    match server.server_addr().to_ip() {
        Some(listen_addr) => println!("Serving spreadsheet viewer at http://{listen_addr}"),
        None => println!("Serving spreadsheet viewer on {addr}"),
    }
    println!("Fetching data from {sheet_url}");
    println!("POST /sync to snapshot the sheet into git");

    for request in server.incoming_requests() {
        let url = sheet_url.to_string();
        thread::spawn(move || {
            let response = match (request.method(), request.url()) {
                (&Method::Get, "/") => match render_sheet_html(&url) {
                    Ok(html) => html_response(html),
                    Err(err) => html_response(render_error_page(&err.to_string())),
                },
                (&Method::Post, "/sync") => match sync_snapshot(&url) {
                    Ok(result) => html_response(render_sync_page(&result)),
                    Err(err) => html_response(render_error_page(&err.to_string())),
                },
                _ => Response::from_string("Not found").with_status_code(StatusCode(404)),
            };

            let _ = request.respond(response);
        });
    }

    Ok(())
}

fn sync_once(sheet_url: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let result = sync_snapshot(sheet_url)?;
    println!("Fetched {} rows from {}", result.snapshot.rows, sheet_url);
    println!("Snapshot hash: {}", result.snapshot.sha256);
    println!("Commit: {}", result.commit_id);
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
}

fn sync_snapshot(url: &str) -> Result<SyncResult, Box<dyn Error + Send + Sync>> {
    let snapshot = fetch_snapshot(url)?;
    let repo = Repository::discover(".")?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| "repository must not be bare".to_string())?;

    fs::create_dir_all(workdir.join(SNAPSHOT_DIR))?;
    fs::write(workdir.join(SNAPSHOT_CSV), &snapshot.csv)?;
    fs::write(workdir.join(SNAPSHOT_META), snapshot.meta.as_bytes())?;

    let commit = commit_snapshot(&repo)?;

    Ok(SyncResult {
        snapshot,
        commit_id: commit.commit_id,
        changed: commit.changed,
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

struct CommitResult {
    commit_id: String,
    changed: bool,
}

fn commit_snapshot(repo: &Repository) -> Result<CommitResult, Box<dyn Error + Send + Sync>> {
    let mut index = repo.index()?;
    index.add_path(Path::new(SNAPSHOT_CSV))?;
    index.add_path(Path::new(SNAPSHOT_META))?;
    index.write()?;

    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;
    let signature = signature(repo)?;

    if let Ok(head) = repo.head() {
        let parent = head.peel_to_commit()?;
        if parent.tree_id() == tree_id {
            return Ok(CommitResult {
                commit_id: parent.id().to_string(),
                changed: false,
            });
        }

        let commit_id = repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            "sync: snapshot Google Sheet document",
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
            "sync: snapshot Google Sheet document",
            &tree,
            &[],
        )?;
        Ok(CommitResult {
            commit_id: commit_id.to_string(),
            changed: true,
        })
    }
}

fn signature(repo: &Repository) -> Result<Signature<'_>, Box<dyn Error + Send + Sync>> {
    if let Ok(sig) = repo.signature() {
        return Ok(sig);
    }

    Ok(Signature::now("Start Small Bot", "start-small-bot@example.com")?)
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
         .actions{margin:0 0 16px 0}\
         button{padding:8px 12px;border:1px solid #d0d7de;background:#f6f8fa;border-radius:6px;cursor:pointer}\
         </style></head><body><h1>Spreadsheet Viewer</h1><div class=\"actions\"><form method=\"post\" action=\"/sync\"><button type=\"submit\">Sync snapshot to git</button></form></div><div class=\"wrap\"><table>",
    );

    for (index, row) in rows.iter().enumerate() {
        html.push_str("<tr>");
        html.push_str(&format!("<th class=\"rownum\">{}</th>", index + 1));
        for cell in row.iter() {
            html.push_str("<td>");
            html.push_str(&escape_html(cell));
            html.push_str("</td>");
        }
        html.push_str("</tr>");
    }

    html.push_str("</table></div></body></html>");
    html
}

fn render_sync_page(result: &SyncResult) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Synced</title></head>\
         <body><h1>Snapshot synced</h1><p>Commit: <code>{}</code></p>\
         <p>SHA-256: <code>{}</code></p>\
         <p>Rows: {}</p>\
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
