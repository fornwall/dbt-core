{#
  Spanner table materialization. Verified end-to-end against the Spanner emulator
  with the adbc-spanner driver.

  Spanner's GoogleSQL differs from BigQuery in ways that make the standard dbt
  table flow unusable:
    * There is no `CREATE TABLE AS SELECT` — tables are created with an explicit
      column schema and a mandatory `PRIMARY KEY`.
    * DDL (`CREATE TABLE`) and DML (`INSERT`) cannot be mixed in one batch or
      transaction. Schema changes go through `UpdateDatabaseDdl` (asynchronous);
      row mutations go through a read-write transaction.

  The build uses an intermediate table + rename so the existing table is only
  replaced once the new data is fully in place (mirrors dbt's default table
  materialization, adapted for Spanner's DDL/DML split):
    1. CREATE <intermediate> (...columns...) PRIMARY KEY (...) — schema only (DDL).
    2. INSERT INTO <intermediate> ... SELECT ... — populate (DML).
    3. If <target> exists, rename <target> -> <backup> (DDL).
    4. Rename <intermediate> -> <target> (DDL).
    5. Drop <backup> (DDL).

  The primary key is taken from the model `primary_key` config, e.g.:
      {{ config(materialized='table', primary_key=['id']) }}

  Known limitations / open items:
    * Not fully atomic: Spanner DDL is not transactional, so the two renames in
      steps 3-4 are separate async `UpdateDatabaseDdl` operations. There is a
      brief window after step 3 and before step 4 where <target> does not exist.
      (BigQuery has the same limitation.) Batching both renames into one DDL
      request would close the window — needs driver support to express.
    * A failed INSERT (step 2) leaves the original <target> untouched and only
      an orphan <intermediate>, which the next run cleans up. This is the main
      win over the drop-then-create approach.
    * Transaction boundaries: the CREATE runs with auto_begin=False so it executes
      as autocommit DDL (Spanner rejects DDL inside a read-write transaction). The
      driver treats auto_begin=False as autocommit for DDL, as verified end-to-end.
    * Column types: `get_column_schema_from_query` may yield bare `STRING`/`BYTES`;
      Spanner DDL requires a length (e.g. `STRING(MAX)`). Sizing may be needed.
    * Contract path: column ordering for the INSERT column list vs the SELECT.
    * Rename constraints: Spanner `ALTER TABLE ... RENAME TO` can be blocked by
      foreign keys / interleaving referencing the table.
#}

{#-- Resolve and normalize the required primary key from config. --#}
{% macro spanner_get_primary_key() %}
  {%- set primary_key = config.get('primary_key') -%}
  {%- if primary_key is none -%}
    {% do exceptions.raise_compiler_error(
      "Spanner tables require a primary key. Set `primary_key` in the model config, e.g.\n"
      ~ "    {{ config(materialized='table', primary_key=['id']) }}"
    ) %}
  {%- endif -%}
  {%- if primary_key is string -%}
    {%- set primary_key = [primary_key] -%}
  {%- endif -%}
  {{ return(primary_key) }}
{% endmacro %}

{#-- Spanner requires a length on STRING/BYTES DDL columns (BigQuery-style
     introspection yields bare `STRING`/`BYTES`); default them to (MAX). --#}
{% macro spanner_sized_type(data_type) %}
  {%- set upper = data_type | upper | trim -%}
  {%- if upper == 'STRING' -%}
    {{ return('STRING(MAX)') }}
  {%- elif upper == 'BYTES' -%}
    {{ return('BYTES(MAX)') }}
  {%- else -%}
    {{ return(data_type) }}
  {%- endif -%}
{% endmacro %}

{#-- Render "name type, ..." column DDL by introspecting the model query. --#}
{% macro spanner_get_column_ddl_from_query(sql, sql_header=none) %}
  {%- set columns = get_column_schema_from_query(sql, sql_header) -%}
  {%- set ddl = [] -%}
  {%- for col in columns -%}
    {%- do ddl.append(adapter.quote(col.name) ~ " " ~ spanner_sized_type(col.data_type)) -%}
  {%- endfor -%}
  {{ return(ddl | join(",\n      ")) }}
{% endmacro %}

{#-- Introspect just the column names, in query order, for the INSERT column list. --#}
{% macro spanner_get_column_names_from_query(sql, sql_header=none) %}
  {%- set columns = get_column_schema_from_query(sql, sql_header) -%}
  {%- set names = [] -%}
  {%- for col in columns -%}
    {%- do names.append(adapter.quote(col.name)) -%}
  {%- endfor -%}
  {{ return(names) }}
{% endmacro %}

{#-- Schema-only CREATE TABLE. On Spanner this creates an EMPTY table (no data);
     the `table` materialization populates it with a separate INSERT..SELECT. --#}
{% macro spanner__create_table_as(temporary, relation, compiled_code, language='sql') -%}
  {%- if language != 'sql' -%}
    {% do exceptions.raise_compiler_error(
      "The Spanner adapter only supports language='sql', got '" ~ language ~ "'."
    ) %}
  {%- endif -%}

  {%- set primary_key = spanner_get_primary_key() -%}
  {%- set sql_header = config.get('sql_header', none) -%}
  {%- set contract_config = config.get('contract') -%}

  {{ sql_header if sql_header is not none }}

  create table {{ relation }}
  {%- if contract_config.enforced %}
    {#-- Use the enforced contract's column definitions (validated vs the query). --#}
    {{ get_assert_columns_equivalent(compiled_code) }}
    {{ get_table_columns_and_constraints() }}
  {%- else %}
    (
      {{ spanner_get_column_ddl_from_query(compiled_code, sql_header) }}
    )
  {%- endif %}
  {%- set pk_cols = [] -%}
  {%- for key in primary_key -%}
    {%- do pk_cols.append(adapter.quote(key | trim)) -%}
  {%- endfor -%}
  primary key ({{ pk_cols | join(', ') }})
{%- endmacro %}

{% materialization table, adapter='spanner' %}

  {%- set existing_relation = load_cached_relation(this) -%}
  {%- set target_relation = this.incorporate(type='table') -%}

  {#-- Build into an intermediate table, then rename it into place. The
       intermediate/backup relations must not already exist; drop leftovers from a
       previous failed run first. --#}
  {%- set intermediate_relation = make_intermediate_relation(target_relation) -%}
  {%- set preexisting_intermediate_relation = load_cached_relation(intermediate_relation) -%}

  {%- set backup_relation_type = 'table' if existing_relation is none else existing_relation.type -%}
  {%- set backup_relation = make_backup_relation(target_relation, backup_relation_type) -%}
  {%- set preexisting_backup_relation = load_cached_relation(backup_relation) -%}

  {%- set grant_config = config.get('grants') -%}
  {%- set sql_header = config.get('sql_header', none) -%}

  {{ drop_relation_if_exists(preexisting_intermediate_relation) }}
  {{ drop_relation_if_exists(preexisting_backup_relation) }}

  {{ run_hooks(pre_hooks, inside_transaction=False) }}
  {{ run_hooks(pre_hooks, inside_transaction=True) }}

  {#-- 1. Create the empty intermediate table schema with its PRIMARY KEY.
         DDL must run OUTSIDE a transaction on Spanner, so auto_begin=False. --#}
  {% call statement('main', auto_begin=False) -%}
    {{ spanner__create_table_as(False, intermediate_relation, sql) }}
  {%- endcall %}

  {#-- 2. Populate the intermediate table (DML). Spanner INSERT requires an
         explicit column list, in the same order as the SELECT. --#}
  {%- set dest_columns = spanner_get_column_names_from_query(sql, sql_header) -%}
  {% call statement('insert_rows') -%}
    insert into {{ intermediate_relation }} ({{ dest_columns | join(', ') }})
    {{ sql }}
  {%- endcall %}

  {#-- 3. Move the current table aside (DDL), if it still exists. --#}
  {% if existing_relation is not none %}
    {% set existing_relation = load_cached_relation(existing_relation) %}
    {% if existing_relation is not none %}
      {% do adapter.rename_relation(existing_relation, backup_relation) %}
    {% endif %}
  {% endif %}

  {#-- 4. Move the freshly-built intermediate table into place (DDL). --#}
  {% do adapter.rename_relation(intermediate_relation, target_relation) %}

  {{ run_hooks(post_hooks, inside_transaction=True) }}

  {% set should_revoke = should_revoke(existing_relation, full_refresh_mode=True) %}
  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

  {% do persist_docs(target_relation, model) %}

  {{ adapter.commit() }}

  {#-- 5. Drop the backup now that the swap is committed (DDL). --#}
  {{ drop_relation_if_exists(backup_relation) }}

  {{ run_hooks(post_hooks, inside_transaction=False) }}

  {{ return({'relations': [target_relation]}) }}

{% endmaterialization %}
