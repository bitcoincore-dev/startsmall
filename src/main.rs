use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use csv::ReaderBuilder;
use git2::{IndexAddOption, Repository, Signature};
use sha2::{Digest, Sha256};

const DEFAULT_SHEET_CSV_URL: &str = "https://docs.google.com/spreadsheets/d/1-eGxq2mMoEGwgSpNVL5j2sa6ToojZUZ-Zun8h2oBAR4/export?format=csv&gid=0";
const SNAPSHOT_DIR: &str = "sheet-snapshots";
const SNAPSHOT_CSV: &str = "sheet-snapshots/google-sheet.csv";
const SNAPSHOT_META: &str = "sheet-snapshots/google-sheet.meta";

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let sheet_url = env::var("SHEET_CSV_URL").unwrap_or_else(|_| DEFAULT_SHEET_CSV_URL.to_string());
    let snapshot = fetch_snapshot(&sheet_url)?;
    let repo = Repository::discover(".")?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| "repository must not be bare".to_string())?;

    let csv_path = workdir.join(SNAPSHOT_CSV);
    let meta_path = workdir.join(SNAPSHOT_META);

    fs::create_dir_all(workdir.join(SNAPSHOT_DIR))?;
    fs::write(&csv_path, &snapshot.csv)?;
    fs::write(&meta_path, snapshot.meta.as_bytes())?;

    let commit_id = commit_snapshot(&repo, &[Path::new(SNAPSHOT_CSV), Path::new(SNAPSHOT_META)])?;

    println!("Fetched {} rows from {}", snapshot.rows, sheet_url);
    println!("Snapshot hash: {}", snapshot.sha256);
    println!("Committed snapshot: {}", commit_id);

    Ok(())
}

struct Snapshot {
    csv: String,
    meta: String,
    rows: usize,
    sha256: String,
}

fn fetch_snapshot(url: &str) -> Result<Snapshot, Box<dyn Error + Send + Sync>> {
    let body = reqwest::blocking::get(url)?.error_for_status()?.text()?;
    let mut reader = ReaderBuilder::new().has_headers(false).from_reader(body.as_bytes());

    let mut rows = 0usize;
    let mut digest = Sha256::new();
    digest.update(body.as_bytes());

    for record in reader.records() {
        record?;
        rows += 1;
    }

    let sha256 = format!("{:x}", digest.finalize());
    let meta = format!(
        "source_url={}\nsha256={}\nrows={}\n",
        url, sha256, rows
    );

    Ok(Snapshot {
        csv: body,
        meta,
        rows,
        sha256,
    })
}

fn commit_snapshot(repo: &Repository, paths: &[&Path]) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut index = repo.index()?;
    index.add_all(paths, IndexAddOption::DEFAULT, None)?;
    index.write()?;

    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;
    let signature = signature(repo)?;

    if let Ok(head) = repo.head() {
        let parent = head.peel_to_commit()?;
        if parent.tree_id() == tree_id {
            return Ok(parent.id().to_string());
        }

        let commit_id = repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            "sync: snapshot Google Sheet document",
            &tree,
            &[&parent],
        )?;
        Ok(commit_id.to_string())
    } else {
        let commit_id = repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            "sync: snapshot Google Sheet document",
            &tree,
            &[],
        )?;
        Ok(commit_id.to_string())
    }
}

fn signature(repo: &Repository) -> Result<Signature<'_>, Box<dyn Error + Send + Sync>> {
    if let Ok(sig) = repo.signature() {
        return Ok(sig);
    }

    Ok(Signature::now("Start Small Bot", "start-small-bot@example.com")?)
}
