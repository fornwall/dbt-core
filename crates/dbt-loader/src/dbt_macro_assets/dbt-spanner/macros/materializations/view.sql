{#
  Spanner view materialization.

  The default view materialization builds an intermediate view and renames it into
  place, but Spanner cannot rename views. Spanner does support
  `CREATE OR REPLACE VIEW`, so we create the view directly at its target name — no
  intermediate/backup/rename dance. `spanner__create_view_as` emits the required
  `... sql security invoker ...`. CREATE VIEW is DDL, which the driver routes to
  `UpdateDatabaseDdl`, so the statement runs autocommit (auto_begin=False).
#}
{% materialization view, adapter='spanner' %}

  {%- set existing_relation = load_cached_relation(this) -%}
  {%- set target_relation = this.incorporate(type='view') -%}
  {%- set grant_config = config.get('grants') -%}

  {{ run_hooks(pre_hooks, inside_transaction=False) }}
  {{ run_hooks(pre_hooks, inside_transaction=True) }}

  {#-- CREATE OR REPLACE VIEW cannot replace a table of the same name; drop it first. --#}
  {% if existing_relation is not none and not existing_relation.is_view %}
    {% do adapter.drop_relation(existing_relation) %}
  {% endif %}

  {% call statement('main', auto_begin=False) -%}
    {{ get_create_view_as_sql(target_relation, sql) }}
  {%- endcall %}

  {% set should_revoke = should_revoke(existing_relation, full_refresh_mode=True) %}
  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

  {% do persist_docs(target_relation, model) %}

  {{ run_hooks(post_hooks, inside_transaction=True) }}

  {% do adapter.commit() %}

  {{ run_hooks(post_hooks, inside_transaction=False) }}

  {{ return({'relations': [target_relation]}) }}

{% endmaterialization %}
