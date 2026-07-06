{#
  Spanner incremental materialization. Verified end-to-end against the Spanner
  emulator with the adbc-spanner driver (append + delete_insert paths).

  Spanner has no MERGE, so the only upsert strategy is delete+insert. It also has
  no temp tables and no CTAS (see materializations/table.sql). Two source modes
  for the incremental DML are provided:

    * Inline (default): the compiled model SQL is referenced inline as a subquery.
      Simplest, but the model SQL is executed twice per incremental run (once in
      the DELETE's EXISTS, once in the INSERT), and a non-deterministic model
      could differ between the two.
    * Staging table (`config(incremental_staging_table=true)`): the model output
      is written once into a real staging table (Spanner has no temp tables), the
      DELETE + INSERT read from it, then it is dropped. Runs the model once and is
      deterministic, at the cost of extra asynchronous DDL (create + drop).

  Transaction model (see `statement` / `adapter.commit`):
    * The delete+insert upsert is emitted as one `;`-separated DML batch in a
      single `statement('main')`. The adbc-spanner driver (>= 0.2.0) splits the
      batch and applies it atomically (ExecuteBatchDml) in one transaction.
    * DDL (CREATE / DROP / RENAME) is routed to `UpdateDatabaseDdl` by the driver
      regardless of path; these statements use auto_begin=False.

  Supported `incremental_strategy`: `append`, `delete_insert` (needs `unique_key`;
  the default when a unique_key is set). `merge` / `insert_overwrite` /
  `microbatch` raise a clear error.

  First run and `--full-refresh` reuse the table build (CREATE ... PRIMARY KEY +
  INSERT), so the `primary_key` config is required here too.
#}

{#-- DELETE target rows whose unique_key matches the source rows. Uses a
     correlated EXISTS so composite unique keys work in GoogleSQL. --#}
{% macro spanner_get_delete_by_unique_key_sql(target, source, unique_key, incremental_predicates=none) %}
  {%- if unique_key is string -%}
    {%- set unique_key = [unique_key] -%}
  {%- endif -%}
  delete from {{ target }} as dbt_internal_dest
  where exists (
    select 1
    from {{ source }} as dbt_internal_source
    where
    {%- for key in unique_key %}
      {{ "and " if not loop.first }}dbt_internal_source.{{ key }} = dbt_internal_dest.{{ key }}
    {%- endfor %}
  )
  {%- if incremental_predicates %}
    {%- for predicate in incremental_predicates %}
    and {{ predicate }}
    {%- endfor %}
  {%- endif %}
{% endmacro %}

{#-- Atomic upsert: DELETE matching rows then INSERT the new rows, emitted as one
     `;`-separated batch. The adbc-spanner driver (>= 0.2.0) splits such a batch
     and applies it atomically via ExecuteBatchDml in a single transaction. --#}
{% macro spanner_get_atomic_delete_insert_sql(target, source, unique_key, dest_cols_csv, incremental_predicates=none) %}
  {{ spanner_get_delete_by_unique_key_sql(target, source, unique_key, incremental_predicates) }};

  insert into {{ target }} ({{ dest_cols_csv }})
  select {{ dest_cols_csv }}
  from {{ source }}
{% endmacro %}

{% materialization incremental, adapter='spanner' %}

  {%- set existing_relation = load_cached_relation(this) -%}
  {%- set target_relation = this.incorporate(type='table') -%}

  {%- set unique_key = config.get('unique_key') -%}
  {%- set sql_header = config.get('sql_header', none) -%}
  {%- set incremental_predicates = config.get('predicates', none) or config.get('incremental_predicates', none) -%}
  {%- set full_refresh_mode = (should_full_refresh() or (existing_relation is not none and existing_relation.is_view)) -%}
  {%- set use_staging_table = config.get('incremental_staging_table', false) -%}

  {#-- Resolve the strategy: default to delete_insert when a unique_key is set. --#}
  {%- set strategy = config.get('incremental_strategy') -%}
  {%- if strategy is none or strategy == 'default' -%}
    {%- set strategy = 'delete_insert' if unique_key else 'append' -%}
  {%- endif -%}
  {%- if strategy not in ['append', 'delete_insert'] -%}
    {% do exceptions.raise_compiler_error(
      "The Spanner adapter only supports the 'append' and 'delete_insert' incremental "
      ~ "strategies (got '" ~ strategy ~ "'). Spanner has no MERGE statement."
    ) %}
  {%- endif -%}
  {%- if strategy == 'delete_insert' and not unique_key -%}
    {% do exceptions.raise_compiler_error(
      "incremental_strategy='delete_insert' requires a `unique_key` config on the Spanner adapter."
    ) %}
  {%- endif -%}

  {%- set grant_config = config.get('grants') -%}
  {%- set intermediate_relation = make_intermediate_relation(target_relation) -%}
  {%- set backup_relation_type = 'table' if existing_relation is none else existing_relation.type -%}
  {%- set backup_relation = make_backup_relation(target_relation, backup_relation_type) -%}
  {%- set staging_relation = make_temp_relation(target_relation, '__dbt_stg') -%}
  {%- set preexisting_intermediate_relation = load_cached_relation(intermediate_relation) -%}
  {%- set preexisting_backup_relation = load_cached_relation(backup_relation) -%}
  {%- set preexisting_staging_relation = load_cached_relation(staging_relation) -%}
  {{ drop_relation_if_exists(preexisting_intermediate_relation) }}
  {{ drop_relation_if_exists(preexisting_backup_relation) }}
  {{ drop_relation_if_exists(preexisting_staging_relation) }}

  {{ run_hooks(pre_hooks, inside_transaction=False) }}
  {{ run_hooks(pre_hooks, inside_transaction=True) }}

  {#-- Column list for INSERT, in query order (Spanner requires an explicit list). --#}
  {%- set dest_columns = spanner_get_column_names_from_query(sql, sql_header) -%}
  {%- set dest_cols_csv = dest_columns | join(', ') -%}

  {%- set need_swap = false -%}
  {%- set need_staging_cleanup = false -%}

  {% if existing_relation is none %}
    {#-- First run: create the table schema (DDL, autocommit) then populate (DML). --#}
    {% call statement('main', auto_begin=False) -%}
      {{ spanner__create_table_as(False, target_relation, sql) }}
    {%- endcall %}
    {% call statement('insert_rows') -%}
      insert into {{ target_relation }} ({{ dest_cols_csv }})
      {{ sql }}
    {%- endcall %}

  {% elif full_refresh_mode %}
    {#-- Full refresh: build into an intermediate table (DDL), populate (DML), swap. --#}
    {% call statement('main', auto_begin=False) -%}
      {{ spanner__create_table_as(False, intermediate_relation, sql) }}
    {%- endcall %}
    {% call statement('insert_rows') -%}
      insert into {{ intermediate_relation }} ({{ dest_cols_csv }})
      {{ sql }}
    {%- endcall %}
    {%- set need_swap = true -%}

  {% else %}
    {#-- Incremental run. Choose the DML source: inline subquery or staging table. --#}
    {% if use_staging_table %}
      {#-- Materialize the model output once into a real staging table. --#}
      {% call statement('create_staging', auto_begin=False) -%}
        {{ spanner__create_table_as(False, staging_relation, sql) }}
      {%- endcall %}
      {% call statement('populate_staging') -%}
        insert into {{ staging_relation }} ({{ dest_cols_csv }})
        {{ sql }}
      {%- endcall %}
      {%- set dml_source = staging_relation -%}
      {%- set need_staging_cleanup = true -%}
    {% else %}
      {#-- Inline: reference the compiled model SQL directly as a subquery. --#}
      {%- set dml_source = "(\n" ~ sql ~ "\n)" -%}
    {% endif %}

    {% if strategy == 'delete_insert' %}
      {#-- Atomic DELETE + INSERT: emitted as one `;`-separated batch. The driver
           (>= 0.2.0) runs the batch atomically in a single transaction. --#}
      {% call statement('main') -%}
        {{ spanner_get_atomic_delete_insert_sql(target_relation, dml_source, unique_key, dest_cols_csv, incremental_predicates) }}
      {%- endcall %}
    {% else %}
      {#-- append --#}
      {% call statement('main') -%}
        insert into {{ target_relation }} ({{ dest_cols_csv }})
        select {{ dest_cols_csv }}
        from {{ dml_source }}
      {%- endcall %}
    {% endif %}
  {% endif %}

  {#-- Swap for the full-refresh path (DDL, autocommit). --#}
  {% if need_swap %}
    {% if existing_relation is not none %}
      {% set existing_relation = load_cached_relation(existing_relation) %}
      {% if existing_relation is not none %}
        {% do adapter.rename_relation(existing_relation, backup_relation) %}
      {% endif %}
    {% endif %}
    {% do adapter.rename_relation(intermediate_relation, target_relation) %}
  {% endif %}

  {% set should_revoke = should_revoke(existing_relation, full_refresh_mode) %}
  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

  {% do persist_docs(target_relation, model) %}

  {{ run_hooks(post_hooks, inside_transaction=True) }}

  {#-- Commit the DML transaction. --#}
  {% do adapter.commit() %}

  {#-- Post-commit DDL cleanup (autocommit). --#}
  {% if need_swap %}
    {{ drop_relation_if_exists(backup_relation) }}
  {% endif %}
  {% if need_staging_cleanup %}
    {{ drop_relation_if_exists(staging_relation) }}
  {% endif %}

  {{ run_hooks(post_hooks, inside_transaction=False) }}

  {{ return({'relations': [target_relation]}) }}

{% endmaterialization %}
