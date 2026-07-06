//! End-to-end integration test for the Google Cloud Spanner adapter.
//!
//! Boots the Spanner emulator in a container (via `dbt-test-containers`), creates
//! a test instance + database, then runs the committed `spanner_test` dbt project
//! (a view, a table with a primary key, and an incremental model) against it with
//! the real `adbc-spanner` driver — asserting `dbt run` succeeds, including the
//! incremental delete+insert path on a second run.
//!
//! This test is `#[ignore]` because it requires Docker and the `adbc-spanner`
//! shared library in the repo `lib/` directory (`libadbc_spanner.so`). Run it with:
//!
//! ```sh
//! cargo test -p dbt-sa-cli --test spanner_emulator -- --ignored --nocapture
//! # or, in CI:
//! cargo nextest run -p dbt-sa-cli --test spanner_emulator --run-ignored ignored-only
//! ```
//!
//! The non-required CI job `.github/workflows/spanner-emulator-tests.yml` fetches
//! the pinned driver library into `lib/` and runs this test.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use dbt_test_containers::container::docker::{
    ContainerConfig, PortBinding, initialize_container, shutdown_container,
};

const EMULATOR_IMAGE: &str = "gcr.io/cloud-spanner-emulator/emulator";
const REST_ADMIN: &str = "http://localhost:9020";
const GRPC_HOST: &str = "localhost:9010";
const PROJECT: &str = "test-project";
const INSTANCE: &str = "test-instance";
const DATABASE: &str = "test-db";

#[tokio::test]
#[ignore = "requires Docker + the adbc-spanner driver in lib/ (boots the Spanner emulator)"]
async fn spanner_emulator_end_to_end() {
    let mut port_bindings = HashMap::new();
    for port in ["9010/tcp", "9020/tcp"] {
        port_bindings.insert(
            port.to_string(),
            Some(vec![PortBinding {
                host_ip: Some("0.0.0.0".to_string()),
                host_port: Some(port.to_string()),
            }]),
        );
    }

    let config = ContainerConfig {
        image_name_base: "spanner-emulator".to_string(),
        image_uri: Some(EMULATOR_IMAGE.to_string()),
        dockerfile_path: None,
        ro_mount_paths: vec![],
        rw_mount_path: None,
        port_bindings,
        network_mode: None,
        reuse_latest: false,
        container_id: None,
        cmd: None,
        env: vec![],
        build_args: vec![],
        bind_user: false,
    };

    let container = initialize_container(config)
        .await
        .expect("failed to start the Spanner emulator container");

    // Run the actual test body, always tearing the container down afterward.
    let outcome = run_dbt_against_emulator();
    let _ = shutdown_container(&container.name).await;
    if let Err(err) = outcome {
        panic!("Spanner emulator end-to-end test failed: {err}");
    }
}

fn run_dbt_against_emulator() -> Result<(), String> {
    wait_for_emulator()?;
    create_instance_and_database()?;

    // Run in a throwaway copy so dbt's `target/`/`logs/` never touch the committed
    // fixture (and it works with a read-only checkout in CI).
    let project_dir = std::env::temp_dir().join(format!("spanner_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&project_dir);
    copy_dir(&fixture_dir(), &project_dir).map_err(|e| format!("copying fixture failed: {e}"))?;

    // First run: view, table, and incremental (first-run create + insert).
    let out = run_dbt(&["run"], &project_dir)?;
    assert_dbt_succeeded(&out, "initial run")?;

    // The table materializes the two upstream rows, and the incremental model is
    // seeded from it (also two rows) on its first, non-incremental run.
    let session = create_session()?;
    assert_count(
        &session,
        "SELECT COUNT(*) AS n FROM spanner_table",
        2,
        "table after initial run",
    )?;
    assert_count(
        &session,
        "SELECT COUNT(*) AS n FROM spanner_incremental",
        2,
        "incremental after initial run",
    )?;

    // Second run of the incremental model exercises the delete+insert path: the
    // delta re-emits id=1 with a new value and adds id=3 (see the fixture model).
    let out = run_dbt(&["run", "--select", "spanner_incremental"], &project_dir)?;
    assert_dbt_succeeded(&out, "incremental delete+insert run")?;

    // Assert delete+insert semantics on a fresh session (strong read of the
    // committed state):
    //   - id=1 is deleted and re-inserted with its new value (not duplicated),
    //   - id=3 is appended,
    //   - id=2 is left untouched.
    // A correct delete+insert yields exactly 3 rows. A plain append of the two
    // delta rows would instead duplicate id=1's primary key (a hard error on
    // Spanner) or, if it somehow didn't, leave 4 rows — either way not 3.
    let session = create_session()?;
    assert_count(
        &session,
        "SELECT COUNT(*) AS n FROM spanner_incremental",
        3,
        "incremental after delete+insert",
    )?;
    assert_scalar(
        &session,
        "SELECT val FROM spanner_incremental WHERE id = 1",
        "HELLO_UPDATED",
        "overlapping key replaced by delete+insert",
    )?;
    assert_scalar(
        &session,
        "SELECT val FROM spanner_incremental WHERE id = 2",
        "world",
        "non-matching row left untouched",
    )?;

    let _ = std::fs::remove_dir_all(&project_dir);
    Ok(())
}

/// Create a Spanner session against the emulator's REST data API (served on the
/// same REST endpoint as the admin API). Returns the fully-qualified session name.
fn create_session() -> Result<String, String> {
    let url = format!(
        "{REST_ADMIN}/v1/projects/{PROJECT}/instances/{INSTANCE}/databases/{DATABASE}/sessions"
    );
    let body = curl_post_capture(&url, "{}")?;
    let v: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| format!("parsing session response failed: {e}\n{body}"))?;
    v["name"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("no session name in response: {body}"))
}

/// Run `sql` via `sessions.executeSql` and return the first column of the first
/// row as a string (Spanner encodes INT64 as a decimal string in JSON).
fn query_scalar(session: &str, sql: &str) -> Result<String, String> {
    let url = format!("{REST_ADMIN}/v1/{session}:executeSql");
    let body = serde_json::json!({ "sql": sql }).to_string();
    let resp = curl_post_capture(&url, &body)?;
    let v: serde_json::Value = serde_json::from_str(&resp)
        .map_err(|e| format!("parsing executeSql response failed: {e}\n{resp}"))?;
    v["rows"][0][0]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("no scalar value in executeSql response for `{sql}`: {resp}"))
}

fn assert_count(session: &str, sql: &str, expected: i64, what: &str) -> Result<(), String> {
    let raw = query_scalar(session, sql)?;
    let got: i64 = raw
        .parse()
        .map_err(|e| format!("{what}: could not parse count `{raw}`: {e}"))?;
    if got != expected {
        return Err(format!("{what}: expected {expected} rows, got {got}"));
    }
    Ok(())
}

fn assert_scalar(session: &str, sql: &str, expected: &str, what: &str) -> Result<(), String> {
    let got = query_scalar(session, sql)?;
    if got != expected {
        return Err(format!("{what}: expected `{expected}`, got `{got}`"));
    }
    Ok(())
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

/// Poll the emulator's REST admin API until it responds (or time out).
fn wait_for_emulator() -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(90);
    let url = format!("{REST_ADMIN}/v1/projects/{PROJECT}/instances");
    while Instant::now() < deadline {
        let ok = Command::new("curl")
            .args(["-sf", "-o", "/dev/null", &url])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err("Spanner emulator REST admin API did not become ready in time".to_string())
}

fn create_instance_and_database() -> Result<(), String> {
    let instance_body = format!(
        r#"{{"instanceId":"{INSTANCE}","instance":{{"config":"projects/{PROJECT}/instanceConfigs/emulator-config","displayName":"test","nodeCount":1}}}}"#
    );
    curl_post(
        &format!("{REST_ADMIN}/v1/projects/{PROJECT}/instances"),
        &instance_body,
    )?;

    let db_body = format!(r#"{{"createStatement":"CREATE DATABASE `{DATABASE}`"}}"#);
    curl_post(
        &format!("{REST_ADMIN}/v1/projects/{PROJECT}/instances/{INSTANCE}/databases"),
        &db_body,
    )?;
    Ok(())
}

fn curl_post(url: &str, body: &str) -> Result<(), String> {
    curl_post_capture(url, body).map(|_| ())
}

/// POST `body` as JSON to `url` and return the response body on success.
fn curl_post_capture(url: &str, body: &str) -> Result<String, String> {
    let output = Command::new("curl")
        .args([
            "-sS",
            "-X",
            "POST",
            url,
            "-H",
            "Content-Type: application/json",
            "-d",
            body,
        ])
        .output()
        .map_err(|e| format!("failed to run curl: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "curl POST {url} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn run_dbt(args: &[&str], project_dir: &Path) -> Result<String, String> {
    let output = Command::new(env!("CARGO_BIN_EXE_dbt-sa-cli"))
        .args(args)
        .arg("--project-dir")
        .arg(project_dir)
        .arg("--profiles-dir")
        .arg(project_dir)
        .env("SPANNER_EMULATOR_HOST", GRPC_HOST)
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
