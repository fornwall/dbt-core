//! End-to-end integration test for the Google Cloud Spanner adapter against a
//! **real** Cloud Spanner instance (not the emulator).
//!
//! It runs the committed `spanner_test` dbt project (a view, a table with a
//! primary key, and an incremental model) against a live Spanner database with
//! the real `adbc-spanner` driver — asserting `dbt run` succeeds, including the
//! incremental delete+insert path on a second run.
//!
//! This is a MANUAL-ONLY test. It is `#[ignore]` and is intentionally referenced
//! by no CI workflow (unlike the emulator test in `spanner_emulator.rs`), because
//! it needs live Google Cloud credentials and mutates a real database. Nothing in
//! `.github/workflows/` runs it — run it yourself.
//!
//! Requirements:
//!   - the `adbc-spanner` shared library in the repo `lib/` directory
//!     (`libadbc_spanner.so`), same as the emulator test;
//!   - a reachable Spanner instance + database you are allowed to write to;
//!   - Application Default Credentials in the environment (e.g. via
//!     `gcloud auth application-default login`, a `GOOGLE_APPLICATION_CREDENTIALS`
//!     key file, or workload identity), or an explicit key file passed through
//!     `SPANNER_GCP_KEYFILE`.
//!
//! Configuration (environment variables):
//!   - `SPANNER_GCP_PROJECT`   (required) — GCP project id
//!   - `SPANNER_GCP_INSTANCE`  (required) — Spanner instance id
//!   - `SPANNER_GCP_DATABASE`  (required) — Spanner database id
//!   - `SPANNER_GCP_KEYFILE`   (optional) — path to a service-account key file;
//!                                          omit to use Application Default Credentials
//!
//! Run it with:
//!
//! ```sh
//! export SPANNER_GCP_PROJECT=my-project
//! export SPANNER_GCP_INSTANCE=my-instance
//! export SPANNER_GCP_DATABASE=my-db
//! cargo test -p dbt-sa-cli --test spanner_gcp -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

#[tokio::test]
#[ignore = "manual-only: requires a real Spanner instance + GCP credentials and the adbc-spanner driver in lib/"]
async fn spanner_gcp_end_to_end() {
    if let Err(err) = run_dbt_against_spanner() {
        panic!("Spanner (real GCP) end-to-end test failed: {err}");
    }
}

fn run_dbt_against_spanner() -> Result<(), String> {
    let project = require_env("SPANNER_GCP_PROJECT")?;
    let instance = require_env("SPANNER_GCP_INSTANCE")?;
    let database = require_env("SPANNER_GCP_DATABASE")?;
    let keyfile = std::env::var("SPANNER_GCP_KEYFILE").ok();

    // Run in a throwaway copy so dbt's `target/`/`logs/` never touch the committed
    // fixture, and so we can drop in a real-GCP `profiles.yml` without editing it.
    let project_dir = std::env::temp_dir().join(format!("spanner_gcp_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&project_dir);
    copy_dir(&fixture_dir(), &project_dir).map_err(|e| format!("copying fixture failed: {e}"))?;
    write_profiles(&project_dir, &project, &instance, &database, keyfile.as_deref())?;

    // First run: view, table, and incremental (first-run create + insert).
    let out = run_dbt(&["run"], &project_dir)?;
    assert_dbt_succeeded(&out, "initial run")?;

    // Second run of the incremental model exercises the delete+insert path.
    let out = run_dbt(&["run", "--select", "spanner_incremental"], &project_dir)?;
    assert_dbt_succeeded(&out, "incremental delete+insert run")?;

    let _ = std::fs::remove_dir_all(&project_dir);
    Ok(())
}

fn require_env(name: &str) -> Result<String, String> {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => Ok(v),
        _ => Err(format!(
            "environment variable {name} must be set to run this manual test \
             (SPANNER_GCP_PROJECT / SPANNER_GCP_INSTANCE / SPANNER_GCP_DATABASE are all required)"
        )),
    }
}

/// Write a real-GCP `profiles.yml` into the copied project (emulator disabled,
/// no `api_endpoint`), overwriting the emulator profile shipped in the fixture.
fn write_profiles(
    project_dir: &Path,
    project: &str,
    instance: &str,
    database: &str,
    keyfile: Option<&str>,
) -> Result<(), String> {
    let keyfile_line = match keyfile {
        Some(path) => format!("\n      keyfile: {path}"),
        None => String::new(),
    };
    let profiles = format!(
        "spanner_test:\n  \
         target: dev\n  \
         outputs:\n    \
         dev:\n      \
         type: spanner\n      \
         project: {project}\n      \
         instance: {instance}\n      \
         database: {database}\n      \
         schema: \"\"\n      \
         emulator: false{keyfile_line}\n      \
         threads: 1\n"
    );
    std::fs::write(project_dir.join("profiles.yml"), profiles)
        .map_err(|e| format!("writing profiles.yml failed: {e}"))
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

fn run_dbt(args: &[&str], project_dir: &Path) -> Result<String, String> {
    // No `SPANNER_EMULATOR_HOST` here: the driver resolves real credentials
    // (Application Default Credentials or the configured keyfile) and talks to
    // production Spanner. The child inherits the ambient environment, so
    // `GOOGLE_APPLICATION_CREDENTIALS` / gcloud ADC are picked up automatically.
    let output = Command::new(env!("CARGO_BIN_EXE_dbt-sa-cli"))
        .args(args)
        .arg("--project-dir")
        .arg(project_dir)
        .arg("--profiles-dir")
        .arg(project_dir)
        .output()
        .map_err(|e| format!("failed to run dbt-sa-cli: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        return Err(format!(
            "dbt {args:?} exited with {}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
            output.status
        ));
    }
    Ok(format!("{stdout}\n{stderr}"))
}

fn assert_dbt_succeeded(output: &str, phase: &str) -> Result<(), String> {
    if output.to_lowercase().contains("error") || output.contains("Failed") {
        return Err(format!("dbt reported an error during {phase}:\n{output}"));
    }
    if !output.contains("Succeeded") {
        return Err(format!(
            "dbt did not report any successful models during {phase}:\n{output}"
        ));
    }
    Ok(())
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("spanner_test")
}
