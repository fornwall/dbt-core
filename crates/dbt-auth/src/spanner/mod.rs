//! Google Cloud Spanner authentication (GoogleSQL dialect).
//!
//! Targets the `adbc-spanner` driver (<https://github.com/fornwall/adbc-spanner>).
//! The driver resolves credentials itself: a service-account key file
//! (`keyfile`) or inline JSON (`keyfile_json`), else Application Default
//! Credentials against production Spanner, or anonymous credentials in emulator
//! mode. So this layer assembles the database path plus the optional
//! endpoint/emulator/keyfile options and hands them to the driver.
//!
//! Per crates/dbt-auth/AGENTS.md, auth changes require human verification.

use crate::{AdapterConfig, Auth, AuthError, AuthOutcome, auth_configure_pipeline};
use database::Builder as DatabaseBuilder;
use dbt_adbc::{Backend, database, spanner};

pub struct SpannerAuth;

/// Resolved Spanner connection target. The driver does its own credential
/// resolution (ADC / emulator), so there is no credential material here.
struct SpannerConnection {
    /// `projects/<p>/instances/<i>/databases/<d>`
    database_path: String,
    endpoint: Option<String>,
    emulator: Option<bool>,
    keyfile: Option<String>,
    keyfile_json: Option<String>,
}

impl SpannerConnection {
    fn apply(self, mut builder: DatabaseBuilder) -> Result<DatabaseBuilder, AuthError> {
        builder.with_named_option(spanner::DATABASE, self.database_path)?;
        if let Some(endpoint) = self.endpoint {
            builder.with_named_option(spanner::ENDPOINT, endpoint)?;
        }
        if let Some(emulator) = self.emulator {
            builder.with_named_option(spanner::EMULATOR, emulator.to_string())?;
        }
        if let Some(keyfile) = self.keyfile {
            builder.with_named_option(spanner::KEYFILE, keyfile)?;
        }
        if let Some(keyfile_json) = self.keyfile_json {
            builder.with_named_option(spanner::KEYFILE_JSON, keyfile_json)?;
        }
        Ok(builder)
    }
}

fn parse_auth(config: &AdapterConfig) -> Result<SpannerConnection, AuthError> {
    let project = config
        .get_str("project")
        .ok_or_else(|| AuthError::config("Missing required field 'project' in Spanner config"))?;
    let instance = config
        .get_str("instance")
        .ok_or_else(|| AuthError::config("Missing required field 'instance' in Spanner config"))?;
    let database = config
        .get_str("database")
        .ok_or_else(|| AuthError::config("Missing required field 'database' in Spanner config"))?;

    let database_path = format!("projects/{project}/instances/{instance}/databases/{database}");

    // `api_endpoint` maps to the driver's endpoint option (e.g. a Spanner emulator).
    let endpoint = config
        .get_str("api_endpoint")
        .or_else(|| config.get_str("endpoint"))
        .map(|s| s.to_string());

    let emulator = config
        .get_string("emulator")
        .map(|s| matches!(s.as_ref(), "true" | "1" | "True"));

    let keyfile = config.get_str("keyfile").map(|s| s.to_string());

    // `keyfile_json` may be given as an inline JSON string or as a YAML mapping;
    // the driver wants a JSON string either way.
    let keyfile_json = match config.get("keyfile_json") {
        Some(dbt_yaml::Value::String(s, _)) => Some(s.to_string()),
        Some(value @ dbt_yaml::Value::Mapping(_, _)) => {
            let json: serde_json::Value = dbt_yaml::from_value(value.clone())?;
            Some(json.to_string())
        }
        Some(_) => {
            return Err(AuthError::config(
                "'keyfile_json' must be a JSON string or a YAML mapping",
            ));
        }
        None => None,
    };

    Ok(SpannerConnection {
        database_path,
        endpoint,
        emulator,
        keyfile,
        keyfile_json,
    })
}

fn apply_connection_args(
    _config: &AdapterConfig,
    builder: DatabaseBuilder,
) -> Result<DatabaseBuilder, AuthError> {
    Ok(builder)
}

impl Auth for SpannerAuth {
    fn backend(&self) -> Backend {
        Backend::Spanner
    }

    fn configure(&self, config: &AdapterConfig) -> Result<AuthOutcome, AuthError> {
        auth_configure_pipeline!(self.backend(), &config, parse_auth, apply_connection_args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_options::other_option_value;
    use dbt_yaml::Mapping;

    fn try_configure(config: Mapping) -> Result<database::Builder, AuthError> {
        SpannerAuth {}
            .configure(&AdapterConfig::new(config))
            .map(|r| r.builder)
    }

    fn base_config() -> Mapping {
        Mapping::from_iter([
            ("project".into(), "my-project".into()),
            ("instance".into(), "my-instance".into()),
            ("database".into(), "my-db".into()),
        ])
    }

    #[test]
    fn test_builds_database_path() {
        let builder = try_configure(base_config()).unwrap();
        assert_eq!(
            other_option_value(&builder, spanner::DATABASE).unwrap(),
            "projects/my-project/instances/my-instance/databases/my-db"
        );
    }

    #[test]
    fn test_missing_instance_errors() {
        let mut config = base_config();
        config.remove("instance");
        let err = try_configure(config).unwrap_err();
        assert!(err.msg().contains("instance"));
    }

    #[test]
    fn test_endpoint_and_emulator() {
        let mut config = base_config();
        config.insert("api_endpoint".into(), "http://localhost:9010".into());
        config.insert("emulator".into(), true.into());
        let builder = try_configure(config).unwrap();
        assert_eq!(
            other_option_value(&builder, spanner::ENDPOINT).unwrap(),
            "http://localhost:9010"
        );
        assert_eq!(
            other_option_value(&builder, spanner::EMULATOR).unwrap(),
            "true"
        );
    }
}
