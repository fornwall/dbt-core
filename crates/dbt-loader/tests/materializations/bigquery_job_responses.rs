use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use arrow_array::{Int64Array, RecordBatch};
use dbt_adapter::AdapterResponse;
use dbt_adapter::load_store::ResultStore;
use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_agate::AgateTable;
use dbt_jinja_ctx::DbtNamespace;
use dbt_schemas::dbt_types::RelationType;
use minijinja::Value;

use crate::macro_test_harness::{MacroTestHarness, default_mock_config, executed_sql};

fn table(rows: usize) -> AgateTable {
    AgateTable::from_record_batch(Arc::new(
        RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from_iter_values(0..rows as i64)) as _,
        )])
        .unwrap(),
    ))
}

fn harness() -> (MacroTestHarness, ResultStore) {
    let mut harness = MacroTestHarness::for_adapter(AdapterType::Bigquery)
        .load_all_macros()
        .with_stub_functions()
        .with_global("dbt_version", Value::from("2.0.0"))
        .with_global("flags", Value::from_serialize(serde_json::json!({"FULL_REFRESH": false})))
        .with_global("dbt", Value::from_object(DbtNamespace::new("dbt")))
        .with_macro(
            "test_project",
            "bq_generate_incremental_build_sql",
            r#"{% macro bq_generate_incremental_build_sql(strategy, tmp_relation, target_relation, sql, unique_key, partition_by, partitions, dest_columns, tmp_relation_exists, copy_partitions, incremental_predicates) %}
                {% do bq_copy_partitions(tmp_relation, target_relation, test_partitions, partition_by) %}
                drop table if exists {{ tmp_relation }}
            {% endmacro %}"#,
        )
        .with_macro(
            "test_project",
            "apply_grants",
            "{% macro apply_grants(relation, grants, should_revoke=False) %}{% endmacro %}",
        )
        .with_macro(
            "test_project",
            "persist_docs",
            "{% macro persist_docs(relation, model) %}{% endmacro %}",
        )
        .with_macro(
            "test_project",
            "create_indexes",
            "{% macro create_indexes(relation) %}{% endmacro %}",
        )
        .with_macro(
            "test_project",
            "bigquery_table_options",
            "{% macro bigquery_table_options(config, model) %}options(){% endmacro %}",
        )
        .build()
        .unwrap();
    let store = ResultStore::default();
    let env = &mut harness.env_mut().env;
    env.add_function("store_result", store.store_result());
    env.add_function("store_raw_result", store.store_raw_result());
    env.add_function("load_result", store.load_result());
    harness.mock().on("execute", |_| {
        Ok(Value::from(vec![
            Value::from_object(
                AdapterResponse::new()
                    .with_query_id("sql-job")
                    .with("job_id", "sql-job"),
            ),
            Value::from_object(AgateTable::default()),
        ]))
    });
    (harness, store)
}

#[test]
fn seed_job_response_survives_noop_and_post_hooks() {
    for (rows, job_id, full_refresh) in [
        (2, Some("load-job"), false),
        (2, Some("load-job"), true),
        (2, None, false),
        (0, None, false),
    ] {
        let (mut harness, store) = harness();
        harness
            .env_mut()
            .env
            .add_function("load_agate_table", move || Value::from_object(table(rows)));
        harness.mock().on("get_relation", |_| Ok(Value::from(())));
        harness.mock().on("commit", |_| Ok(Value::from(())));
        harness
            .mock()
            .on("convert_type", |_| Ok(Value::from("INT64")));
        harness
            .mock()
            .on("quote_seed_column", |args| Ok(args[0].clone()));
        harness.mock().on("load_dataframe", move |_| {
            let mut response = AdapterResponse::new()
                .with_code("LOAD")
                .with_rows_affected(99);
            if let Some(job_id) = job_id {
                response = response.with_query_id(job_id).with("job_id", job_id);
            }
            Ok(Value::from_object(
                response.with("project_id", "billing-project"),
            ))
        });
        let config = default_mock_config();
        config.on("get", move |args| {
            Ok(match args[0].as_str() {
                Some("full_refresh") => Value::from(full_refresh),
                _ => args.get(1).cloned().unwrap_or(Value::from(())),
            })
        });
        let mut ctx = harness
            .materialization_context("seed", "")
            .config(Value::from_dyn_object(config))
            .build();
        ctx.insert("model".into(), Value::from_serialize(serde_json::json!({
            "alias": "seed", "unique_id": "seed.test.seed", "database": "db",
            "schema": "schema", "project_root": "/project/", "original_file_path": "seeds/seed.csv",
            "config": {}, "columns": {}, "batch": null
        })));
        harness.render("{{ materialization_seed_default() }}{% call statement('post_hook') %}select 1{% endcall %}", ctx).unwrap();
        let response = store.main_adapter_response().unwrap();
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["job_id"].as_str(), job_id);
        assert_eq!(response.rows_affected(), rows as u64);
        let code = if full_refresh { "CREATE" } else { "INSERT" };
        assert_eq!(response.code(), code);
        assert_eq!(response.message(), format!("{code} {rows}"));
        assert_eq!(response.query_id().as_deref(), job_id);
        if rows > 0 {
            assert_eq!(json["project_id"], "billing-project");
        } else {
            harness
                .mock()
                .observed_calls()
                .assert_not_called("load_dataframe");
        }
    }
}

#[test]
fn partition_copy_job_response_survives_cleanup() {
    for partitions in [0, 1, 2] {
        let (harness, store) = harness();
        let existing = harness.relation("db", "schema", "model", Some(RelationType::Table));
        harness.mock().on("get_relation", move |_| {
            Ok(RelationObject::new(existing.clone()).into_value())
        });
        harness.mock().on("get_columns_in_relation", |_| {
            Ok(Value::from(Vec::<Value>::new()))
        });
        harness.mock().on("parse_partition_by", |_| {
            Ok(Value::from_serialize(serde_json::json!({
                "copy_partitions": true, "data_type": "int64", "time_ingestion_partitioning": false
            })))
        });
        let calls = Arc::new(AtomicUsize::new(0));
        let copy_calls = calls.clone();
        harness.mock().on("copy_table", move |_| {
            let index = copy_calls.fetch_add(1, Ordering::SeqCst);
            Ok(Value::from_object(
                AdapterResponse::new()
                    .with_code("COPY")
                    .with("job_id", format!("copy-{index}")),
            ))
        });
        let config = default_mock_config();
        config.on("get", |args| {
            Ok(match args[0].as_str() {
                Some("incremental_strategy") => Value::from("insert_overwrite"),
                _ => args.get(1).cloned().unwrap_or(Value::from(())),
            })
        });
        // Exercise the real copy loop and outer materialization while avoiding partition discovery SQL.
        let mut harness = harness;
        harness.env_mut().env.add_global(
            "test_partitions",
            Value::from((0..partitions).collect::<Vec<_>>()),
        );
        let ctx = harness.materialization_context("model", "select 1")
            .config(Value::from_dyn_object(config))
            .with("model", Value::from_serialize(serde_json::json!({"language": "sql", "batch": null, "unique_id": "model.test.model", "columns": {}})))
            .build();
        harness
            .render("{{ materialization_incremental_bigquery() }}", ctx)
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), partitions as usize);
        assert!(
            executed_sql(harness.mock())
                .iter()
                .any(|sql| sql.contains("drop table"))
        );
        let json = serde_json::to_value(store.main_adapter_response().unwrap()).unwrap();
        let expected = if partitions == 0 {
            "sql-job".to_string()
        } else {
            format!("copy-{}", partitions - 1)
        };
        assert_eq!(json["job_id"], expected);
    }
}
