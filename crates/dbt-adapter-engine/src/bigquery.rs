use adbc_core::options::OptionValue;
use dbt_adbc::bigquery::QUERY_USE_STORAGE_API_DISABLED_CLIENT;
use dbt_auth::AdapterConfig;

use crate::Options;

/// Default custom REST endpoints to row-based result fetching, as in dbt Core 1.x.
pub fn apply_statement_options(config: &AdapterConfig, options: &mut Options) {
    if config.get_str("api_endpoint").is_some()
        && config.get_str("storage_read_api_endpoint").is_none()
        && !options
            .iter()
            .any(|(key, _)| key == QUERY_USE_STORAGE_API_DISABLED_CLIENT)
    {
        options.push((
            QUERY_USE_STORAGE_API_DISABLED_CLIENT.to_owned(),
            OptionValue::String("true".to_owned()),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_yaml::Mapping;

    fn config(yaml: &str) -> AdapterConfig {
        AdapterConfig::new(dbt_yaml::from_str::<Mapping>(yaml).unwrap())
    }

    fn string_options(options: &Options) -> Vec<(&str, &str)> {
        options
            .iter()
            .map(|(key, value)| match value {
                OptionValue::String(value) => (key.as_str(), value.as_str()),
                _ => panic!("expected a string option: {key}"),
            })
            .collect()
    }

    #[test]
    fn custom_rest_endpoint_uses_rest_results() {
        for yaml in [
            "api_endpoint: http://localhost:9050/",
            "api_endpoint: https://proxy.example.com/",
            "api_endpoint: http://localhost:9050/\nstorage_read_api_endpoint: null",
        ] {
            let mut options = Options::new();
            apply_statement_options(&config(yaml), &mut options);
            assert_eq!(
                string_options(&options),
                vec![(QUERY_USE_STORAGE_API_DISABLED_CLIENT, "true")],
                "{yaml}"
            );
        }
    }

    #[test]
    fn default_and_explicit_storage_endpoints_keep_storage_results() {
        for yaml in [
            "{}",
            "api_endpoint: null",
            "storage_read_api_endpoint: http://localhost:9060/",
            "api_endpoint: http://localhost:9050/\nstorage_read_api_endpoint: http://localhost:9060/",
        ] {
            let mut options = Options::new();
            apply_statement_options(&config(yaml), &mut options);
            assert!(options.is_empty(), "{yaml}");
        }
    }

    #[test]
    fn explicit_statement_options_take_precedence() {
        for value in ["true", "false", "1", "0"] {
            let mut options = vec![
                (
                    dbt_adbc::bigquery::QUERY_LABELS.to_owned(),
                    OptionValue::String(r#"{"invocation_id":"test"}"#.to_owned()),
                ),
                (
                    QUERY_USE_STORAGE_API_DISABLED_CLIENT.to_owned(),
                    OptionValue::String(value.to_owned()),
                ),
            ];
            apply_statement_options(
                &config("api_endpoint: http://localhost:9050/"),
                &mut options,
            );
            assert_eq!(
                string_options(&options),
                vec![
                    (
                        dbt_adbc::bigquery::QUERY_LABELS,
                        r#"{"invocation_id":"test"}"#
                    ),
                    (QUERY_USE_STORAGE_API_DISABLED_CLIENT, value),
                ]
            );
        }
    }
}
