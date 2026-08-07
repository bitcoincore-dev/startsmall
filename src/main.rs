use std::env;
use std::error::Error;
use std::io::Cursor;
use std::thread;

use csv::ReaderBuilder;
use tiny_http::{Header, Response, Server};

const DEFAULT_SHEET_CSV_URL: &str = "https://docs.google.com/spreadsheets/d/1-eGxq2mMoEGwgSpNVL5j2sa6ToojZUZ-Zun8h2oBAR4/export?format=csv&gid=0";

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let sheet_url = env::var("SHEET_CSV_URL").unwrap_or_else(|_| DEFAULT_SHEET_CSV_URL.to_string());
    let addr = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:0".to_string());
    let server = Server::http(&addr)?;

    match server.server_addr().to_ip() {
        Some(listen_addr) => println!("Serving spreadsheet viewer at http://{listen_addr}"),
        None => println!("Serving spreadsheet viewer on {addr}"),
    }
    println!("Fetching data from {sheet_url}");

    for request in server.incoming_requests() {
        let url = sheet_url.clone();
        thread::spawn(move || {
            let response = match fetch_sheet_html(&url) {
                Ok(html) => html_response(html),
                Err(err) => html_response(render_error_page(&err.to_string())),
            };

            let _ = request.respond(response);
        });
    }

    Ok(())
}

fn fetch_sheet_html(url: &str) -> Result<String, Box<dyn Error>> {
    let body = reqwest::blocking::get(url)?.error_for_status()?.text()?;
    let mut reader = ReaderBuilder::new().has_headers(false).from_reader(Cursor::new(body));

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
         </style></head><body><h1>Spreadsheet Viewer</h1><div class=\"wrap\"><table>",
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
