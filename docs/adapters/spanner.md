# Google Cloud Spanner adapter (experimental)

Status: **experimental, working.** This adapter targets Google Cloud Spanner's
**GoogleSQL** dialect. Spanner is closely related to BigQuery at the SQL level, so
this adapter reuses BigQuery's dialect for parsing/analysis and mirrors much of
BigQuery's behavior — but Spanner has important differences (see
[Limitations](#limitations)). View, table, and incremental models have been run
end-to-end against the Spanner emulator.

## Installing the ADBC driver

Unlike first-party adapters, the Spanner ADBC driver is **not downloaded
automatically** from the dbt Labs CDN. Install the
[`adbc-spanner`](https://github.com/fornwall/adbc-spanner) driver manually — grab
the prebuilt `libadbc_spanner.so` (or `.dylib`/`.dll`) for your platform from a
release/CI artifact, and put it where dbt can find it (the engine searches an
upward `lib/` directory from the `dbt` binary; you can also place it on your
system library path). The driver is built on the official
[`google-cloud-spanner`](https://github.com/googleapis/google-cloud-rust) Rust
client, exports the `AdbcSpannerInit` entrypoint, and is autocommit-first.

## Configuring `profiles.yml`

Spanner addresses a database hierarchically as `project` → `instance` →
`database`, plus an optional named `schema`.

```yaml
my_project:
  target: dev
  outputs:
    dev:
      type: spanner
      project: my-gcp-project        # GCP project that owns the instance
      instance: my-spanner-instance  # Spanner instance id
      database: my-database          # Spanner database (dbt's "database")
      schema: ""                     # named schema; default is the unnamed schema
      threads: 4

      # Authentication (see below). Defaults to Application Default Credentials.
      method: oauth
```

### Authentication methods

The driver resolves credentials itself:

| Field          | Notes                                                                     |
| -------------- | ------------------------------------------------------------------------- |
| _(none)_       | Application Default Credentials against production Spanner (the default). |
| `keyfile`      | Path to a service-account JSON key file.                                  |
| `keyfile_json` | Inline service-account JSON (a YAML mapping or JSON string).              |
| `emulator`     | `true` to connect to a Spanner emulator with anonymous credentials.       |

Optional connection fields: `api_endpoint` (the driver's endpoint override, e.g.
`http://localhost:9010` for an emulator) and `retries`. The driver also
auto-detects the `SPANNER_EMULATOR_HOST` environment variable.

You can also generate a profile interactively with `dbt init` (choose
**Spanner** from the adapter list).

## Limitations

Spanner's GoogleSQL is **not** identical to BigQuery. Known differences:

- **Tables require a `PRIMARY KEY`.** Spanner has no `CREATE TABLE AS SELECT`, so
  table models build in steps (`CREATE TABLE (...) PRIMARY KEY (...)` then
  `INSERT ... SELECT`, into an intermediate table that is renamed into place). You
  **must** set a primary key on table (and incremental) models:

  ```sql
  {{ config(materialized='table', primary_key=['id']) }}
  ```

  STRING/BYTES columns are sized to `STRING(MAX)`/`BYTES(MAX)` automatically.
  Views use `CREATE OR REPLACE VIEW ... SQL SECURITY INVOKER` (Spanner cannot
  rename views, so they are created in place).
- **No `MERGE`.** Incremental models support `append` and `delete_insert` only.
  `delete_insert` requires a `unique_key` (and is the default when one is set) and
  also needs `primary_key`; it issues the `DELETE` and `INSERT` as separate
  statements. `merge` / `insert_overwrite` / `microbatch` raise a clear error. By
  default the model SQL is applied inline (executed twice per run); set
  `config(incremental_staging_table=true)` to materialize it once into a staging
  table instead (extra DDL, but the model runs once and is deterministic).
- **No `TRUNCATE`.** `truncate_relation` is emulated with `DELETE ... WHERE true`.
- **No `DATETIME` type.** Only `TIMESTAMP` and `DATE` exist. Date-math macros use
  `TIMESTAMP_ADD`/`TIMESTAMP_DIFF`, which only support dateparts up to `DAY`;
  `WEEK`/`MONTH`/`QUARTER`/`YEAR` need `DATE_ADD`/`DATE_DIFF` (TODO).
- **Named schemas** are a newer Spanner feature; `create_schema`/`drop_schema`
  DDL is minimal and should be verified against your instance.
- BigQuery-API-specific adapter operations (Python models, dataset ACL grants,
  dataset location lookup) are not applicable to Spanner and are not implemented.

## Implementation notes

- Adapter type / dialect: `Spanner` reuses `Dialect::Bigquery`
  (`crates/dbt-common/src/adapter.rs`).
- Profile schema: `SpannerDbConfig` in `crates/dbt-schemas/src/schemas/profiles.rs`.
- Auth: `crates/dbt-auth/src/spanner/mod.rs` (see `crates/dbt-auth/CLAUDE.md` —
  auth changes require human verification before release).
- Macros: `crates/dbt-loader/src/dbt_macro_assets/dbt-spanner/`.
