use super::common::{ConfigField, ConfigProcessor, FieldValue, InteractiveSetup};
use dbt_common::FsResult;
use dbt_schemas::schemas::profiles::SpannerDbConfig;
use dbt_schemas::schemas::serde::StringOrInteger;

impl InteractiveSetup for SpannerDbConfig {
    fn get_fields() -> Vec<ConfigField> {
        vec![
            // Core connection settings: project -> instance -> database.
            ConfigField::input("project", "Project ID"),
            ConfigField::input("instance", "Instance ID"),
            ConfigField::input("database", "Database"),
            ConfigField::optional_input(
                "schema",
                "Named schema (leave blank for the default/unnamed schema)",
                None,
            ),
            // Authentication
            ConfigField::select(
                "auth_method",
                "Which authentication method would you like to use?",
                vec!["Service Account (JSON file)", "gcloud oauth"],
                0,
            ),
            ConfigField::input("keyfile", "Path to service account JSON file")
                .when_field_equals("auth_method", FieldValue::Integer(0)),
        ]
    }

    fn set_field(&mut self, field_name: &str, value: FieldValue) -> FsResult<()> {
        match field_name {
            "project" => {
                if let FieldValue::String(s) = value {
                    self.project = Some(s);
                }
            }
            "instance" => {
                if let FieldValue::String(s) = value {
                    self.instance = Some(s);
                }
            }
            "database" => {
                if let FieldValue::String(s) = value {
                    self.database = Some(s);
                }
            }
            "schema" => {
                if let FieldValue::String(s) = value
                    && !s.is_empty()
                {
                    self.schema = Some(s);
                }
            }
            "keyfile" => {
                if let FieldValue::String(s) = value {
                    self.keyfile = Some(s);
                    self.method = Some("service-account".to_string());
                }
            }
            "auth_method" => {
                if let FieldValue::Integer(auth_method) = value {
                    match auth_method {
                        0 => {} // Service account - method will be set when keyfile is provided
                        1 => self.method = Some("oauth".to_string()), // gcloud oauth
                        _ => {}
                    }
                }
            }
            _ => {} // Ignore temporary fields
        }
        Ok(())
    }

    fn get_field(&self, field_name: &str) -> Option<FieldValue> {
        match field_name {
            "project" => self.project.as_ref().map(|s| FieldValue::String(s.clone())),
            "instance" => self
                .instance
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            "database" => self
                .database
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            "schema" => self.schema.as_ref().map(|s| FieldValue::String(s.clone())),
            "keyfile" => self.keyfile.as_ref().map(|s| FieldValue::String(s.clone())),
            "auth_method" => {
                if self.keyfile.is_some() {
                    Some(FieldValue::Integer(0))
                } else if self.method.as_ref().is_some_and(|m| m == "oauth") {
                    Some(FieldValue::Integer(1))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn is_field_set(&self, field_name: &str) -> bool {
        match field_name {
            "project" => self.project.is_some(),
            "instance" => self.instance.is_some(),
            "database" => self.database.is_some(),
            "schema" => self.schema.is_some(),
            "keyfile" => self.keyfile.is_some(),
            _ => false,
        }
    }
}

pub fn setup_spanner_profile(
    existing_config: Option<&SpannerDbConfig>,
) -> FsResult<Box<SpannerDbConfig>> {
    let default_config = SpannerDbConfig {
        threads: None,
        project: None,
        instance: None,
        database: None,
        schema: None,
        method: None,
        keyfile: None,
        keyfile_json: None,
        refresh_token: None,
        client_id: None,
        client_secret: None,
        token_uri: None,
        token: None,
        impersonate_service_account: None,
        scopes: None,
        quota_project: None,
        api_endpoint: None,
        emulator: None,
        retries: None,
        target_name: None,
    };
    let mut config = ConfigProcessor::process_config(existing_config.or(Some(&default_config)))?;

    if config.threads.is_none() {
        config.threads = Some(StringOrInteger::Integer(16));
    }

    Ok(Box::new(config))
}
