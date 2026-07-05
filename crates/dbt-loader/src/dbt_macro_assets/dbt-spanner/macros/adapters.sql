{#
  Spanner (Google Cloud Spanner, GoogleSQL dialect) adapter macros.

  Before editing Spanner behavior, read the agent guide at
  crates/dbt-auth/src/spanner/AGENTS.md (whole-adapter oddities) and the
  user docs at docs/adapters/spanner.md.

  These `spanner__` macros are the dispatch implementations dbt selects when
  adapter.type() == 'spanner'. They override the global `default__` macros
  shipped in the dbt-adapters base package.

  IMPORTANT dialect notes (see individual macros for detail):
    - No CREATE TABLE AS SELECT (CTAS). Tables need an explicit schema + PRIMARY KEY,
      and DDL cannot be mixed with DML. The `table` materialization and
      `spanner__create_table_as` live in materializations/table.sql (DRAFT).
      Set `primary_key` in the model config for table models.
    - Views must be created with `SQL SECURITY INVOKER`.
    - No MERGE and no TRUNCATE statements.
    - INFORMATION_SCHEMA is queryable; the default schema is the empty string ''.
    - Quoting uses backticks (same as BigQuery).
    - No partition/cluster concept in the BigQuery sense.
#}


{#
  Views work on Spanner, but the definition MUST include `SQL SECURITY INVOKER`.
  Shape mirrors bigquery__create_view_as but drops bigquery_view_options and adds
  the required security clause.
#}
{% macro spanner__create_view_as(relation, sql) -%}
  {%- set sql_header = config.get('sql_header', none) -%}

  {{ sql_header if sql_header is not none }}

  create or replace view {{ relation }} sql security invoker
  {%- set contract_config = config.get('contract') -%}
  {%- if contract_config.enforced -%}
    {{ get_assert_columns_equivalent(sql) }}
  {%- endif %}
  as {{ sql }};

{% endmacro %}


{#
  Relation listing via INFORMATION_SCHEMA.TABLES. Spanner exposes table_catalog,
  table_schema, table_name and table_type ('BASE TABLE' | 'VIEW'); the default
  schema is the empty string ''.
#}
{% macro spanner__list_relations_without_caching(schema_relation) %}
  {% call statement('list_relations_without_caching', fetch_result=True) -%}
    select
      table_catalog as database,
      table_name as name,
      table_schema as schema,
      case
        when table_type = 'VIEW' then 'view'
        else 'table'
      end as type
    from information_schema.tables
    where table_schema = '{{ schema_relation.schema }}'
  {% endcall %}
  {{ return(load_result('list_relations_without_caching').table) }}
{% endmacro %}


{% macro spanner__list_schemas(database) -%}
  {% call statement('list_schemas', fetch_result=True, auto_begin=False) %}
    select distinct schema_name
    from information_schema.schemata
  {% endcall %}
  {{ return(load_result('list_schemas').table) }}
{% endmacro %}


{% macro spanner__check_schema_exists(information_schema, schema) -%}
  {% call statement('check_schema_exists', fetch_result=True) -%}
    select count(*)
    from information_schema.schemata
    where schema_name = '{{ schema }}'
  {%- endcall %}
  {{ return(load_result('check_schema_exists').table) }}
{%- endmacro %}


{#
  Spanner does not have the classic SQL CREATE/DROP SCHEMA with `if not exists` /
  `cascade`; named schemas are a newer feature. Keep these minimal and plain.
#}
{#-- Spanner's default schema is the unnamed/empty schema '', which always exists,
     so creating/dropping it is a no-op. Named schemas use a bare `CREATE SCHEMA
     <name>` (no database prefix — the connection is to a single database). --#}
{% macro spanner__create_schema(relation) -%}
  {%- if relation.schema and relation.schema != '' -%}
    {%- call statement('create_schema') -%}
      create schema if not exists {{ adapter.quote(relation.schema) }}
    {%- endcall -%}
  {%- endif -%}
{% endmacro %}


{% macro spanner__drop_schema(relation) -%}
  {%- if relation.schema and relation.schema != '' -%}
    {%- call statement('drop_schema') -%}
      drop schema if exists {{ adapter.quote(relation.schema) }}
    {%- endcall -%}
  {%- endif -%}
{% endmacro %}


{#
  Dispatches to the shared get_drop_sql, which emits `drop view` / `drop table`
  for the respective relation types. Modeled on bigquery__drop_relation.
#}
{% macro spanner__drop_relation(relation) -%}
  {%- call statement('drop_relation', auto_begin=False) -%}
    {{ get_drop_sql(relation) }}
  {%- endcall -%}
{% endmacro %}

{#-- Spanner has no CASCADE on DROP, and supports DROP {VIEW,TABLE} IF EXISTS. --#}
{% macro spanner__get_drop_sql(relation) -%}
  {%- if relation.is_view -%}
    drop view if exists {{ relation.render() }}
  {%- else -%}
    drop table if exists {{ relation.render() }}
  {%- endif -%}
{%- endmacro %}


{#
  Tables can be renamed with ALTER TABLE ... RENAME TO. Views CANNOT be renamed
  on Spanner; that would require a drop + recreate using the stored view
  definition, which is not yet implemented here.
#}
{% macro spanner__rename_relation(from_relation, to_relation) -%}
  {%- if from_relation.is_view -%}
    {# TODO: implement view rename as drop + recreate using the stored view definition #}
    {% do exceptions.raise_compiler_error(
        "Spanner does not support renaming views. Renaming '" ~ from_relation
        ~ "' requires a drop + recreate, which is not yet implemented for the spanner adapter."
    ) %}
  {%- else -%}
    {% set target_name = adapter.quote_as_configured(to_relation.identifier, 'identifier') %}
    {% call statement('rename_relation') -%}
      alter table {{ from_relation.render() }} rename to {{ target_name }}
    {%- endcall %}
  {%- endif -%}
{% endmacro %}


{#
  Spanner has no TRUNCATE statement; emulate with an unconditional DELETE.
#}
{% macro spanner__truncate_relation(relation) -%}
  {% call statement('truncate_relation') -%}
    delete from {{ relation.render() }} where true
  {%- endcall %}
{% endmacro %}


{#
  Column listing via INFORMATION_SCHEMA.COLUMNS. Spanner's canonical type string
  lives in the SPANNER_TYPE column (e.g. 'STRING(MAX)', 'INT64'). The default
  schema is the empty string ''.
#}
{% macro spanner__get_columns_in_relation(relation) -%}
  {% call statement('get_columns_in_relation', fetch_result=True) %}
    select
      column_name,
      spanner_type as data_type
    from information_schema.columns
    where table_schema = '{{ relation.schema }}'
      and table_name = '{{ relation.identifier }}'
      {# TODO: verify whether table_catalog should also be filtered (Spanner default catalog is '') #}
    order by ordinal_position
  {% endcall %}
  {% set table = load_result('get_columns_in_relation').table %}
  {{ return(sql_convert_columns_in_relation(table)) }}
{% endmacro %}


{% macro spanner__current_timestamp() -%}
  current_timestamp()
{%- endmacro %}


{#
  BigQuery implements alter_column_type via a full CTAS with a CAST, which Spanner
  cannot do. Spanner only permits a limited set of in-place type changes through
  ALTER TABLE ALTER COLUMN; a general type change needs a full table rebuild.
#}
{% macro spanner__alter_column_type(relation, column_name, new_column_type) -%}
  {# TODO: Spanner only allows a limited set of column type changes via ALTER TABLE ALTER COLUMN;
     a general type change (as BigQuery does via CTAS + CAST) requires a full table rebuild. #}
  {% call statement('alter_column_type') %}
    alter table {{ relation.render() }} alter column {{ adapter.quote(column_name) }} {{ new_column_type }}
  {% endcall %}
{% endmacro %}


{#
  Spanner permits only ONE ADD COLUMN per ALTER TABLE statement, so emit one
  statement per column (unlike BigQuery, which batches them).
#}
{% macro spanner__alter_relation_add_columns(relation, add_columns) %}
  {% for column in add_columns %}
    {% set sql -%}
      alter table {{ relation.render() }} add column {{ column.name }} {{ column.data_type }}
    {%- endset %}
    {% do run_query(sql) %}
  {% endfor %}
{% endmacro %}
