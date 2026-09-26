{#--
The macro override naming method (spark__statement) only works for macros which are called with adapter.dispatch. For macros called directly, you can just redefine them.
--#}
-- funcsign: (optional[string], optional[bool], optional[bool], optional[string]) -> string
{%- macro statement(name=None, fetch_result=False, auto_begin=True, language='sql') -%}
  {%- if execute: -%}
    {%- set compiled_code = caller() -%}

    {%- if name == 'main' -%}
      {{ log('Writing runtime {} for node "{}"'.format(language, model['unique_id'])) }}
      {{ write(compiled_code) }}
    {%- endif -%}
    {%- if language == 'sql'-%}
      {%- set res, table = adapter.execute(compiled_code, auto_begin=auto_begin, fetch=fetch_result) -%}
    {%- elif language == 'python' -%}
      {%- set res = submit_python_job(model, compiled_code) -%}
      {#-- TODO: What should table be for python models? --#}
      {%- set table = None -%}
    {%- else -%}
      {% do exceptions.raise_compiler_error("statement macro didn't get supported language") %}
    {%- endif -%}

    {%- if name is not none -%}
      {{ store_result(name, response=res, agate_table=table) }}
    {%- endif -%}

  {%- endif -%}
{%- endmacro %}


-- ai
-- funcsign: (optional[string], optional[string], optional[string], optional[string], optional[agate_table], optional[any]) -> string
{% macro noop_statement(name=None, message=None, code=None, rows_affected=None, res=None, response=None) -%}
  {%- set sql = caller() -%}

  {%- if name == 'main' -%}
    {{ log('Writing runtime SQL for node "{}"'.format(model['unique_id'])) }}
    {{ write(sql) }}
  {%- endif -%}

  {%- if name is not none -%}
    {# Omit the new keyword when absent for compatibility with Python's store_raw_result:
       https://github.com/dbt-labs/dbt-core/blob/34bb3f94dde716a3f9c36481d2ead85c211075dd/core/dbt/context/providers.py #}
    {% if response is not none %}
      {{ store_raw_result(name, message=message, code=code, rows_affected=rows_affected, agate_table=res, response=response) }}
    {% else %}
      {{ store_raw_result(name, message=message, code=code, rows_affected=rows_affected, agate_table=res) }}
    {% endif %}
  {%- endif -%}

{%- endmacro %}


{# a user-friendly interface into statements #}
-- funcsign: (string) -> agate_table
{% macro run_query(sql) %}
  {% call statement("run_query_statement", fetch_result=true, auto_begin=false) %}
    {{ sql }}
  {% endcall %}

  {% do return(load_result("run_query_statement").table) %}
{% endmacro %}
