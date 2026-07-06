# AGENTS.md — Google Cloud Spanner adapter (GoogleSQL)

Whole-adapter guidance for the experimental Spanner adapter. It lives in the
Spanner auth module because that is the one Spanner-dedicated Rust directory, but
it covers **all** Spanner code: the macros in
`crates/dbt-loader/src/dbt_macro_assets/dbt-spanner/`, and the `Spanner` match
arms across `dbt-adapter`, `dbt-adapter-sql`, `dbt-schemas`, `dbt-adbc`, and this
crate. Read it before touching any of them. The user-facing version is
`docs/adapters/spanner.md`.

Spanner is **experimental**, but the ADBC driver is now integrated and the
adapter runs end-to-end against the Spanner emulator (view, table, and
incremental including the delete+insert path). See **Driver status** below for
the tested runtime facts before changing transaction/DDL behavior.

## Guiding principle

Spanner speaks **GoogleSQL**, the same dialect family as BigQuery. The adapter
reuses `Dialect::Bigquery` for parsing/static analysis and, in the Rust layer,
**mirrors BigQuery's behavioral match arms** (grep `Spanner` — it almost always
sits next to `Bigquery`). When adding a new adapter capability, default to "do
what BigQuery does" unless one of the oddities below says otherwise.

But Spanner is **not** BigQuery. The differences below are the whole reason this
adapter exists as its own thing rather than an alias.

## Spanner oddities that change dbt behavior

- **No `CREATE TABLE AS SELECT`.** Tables are created with an explicit column
  schema and a **mandatory `PRIMARY KEY`**. There is no `CREATE OR REPLACE TABLE`.
  Table models must set `primary_key` in config
  (`{{ config(materialized='table', primary_key=['id']) }}`).
- **DDL and DML cannot mix.** Schema changes go through async `UpdateDatabaseDdl`;
  row writes go through a read-write transaction. You cannot run
  `CREATE TABLE ...; INSERT ...` as one batch/transaction. This is why the table
  build is a custom `materialization table, adapter='spanner'` that runs
  drop → create (DDL) → insert (DML) as **separate** statements (see
  `dbt-spanner/macros/materializations/table.sql`). Precedent: dbt-fabric does a
  similar custom table materialization.
  Transaction convention in the Spanner macros: **DDL** statements use
  `statement(..., auto_begin=False)` (autocommit — Spanner rejects DDL in a
  transaction); **DML** statements use the default `auto_begin=True` and share the
  one transaction that the trailing `adapter.commit()` closes. The delete+insert
  upsert is emitted as a single `statement('main')` block (a `;`-separated
  `DELETE; INSERT`) that the driver runs atomically via `ExecuteBatchDml`. The
  driver honors `auto_begin` accordingly; this is verified against the emulator.
- **No `MERGE`.** Incremental models support only `append` and `delete_insert`
  (custom `materialization incremental, adapter='spanner'` in
  `dbt-spanner/macros/materializations/incremental.sql`). `delete_insert` uses a
  correlated `EXISTS` delete (composite unique keys) + insert, with the model SQL
  inlined as the source (no temp table — Spanner has none). Rust
  `valid_incremental_strategies` returns `[Append, DeleteInsert]` for Spanner.
- **No `TRUNCATE`.** `spanner__truncate_relation` emulates it with
  `DELETE ... WHERE true`.
- **No `DATETIME` type.** Only `TIMESTAMP` and `DATE`. Date-math macros use
  `TIMESTAMP_ADD`/`TIMESTAMP_DIFF`, which only support dateparts up to `DAY`;
  `WEEK`/`MONTH`/`QUARTER`/`YEAR` need `DATE_ADD`/`DATE_DIFF` on a `DATE` (TODO).
  Do NOT reintroduce BigQuery's `DATETIME_ADD`/`cast(... as datetime)`.
- **Sized string/bytes.** DDL requires `STRING(MAX)` / `BYTES(MAX)` (or an
  explicit length), not bare `STRING`. Query introspection may return unsized
  types — sizing them for `CREATE TABLE` is an open TODO.
- **Views require `SQL SECURITY INVOKER`.** `spanner__create_view_as` must emit
  `create or replace view ... sql security invoker as ...`. Views cannot be
  renamed (drop + recreate).
- **Named schemas** are a newer, differently-shaped feature. The default schema
  is the unnamed/empty schema `''`. `create_schema`/`drop_schema` DDL here is
  deliberately minimal; the end-to-end fixture exercises only the default
  unnamed schema, so named-schema DDL has not been run against a live database.
- **No partition/cluster** in the BigQuery sense — do not port `partition_by` /
  `cluster_by` / options macros. (Interleaving is a different concept; don't
  attempt it.)
- **Quoting** uses backticks, like BigQuery.
- **INFORMATION_SCHEMA** is queryable (`information_schema.tables/columns/schemata`,
  columns `table_catalog`/`table_schema`/`table_name`, and `spanner_type` for the
  canonical column type string).
- **BigQuery-API-only operations are N/A:** Python models, dataset ACL grants
  (`grant_access_to`), dataset-location lookup, dataproc. Spanner's own macro
  package gates which adapter methods get invoked, so the BigQuery-grouped Rust
  arms for these are currently unreachable from Spanner — leave them unless you
  are giving Spanner a real, different implementation.

## Connection model

`project` → `instance` → `database`, plus an optional named `schema`. This maps
to dbt's `database` = Spanner database and `schema` = named schema. Auth reuses
the Google Cloud families (ADC/oauth, service account, keyfile-json, temp token)
— see `mod.rs` in this directory. **Auth changes require human verification** per
`crates/dbt-auth/AGENTS.md`.

## Emulator awareness: connection-aware, dialect-blind

The Spanner emulator is a faithful drop-in subset of production Spanner at the
API/SQL level. **Do NOT make the adapter emulator-aware in any SQL/dialect/
materialization code.** The whole value of the emulator is that the *same*
adapter code runs against it and production; branching SQL on "am I on the
emulator" would test a code path that never runs in production (untested
divergence), defeating the point.

What actually differs about the emulator is only **how you connect** — a local
gRPC endpoint and anonymous credentials instead of ADC. That is a driver /
connection-config concern, and it already lives there and ONLY there:
`adbc.spanner.endpoint` / `adbc.spanner.emulator` (dbt-adbc/src/spanner.rs), the
auth module forwarding them, and the `emulator`/`api_endpoint` profile fields.
The driver also auto-detects `SPANNER_EMULATOR_HOST`. There are **zero** emulator
references in the macros or in any Rust dialect/behavior arm — keep it that way.
An `if emulator` in a macro or a `match adapter_type` arm is a smell: the thing
you want belongs in the driver/connection config instead.

Caveat: the emulator is faithful but not complete — it has documented feature
gaps vs production (some functions, parts of INFORMATION_SCHEMA, historically
named schemas / constraint enforcement, single-writer semantics, different error
text). Handle those as **skipped/xfail tests** (a test-environment concern), not
as code branches. When an emulator test fails, first decide "emulator gap vs
production" before assuming the adapter is wrong.

## Driver status: WORKING end-to-end

The adapter runs end-to-end on the real
[fornwall/adbc-spanner](https://github.com/fornwall/adbc-spanner) driver against
the Spanner emulator — view, table, and incremental (first-run + delete+insert)
all succeed. Library `adbc_spanner` (`libadbc_spanner.so`), entrypoint
`AdbcSpannerInit`, options in `crates/dbt-adbc/src/spanner.rs`
(`adbc.spanner.database|endpoint|emulator|keyfile|keyfile_json`). The pinned
commit's shared lib goes in the repo `lib/`. See the `spanner-driver-integration`
memory for the exact run recipe and the fixes that made it work.

Runtime facts that shaped the code (don't regress these):
- The driver runs **DML via `execute_update` (a read/write txn)** and queries via
  a single-use read txn that rejects DML. `is_update_statement`
  (`dbt-adapter-sql/statements.rs`) flags Spanner DML so `adapter_engine.rs` calls
  `execute_update`. DDL is auto-routed to `UpdateDatabaseDdl` on either path.
- The driver runs a **`;`-separated DML batch atomically via `ExecuteBatchDml`**
  in one `execute_update` transaction (driver >= 0.2.0), which is how the
  incremental `delete_insert` upsert emits `DELETE; INSERT` as a single
  `statement('main')`. DDL, by contrast, must not be batched with DML.
- Spanner DDL needs **`STRING(MAX)`/`BYTES(MAX)`** (not bare STRING/BYTES), **no
  `CASCADE`** on DROP, and **cannot rename views** (the view materialization uses
  `CREATE OR REPLACE VIEW` directly).
- Relations render **without a database prefix** and **skip an empty schema**.
- Emulator gap: a top-level `WITH` model fails column introspection (`WITH is not
  supported on subqueries` — likely emulator-only). Do not branch on the emulator
  to fix it (see the emulator-awareness rule above).
